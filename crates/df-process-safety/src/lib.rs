//! Fail-closed execution of trusted sidecars that consume untrusted data.
//!
//! On Windows the child is placed in a one-process Job Object before stdin is
//! released. The job enforces memory limits and kill-on-close; a parent-side
//! deadline terminates and reaps it. Other platforms stay unsupported until
//! an equivalent kernel-enforced backend exists.

#![deny(unsafe_code)]

use std::ffi::OsString;
#[cfg(windows)]
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use df_fs_safety::{metadata_is_reparse, SafeOutputRoot, SafeRelativePath};
use thiserror::Error;

const MIN_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_TIMEOUT: Duration = Duration::from_secs(600);
const MIN_MEMORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_STDIN_BYTES: u64 = 512 * 1024 * 1024;
const MAX_STDOUT_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(windows)]
const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Error)]
pub enum ProcessSafetyError {
    #[error("isolated process configuration is invalid: {0}")]
    InvalidConfiguration(String),
    #[error("isolated process backend is unavailable on {0}")]
    UnsupportedPlatform(&'static str),
    #[error("cannot launch isolated process `{path}`: {source}")]
    Launch {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("isolated process setup failed: {0}")]
    Isolation(String),
    #[error("isolated process exceeded its {0} ms deadline")]
    Timeout(u128),
    #[error("isolated process I/O failed: {0}")]
    Io(String),
    #[error("isolated process exited unsuccessfully ({0})")]
    Exit(String),
    #[error("isolated process stdout exceeded {0} bytes")]
    OutputLimit(u64),
}

pub type ProcessSafetyResult<T> = Result<T, ProcessSafetyError>;

/// Hard resource contract applied to one child invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLimits {
    pub timeout: Duration,
    pub memory_bytes: u64,
    pub max_stdin_bytes: u64,
    pub max_stdout_bytes: u64,
}

impl ProcessLimits {
    pub fn validate(self) -> ProcessSafetyResult<Self> {
        if !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&self.timeout) {
            return Err(ProcessSafetyError::InvalidConfiguration(format!(
                "timeout must be between {} and {} ms",
                MIN_TIMEOUT.as_millis(),
                MAX_TIMEOUT.as_millis()
            )));
        }
        if !(MIN_MEMORY_BYTES..=MAX_MEMORY_BYTES).contains(&self.memory_bytes) {
            return Err(ProcessSafetyError::InvalidConfiguration(format!(
                "memory must be between {MIN_MEMORY_BYTES} and {MAX_MEMORY_BYTES} bytes"
            )));
        }
        if self.max_stdin_bytes == 0 || self.max_stdin_bytes > MAX_STDIN_BYTES {
            return Err(ProcessSafetyError::InvalidConfiguration(format!(
                "stdin limit must be between 1 and {MAX_STDIN_BYTES} bytes"
            )));
        }
        if self.max_stdout_bytes == 0 || self.max_stdout_bytes > MAX_STDOUT_BYTES {
            return Err(ProcessSafetyError::InvalidConfiguration(format!(
                "stdout limit must be between 1 and {MAX_STDOUT_BYTES} bytes"
            )));
        }
        Ok(self)
    }
}

/// Run an explicitly selected sidecar. `PATH`, inherited environment and
/// stderr are never used as part of the protocol.
pub fn run_isolated(
    executable: &Path,
    arguments: &[OsString],
    request: &[u8],
    limits: ProcessLimits,
) -> ProcessSafetyResult<Vec<u8>> {
    let limits = limits.validate()?;
    if !executable.is_absolute() {
        return Err(ProcessSafetyError::InvalidConfiguration(
            "sidecar path must be absolute".to_string(),
        ));
    }
    if u64::try_from(request.len()).unwrap_or(u64::MAX) > limits.max_stdin_bytes {
        return Err(ProcessSafetyError::InvalidConfiguration(format!(
            "request exceeds {} bytes",
            limits.max_stdin_bytes
        )));
    }
    let metadata =
        std::fs::symlink_metadata(executable).map_err(|source| ProcessSafetyError::Launch {
            path: executable.to_path_buf(),
            source,
        })?;
    if metadata_is_reparse(&metadata) || !metadata.is_file() {
        return Err(ProcessSafetyError::InvalidConfiguration(
            "sidecar must be a plain regular file, not a reparse point".to_string(),
        ));
    }
    let parent = executable.parent().ok_or_else(|| {
        ProcessSafetyError::InvalidConfiguration("sidecar has no parent directory".to_string())
    })?;
    let name = executable.file_name().ok_or_else(|| {
        ProcessSafetyError::InvalidConfiguration("sidecar has no file name".to_string())
    })?;
    let root = SafeOutputRoot::validate(parent).map_err(|error| {
        ProcessSafetyError::InvalidConfiguration(format!("unsafe sidecar parent: {error}"))
    })?;
    let relative = SafeRelativePath::parse(Path::new(name)).map_err(|error| {
        ProcessSafetyError::InvalidConfiguration(format!("unsafe sidecar name: {error}"))
    })?;
    let executable_lease = root.lease_existing_file(&relative).map_err(|error| {
        ProcessSafetyError::InvalidConfiguration(format!("cannot lease sidecar: {error}"))
    })?;

    #[cfg(windows)]
    {
        let leased_path = executable_lease.path().to_path_buf();
        run_windows(&leased_path, arguments, request, limits, executable_lease)
    }
    #[cfg(not(windows))]
    {
        let _ = (arguments, request, executable_lease);
        Err(ProcessSafetyError::UnsupportedPlatform(
            std::env::consts::OS,
        ))
    }
}

