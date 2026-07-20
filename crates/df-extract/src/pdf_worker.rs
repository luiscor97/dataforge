use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(windows)]
use std::io::Read;
#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Instant;

use crate::worker_protocol::{self, HARD_MAX_OUTPUT_BYTES};
#[cfg(windows)]
use crate::worker_protocol::{WorkerStatus, MAX_ERROR_BYTES, RESPONSE_HEADER_BYTES};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const MIN_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_MEMORY_LIMIT_BYTES: u64 = 384 * 1024 * 1024;
const MIN_MEMORY_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEMORY_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;
#[cfg(windows)]
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Explicit policy and executable path for the isolated PDF parser.
///
/// The executable is never discovered through `PATH`. On Windows it is
/// assigned to a Job Object with a per-process memory limit and
/// `KILL_ON_JOB_CLOSE` before the parent sends any untrusted bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfWorkerConfig {
    executable: PathBuf,
    timeout: Duration,
    memory_limit_bytes: u64,
}

impl PdfWorkerConfig {
    /// Create a policy for an explicitly selected, absolute worker path.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            timeout: DEFAULT_TIMEOUT,
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
        }
    }

    /// Override the wall-clock deadline. Validation rejects values outside
    /// the built-in 100 ms to 60 second safety range.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Override the Job Object process-memory ceiling. Validation rejects
    /// values outside the built-in 64 MiB to 1 GiB range.
    #[must_use]
    pub fn with_memory_limit_bytes(mut self, bytes: u64) -> Self {
        self.memory_limit_bytes = bytes;
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn memory_limit_bytes(&self) -> u64 {
        self.memory_limit_bytes
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.executable.is_absolute() {
            return Err("PDF worker executable path must be absolute".to_string());
        }
        let metadata = std::fs::symlink_metadata(&self.executable)
            .map_err(|error| format!("cannot inspect PDF worker executable: {error}"))?;
        if is_reparse_point(&metadata) {
            return Err("PDF worker executable cannot be a symlink or reparse point".to_string());
        }
        if !metadata.is_file() {
            return Err("PDF worker executable is not a regular file".to_string());
        }
        for ancestor in self.executable.ancestors().skip(1) {
            let metadata = std::fs::symlink_metadata(ancestor).map_err(|error| {
                format!(
                    "cannot inspect PDF worker executable ancestor `{}`: {error}",
                    ancestor.display()
                )
            })?;
            if is_reparse_point(&metadata) {
                return Err(format!(
                    "PDF worker executable ancestor `{}` is a reparse point",
                    ancestor.display()
                ));
            }
            if !metadata.is_dir() {
                return Err(format!(
                    "PDF worker executable ancestor `{}` is not a directory",
                    ancestor.display()
                ));
            }
        }
        if !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&self.timeout) {
            return Err(format!(
                "PDF worker timeout must be between {} and {} milliseconds",
                MIN_TIMEOUT.as_millis(),
                MAX_TIMEOUT.as_millis()
            ));
        }
        if !(MIN_MEMORY_LIMIT_BYTES..=MAX_MEMORY_LIMIT_BYTES).contains(&self.memory_limit_bytes) {
            return Err(format!(
                "PDF worker memory limit must be between {MIN_MEMORY_LIMIT_BYTES} and {MAX_MEMORY_LIMIT_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

// On POSIX the isolated PDF worker never runs (extraction fails closed
// until M0.8), so these variants are only constructed on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PdfWorkerOutcome {
    Text(String),
    Rejected(String),
    OutputLimit,
    Internal(String),
    Limited(String),
}

pub(crate) fn invoke(
    config: &PdfWorkerConfig,
    input: &[u8],
    max_output_bytes: u64,
) -> PdfWorkerOutcome {
    if let Err(error) = config.validate() {
        return PdfWorkerOutcome::Limited(error);
    }
    if input.is_empty()
        || u64::try_from(input.len()).unwrap_or(u64::MAX) > worker_protocol::HARD_MAX_INPUT_BYTES
    {
        return PdfWorkerOutcome::Limited(
            "PDF input exceeds the worker protocol limit".to_string(),
        );
    }
    if max_output_bytes == 0 || max_output_bytes > HARD_MAX_OUTPUT_BYTES {
        return PdfWorkerOutcome::Limited(
            "PDF output limit exceeds the worker protocol limit".to_string(),
        );
    }

    #[cfg(windows)]
    {
        invoke_windows(config, input, max_output_bytes)
    }
    #[cfg(not(windows))]
    {
        let _ = (input, max_output_bytes);
        PdfWorkerOutcome::Limited(
            "PDF extraction is disabled: this platform has no configured process isolation backend"
                .to_string(),
        )
    }
}

#[cfg(windows)]
fn invoke_windows(
    config: &PdfWorkerConfig,
    input: &[u8],
    max_output_bytes: u64,
) -> PdfWorkerOutcome {
    let mut command = Command::new(&config.executable);
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return PdfWorkerOutcome::Limited(format!("cannot launch PDF worker: {error}"));
        }
    };

    let job = match windows_job::Job::create(config.memory_limit_bytes) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return PdfWorkerOutcome::Limited(error);
        }
    };
    if let Err(error) = job.assign(&child) {
        job.terminate();
        let _ = child.kill();
        let _ = child.wait();
        return PdfWorkerOutcome::Limited(error);
    }

    let Some(mut stdin) = child.stdin.take() else {
        job.terminate();
        let _ = child.wait();
        return PdfWorkerOutcome::Limited("PDF worker stdin pipe is unavailable".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        job.terminate();
        let _ = child.wait();
        return PdfWorkerOutcome::Limited("PDF worker stdout pipe is unavailable".to_string());
    };

    let owned_input = input.to_vec();
    let writer = thread::spawn(move || {
        worker_protocol::write_request(&mut stdin, &owned_input, max_output_bytes)
    });
    let wire_payload_limit = max_output_bytes.max(MAX_ERROR_BYTES as u64);
    let wire_limit = u64::try_from(RESPONSE_HEADER_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(wire_payload_limit);
    let reader = thread::spawn(move || read_bounded_output(stdout, wire_limit));

    let started = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < config.timeout => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break Err(PdfWorkerOutcome::Limited(format!(
                    "PDF worker exceeded its {} millisecond deadline",
                    config.timeout.as_millis()
                )));
            }
            Err(error) => {
                job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break Err(PdfWorkerOutcome::Limited(format!(
                    "cannot wait for PDF worker: {error}"
                )));
            }
        }
    };

    let write_result = writer
        .join()
        .unwrap_or_else(|_| Err("PDF worker request writer panicked".to_string()));
    let read_result = reader
        .join()
        .unwrap_or_else(|_| Err("PDF worker response reader panicked".to_string()));
    let status = match exit_status {
        Ok(status) => status,
        Err(outcome) => return outcome,
    };
    if let Err(error) = write_result {
        return PdfWorkerOutcome::Limited(error);
    }
    if !status.success() {
        return PdfWorkerOutcome::Limited(format!("PDF worker exited unsuccessfully ({status})"));
    }
    let bytes = match read_result {
        Ok(bytes) => bytes,
        Err(error) => return PdfWorkerOutcome::Limited(error),
    };
    decode_response(&bytes, max_output_bytes)
}

