use std::ffi::OsString;
use std::path::{Path, PathBuf};

use df_process_safety::{run_isolated, ProcessLimits, ProcessSafetyError};
use thiserror::Error;

use crate::types::{ProviderDescriptor, ProviderKind};

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProviderFailure {
    #[error("provider is unavailable")]
    Unavailable,
    #[error("provider request exceeded its deadline")]
    Timeout,
    #[error("provider violated an I/O limit")]
    LimitExceeded,
    #[error("provider protocol failed")]
    Protocol,
}

/// Abstract, suggestion-only provider boundary. Invocation is intentionally a
/// sealed crate operation, so even official provider values cannot bypass the
/// engine's disclosure and consent gate.
pub trait Provider: private::ProviderInvocation + Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;
}

pub(crate) mod private {
    use super::ProviderFailure;

    pub trait ProviderInvocation {
        fn invoke_after_policy(&self, request: &[u8]) -> Result<Vec<u8>, ProviderFailure>;
    }
}

/// A local provider runs only an absolute, explicitly selected sidecar through
/// `df-process-safety` (cleared environment, Job Object, deadline and I/O caps).
pub struct LocalProcessProvider {
    descriptor: ProviderDescriptor,
    executable: PathBuf,
    arguments: Vec<OsString>,
    limits: ProcessLimits,
}

impl LocalProcessProvider {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        executable: impl AsRef<Path>,
        arguments: Vec<OsString>,
        limits: ProcessLimits,
    ) -> Self {
        let executable = executable.as_ref().to_path_buf();
        Self {
            descriptor: ProviderDescriptor {
                kind: ProviderKind::LocalProcess,
                provider: provider.into(),
                model: model.into(),
                endpoint: executable.to_string_lossy().into_owned(),
            },
            executable,
            arguments,
            limits,
        }
    }
}

impl Provider for LocalProcessProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
}

impl private::ProviderInvocation for LocalProcessProvider {
    fn invoke_after_policy(&self, request: &[u8]) -> Result<Vec<u8>, ProviderFailure> {
        run_isolated(&self.executable, &self.arguments, request, self.limits)
            .map_err(map_process_failure)
    }
}

fn map_process_failure(error: ProcessSafetyError) -> ProviderFailure {
    match error {
        ProcessSafetyError::Timeout(_) => ProviderFailure::Timeout,
        ProcessSafetyError::OutputLimit(_) | ProcessSafetyError::InvalidConfiguration(_) => {
            ProviderFailure::LimitExceeded
        }
        ProcessSafetyError::Exit(_) | ProcessSafetyError::Io(_) => ProviderFailure::Protocol,
        ProcessSafetyError::UnsupportedPlatform(_)
        | ProcessSafetyError::Launch { .. }
        | ProcessSafetyError::Isolation(_) => ProviderFailure::Unavailable,
    }
}

/// Injected cloud transport. Authentication belongs to the implementation and
/// is never passed to, inspected by, or recorded by this crate.
pub trait CloudTransport: Send + Sync {
    fn send(&self, endpoint: &str, request: &[u8]) -> Result<Vec<u8>, ProviderFailure>;
}

pub struct CloudProvider<T> {
    descriptor: ProviderDescriptor,
    transport: T,
}

impl<T> CloudProvider<T> {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint: impl Into<String>,
        transport: T,
    ) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                kind: ProviderKind::Cloud,
                provider: provider.into(),
                model: model.into(),
                endpoint: endpoint.into(),
            },
            transport,
        }
    }
}

impl<T: CloudTransport> Provider for CloudProvider<T> {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
}

impl<T: CloudTransport> private::ProviderInvocation for CloudProvider<T> {
    fn invoke_after_policy(&self, request: &[u8]) -> Result<Vec<u8>, ProviderFailure> {
        self.transport.send(&self.descriptor.endpoint, request)
    }
}