#[cfg(windows)]
fn run_windows(
    executable: &Path,
    arguments: &[OsString],
    request: &[u8],
    limits: ProcessLimits,
    _executable_lease: df_fs_safety::ReadLease,
) -> ProcessSafetyResult<Vec<u8>> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|source| ProcessSafetyError::Launch {
            path: executable.to_path_buf(),
            source,
        })?;
    let job = match windows_job::Job::create(limits.memory_bytes) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    if let Err(error) = job.assign(&child) {
        job.terminate();
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let Some(mut stdin) = child.stdin.take() else {
        job.terminate();
        let _ = child.wait();
        return Err(ProcessSafetyError::Io(
            "child stdin pipe is unavailable".to_string(),
        ));
    };
    let Some(stdout) = child.stdout.take() else {
        job.terminate();
        let _ = child.wait();
        return Err(ProcessSafetyError::Io(
            "child stdout pipe is unavailable".to_string(),
        ));
    };

    let owned_request = request.to_vec();
    let writer = thread::spawn(move || {
        use std::io::Write;
        stdin
            .write_all(&owned_request)
            .and_then(|()| stdin.flush())
            .map_err(|error| error.to_string())
    });
    let output_limit = limits.max_stdout_bytes;
    let reader = thread::spawn(move || read_bounded(stdout, output_limit));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < limits.timeout => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break Err(ProcessSafetyError::Timeout(limits.timeout.as_millis()));
            }
            Err(error) => {
                job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break Err(ProcessSafetyError::Io(format!(
                    "cannot wait for child: {error}"
                )));
            }
        }
    };
    let write_result = writer
        .join()
        .map_err(|_| ProcessSafetyError::Io("stdin writer panicked".to_string()))?;
    let read_result = reader
        .join()
        .map_err(|_| ProcessSafetyError::Io("stdout reader panicked".to_string()))?;
    let status = status?;
    write_result.map_err(ProcessSafetyError::Io)?;
    if !status.success() {
        return Err(ProcessSafetyError::Exit(status.to_string()));
    }
    read_result
}

#[cfg(windows)]
fn read_bounded(mut reader: impl Read, limit: u64) -> ProcessSafetyResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| ProcessSafetyError::Io(error.to_string()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(ProcessSafetyError::OutputLimit(limit));
    }
    Ok(bytes)
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_job {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };

    pub(super) struct Job(HANDLE);

    impl Job {
        pub(super) fn create(memory_limit_bytes: u64) -> super::ProcessSafetyResult<Self> {
            let memory = usize::try_from(memory_limit_bytes).map_err(|_| {
                super::ProcessSafetyError::Isolation(
                    "memory limit does not fit this platform".to_string(),
                )
            })?;
            // SAFETY: null security/name pointers create a private unnamed job.
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(super::ProcessSafetyError::Isolation(format!(
                    "cannot create Job Object: {}",
                    io::Error::last_os_error()
                )));
            }
            let job = Self(handle);
            // SAFETY: this Windows structure is plain C data.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            limits.ProcessMemoryLimit = memory;
            limits.JobMemoryLimit = memory;
            let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("Windows Job Object structure size fits u32");
            // SAFETY: limits points to a live value of exactly the given size.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size,
                )
            };
            if ok == 0 {
                return Err(super::ProcessSafetyError::Isolation(format!(
                    "cannot configure Job Object: {}",
                    io::Error::last_os_error()
                )));
            }
            Ok(job)
        }

        pub(super) fn assign(&self, child: &Child) -> super::ProcessSafetyResult<()> {
            let process = child.as_raw_handle() as HANDLE;
            // SAFETY: both handles are live during this call.
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(super::ProcessSafetyError::Isolation(format!(
                    "cannot assign child to Job Object: {}",
                    io::Error::last_os_error()
                )));
            }
            Ok(())
        }

        pub(super) fn terminate(&self) {
            // SAFETY: the job handle is live; failure is handled by child.kill.
            let _ = unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: this value uniquely owns the handle.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_hard_bounded() {
        let valid = ProcessLimits {
            timeout: Duration::from_secs(1),
            memory_bytes: 128 * 1024 * 1024,
            max_stdin_bytes: 1024,
            max_stdout_bytes: 1024,
        };
        assert_eq!(valid.validate().unwrap(), valid);
        assert!(ProcessLimits {
            timeout: Duration::ZERO,
            ..valid
        }
        .validate()
        .is_err());
        assert!(ProcessLimits {
            max_stdout_bytes: u64::MAX,
            ..valid
        }
        .validate()
        .is_err());
    }

    #[test]
    fn relative_executable_is_rejected_before_launch() {
        let error = run_isolated(
            Path::new("worker.exe"),
            &[],
            b"request",
            ProcessLimits {
                timeout: Duration::from_secs(1),
                memory_bytes: 128 * 1024 * 1024,
                max_stdin_bytes: 1024,
                max_stdout_bytes: 1024,
            },
        )
        .unwrap_err();
        assert!(matches!(error, ProcessSafetyError::InvalidConfiguration(_)));
    }
}