#[cfg(windows)]
fn read_bounded_output(mut stdout: impl Read, wire_limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    stdout
        .by_ref()
        .take(wire_limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read PDF worker response: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > wire_limit {
        return Err("PDF worker response exceeded its framed output limit".to_string());
    }
    Ok(bytes)
}

#[cfg(windows)]
fn decode_response(bytes: &[u8], max_output_bytes: u64) -> PdfWorkerOutcome {
    let response = match worker_protocol::parse_response(bytes, max_output_bytes) {
        Ok(response) => response,
        Err(error) => return PdfWorkerOutcome::Limited(error),
    };
    match response.status {
        WorkerStatus::Ok => match String::from_utf8(response.payload) {
            Ok(text) => PdfWorkerOutcome::Text(text),
            Err(_) => PdfWorkerOutcome::Limited(
                "PDF worker returned text that is not valid UTF-8".to_string(),
            ),
        },
        WorkerStatus::Rejected => PdfWorkerOutcome::Rejected(error_payload(response.payload)),
        WorkerStatus::OutputLimit => PdfWorkerOutcome::OutputLimit,
        WorkerStatus::Internal => PdfWorkerOutcome::Internal(error_payload(response.payload)),
    }
}

#[cfg(windows)]
fn error_payload(payload: Vec<u8>) -> String {
    String::from_utf8(payload)
        .unwrap_or_else(|_| "PDF worker returned a non-UTF-8 error".to_string())
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
        pub(super) fn create(memory_limit_bytes: u64) -> Result<Self, String> {
            let memory_limit = usize::try_from(memory_limit_bytes)
                .map_err(|_| "PDF worker memory limit does not fit this platform".to_string())?;
            // SAFETY: null security/name pointers request a private unnamed Job
            // Object. The returned owned handle is closed by Drop.
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(format!(
                    "cannot create PDF worker Job Object: {}",
                    io::Error::last_os_error()
                ));
            }
            let job = Self(handle);
            // SAFETY: the Windows structure is plain C data and zero is the
            // documented neutral value for fields whose limit flags are unset.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            limits.ProcessMemoryLimit = memory_limit;
            limits.JobMemoryLimit = memory_limit;
            let structure_bytes = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .expect("Windows Job Object structure size fits u32");
            // SAFETY: `limits` points to a live structure of the declared size;
            // `job.0` is an owned Job Object handle.
            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    structure_bytes,
                )
            };
            if configured == 0 {
                return Err(format!(
                    "cannot configure PDF worker Job Object: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(job)
        }

        pub(super) fn assign(&self, child: &Child) -> Result<(), String> {
            let process = child.as_raw_handle() as HANDLE;
            // SAFETY: both handles are live for the duration of the call.
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(format!(
                    "cannot isolate PDF worker in its Job Object: {}",
                    io::Error::last_os_error()
                ));
            }
            Ok(())
        }

        pub(super) fn terminate(&self) {
            // SAFETY: `self.0` remains an owned live Job Object handle. Errors
            // are intentionally ignored because the caller also kills/waits.
            let _ = unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned uniquely by this value and closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_rejects_relative_paths_and_unbounded_values() {
        let relative = PdfWorkerConfig::new("worker.exe");
        assert!(relative.validate().is_err());

        let executable = std::env::current_exe().unwrap();
        assert!(PdfWorkerConfig::new(&executable)
            .with_timeout(Duration::ZERO)
            .validate()
            .is_err());
        assert!(PdfWorkerConfig::new(executable)
            .with_memory_limit_bytes(u64::MAX)
            .validate()
            .is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_fails_closed_without_spawning() {
        let config = PdfWorkerConfig::new(std::env::current_exe().unwrap());
        assert!(matches!(
            invoke(&config, b"%PDF-1.4", 1024),
            PdfWorkerOutcome::Limited(message) if message.contains("disabled")
        ));
    }
}
