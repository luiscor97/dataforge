//! Safe filesystem boundary for DataForge's output writes.
//!
//! Everything DataForge writes goes through this crate. Its job is to make the
//! central promise demonstrable: **nothing is written outside the authorised
//! output root, and nothing preexisting is overwritten** (RFC-0001 rules 2
//! and 3).
//!
//! Why a dedicated layer (ADR-0017): validating that a destination is relative
//! and free of `..` is *text*, and text says nothing about the filesystem. If
//! `Salida\clientes` already exists and is a junction to `C:\DatosExternos`,
//! `output_root.join("clientes/x.pdf")` is a perfectly well-formed relative
//! path that writes somewhere else entirely. `canonicalize` does not help
//! either: it *follows* links, so it happily reports the escaped location as
//! legitimate.
//!
//! So the rules here are:
//! - resolve the destination **component by component**, rejecting any
//!   existing component that carries a reparse point (symlink, junction or
//!   mount point — Windows does not distinguish them in the attribute);
//! - identify the output root **physically** (volume serial + file index) and
//!   re-check that identity during execution, not just once at the start;
//! - finalize with a platform primitive that **refuses** to replace, instead
//!   of a check-then-rename that races.
//!
//! ## Platform support
//!
//! Windows is the only platform with a real implementation in v0.1.1-dev.
//! Every other platform returns [`FsSafetyError::UnsupportedPlatform`] from
//! [`SafeOutputRoot::validate`], which blocks execution rather than pretending
//! (RFC-0001 rule 19: no claiming a guarantee without evidence).

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Typed failures of the safe filesystem layer.
#[derive(Debug, thiserror::Error)]
pub enum FsSafetyError {
    /// A component of the path is (or became) a reparse point.
    #[error("`{path}` is a reparse point (symlink, junction or mount point); refusing to write through it")]
    ReparsePoint { path: PathBuf },

    /// The resolved path left the output root.
    #[error("resolved path `{resolved}` is outside the output root `{root}`")]
    OutsideOutputRoot { resolved: PathBuf, root: PathBuf },

    /// The output root is no longer the directory we validated.
    #[error("the output root `{root}` is no longer the same physical directory")]
    OutputRootIdentityChanged { root: PathBuf },

    /// Two roots that look separate as strings resolve to the same physical
    /// tree (or one resolves below the other).
    #[error("filesystem roots `{left}` and `{right}` overlap physically")]
    PhysicalRootOverlap { left: PathBuf, right: PathBuf },

    /// A root cannot be resolved safely without creating anything.
    #[error("invalid filesystem root `{path}`: {reason}")]
    InvalidRootPath { path: PathBuf, reason: String },

    /// The destination already exists; we never replace.
    #[error("destination `{path}` already exists; DataForge never overwrites")]
    DestinationExists { path: PathBuf },

    /// The relative path is not usable as a destination.
    #[error("invalid relative destination `{path}`: {reason}")]
    InvalidRelativePath { path: PathBuf, reason: String },

    /// This platform has no safe implementation yet.
    #[error("filesystem safety is only implemented on Windows in this version; refusing to execute on {platform}")]
    UnsupportedPlatform { platform: &'static str },

    #[error("I/O error at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl FsSafetyError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<FsSafetyError> for df_error::DfError {
    fn from(error: FsSafetyError) -> Self {
        match error {
            FsSafetyError::Io { path, source } => df_error::DfError::io(path, source),
            other => df_error::DfError::Validation(other.to_string()),
        }
    }
}

pub type FsResult<T> = Result<T, FsSafetyError>;

/// Physical identity of a filesystem object.
///
/// On Windows this is `(volume serial, file index)` from
/// `GetFileInformationByHandle` — the closest thing to an inode. Two paths
/// with the same identity are the same object, whatever alias, junction or
/// 8.3 name was used to reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FileIdentity {
    pub volume_serial: u64,
    pub file_index: u64,
}

/// How confidently we could identify an object (RFC-0001 §13.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IdentityLevel {
    /// The filesystem gave us volume serial + file index.
    Physical,
    /// The filesystem could not; callers must not treat it as strong identity.
    Degraded,
}

/// A relative destination that has passed *textual* validation.
///
/// This is deliberately the weak half: it proves the path is relative, has no
/// `..`, no root/prefix and no empty or reserved components. It proves nothing
/// about the filesystem — that is [`SafeOutputRoot`]'s job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeRelativePath {
    components: Vec<String>,
}

impl SafeRelativePath {
    /// Validate a relative destination path.
    pub fn parse(path: &Path) -> FsResult<Self> {
        let invalid = |reason: &str| FsSafetyError::InvalidRelativePath {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        };
        if path.as_os_str().is_empty() {
            return Err(invalid("empty"));
        }
        let mut components = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => {
                    let text = part.to_string_lossy();
                    if text.is_empty() {
                        return Err(invalid("empty component"));
                    }
                    // Trailing dots and spaces are silently stripped by the
                    // Win32 layer, so `a ` and `a` would collide and a
                    // destination could be redirected to an unintended file.
                    if text.ends_with(' ') || text.ends_with('.') {
                        return Err(invalid(
                            "component ends with a space or dot, which Windows strips",
                        ));
                    }
                    if text.encode_utf16().count() > 255 {
                        return Err(invalid(
                            "component exceeds the portable Windows limit of 255 UTF-16 units",
                        ));
                    }
                    if text.chars().any(|character| {
                        character < '\u{20}'
                            || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\')
                    }) {
                        return Err(invalid(
                            "component contains a character Windows cannot create safely",
                        ));
                    }
                    let device_stem = text
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    let reserved_device = matches!(
                        device_stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
                    ) || device_stem
                        .strip_prefix("COM")
                        .or_else(|| device_stem.strip_prefix("LPT"))
                        .is_some_and(|number| {
                            matches!(
                                number,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        });
                    if reserved_device {
                        return Err(invalid("component is a reserved Windows device name"));
                    }
                    components.push(text.into_owned());
                }
                Component::ParentDir => return Err(invalid("contains `..`")),
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => {
                    return Err(invalid("must be relative, without root or drive prefix"))
                }
            }
        }
        if components.is_empty() {
            return Err(invalid("resolves to no components"));
        }
        Ok(Self { components })
    }

    pub fn components(&self) -> &[String] {
        &self.components
    }

    pub fn to_path(&self) -> PathBuf {
        self.components.iter().collect()
    }

    /// Last component (the file or directory name itself).
    pub fn file_name(&self) -> &str {
        self.components
            .last()
            .map(String::as_str)
            .expect("a SafeRelativePath always has at least one component")
    }

    /// The containing directory, or `None` when this sits directly at the root.
    pub fn parent(&self) -> Option<Self> {
        if self.components.len() <= 1 {
            return None;
        }
        Some(Self {
            components: self.components[..self.components.len() - 1].to_vec(),
        })
    }

    /// Same directory, different last component. The new name is validated
    /// exactly like a parsed one, so a crafted name cannot smuggle in a
    /// separator or `..`.
    pub fn with_file_name(&self, name: &str) -> FsResult<Self> {
        let candidate = Self::parse(Path::new(name))?;
        if candidate.components.len() != 1 {
            return Err(FsSafetyError::InvalidRelativePath {
                path: PathBuf::from(name),
                reason: "a file name must be a single component".to_string(),
            });
        }
        let mut components = self.components.clone();
        components.pop();
        components.push(candidate.components.into_iter().next().expect("one"));
        Ok(Self { components })
    }
}

/// What we learned about one component of a destination path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentInfo {
    pub path: PathBuf,
    /// The component does not exist yet (so it cannot be a link).
    pub exists: bool,
    pub is_reparse_point: bool,
    pub is_dir: bool,
}

/// An output root that has been validated and physically identified.
#[derive(Debug, Clone)]
pub struct SafeOutputRoot {
    path: PathBuf,
    identity: Option<FileIdentity>,
}

impl SafeOutputRoot {
    /// Validate and identify the output root.
    ///
    /// Fails on platforms without a safe implementation, so a caller can never
    /// execute believing it is protected when it is not.
    pub fn validate(path: &Path) -> FsResult<Self> {
        if !cfg!(windows) {
            return Err(FsSafetyError::UnsupportedPlatform {
                platform: std::env::consts::OS,
            });
        }
        if !path.is_absolute() {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "the output root must be absolute".to_string(),
            });
        }
        std::fs::create_dir_all(path).map_err(|e| FsSafetyError::io(path, e))?;
        if is_reparse_point(path)? {
            return Err(FsSafetyError::ReparsePoint {
                path: path.to_path_buf(),
            });
        }
        let identity = identity_of(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            identity,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> Option<FileIdentity> {
        self.identity
    }

    pub fn identity_level(&self) -> IdentityLevel {
        match self.identity {
            Some(_) => IdentityLevel::Physical,
            None => IdentityLevel::Degraded,
        }
    }

    /// Re-check that the output root is still the very same directory.
    ///
    /// Called during execution, not only at the start: a root swapped for a
    /// junction mid-run must stop the run (threat T3).
    pub fn revalidate(&self) -> FsResult<()> {
        if is_reparse_point(&self.path)? {
            return Err(FsSafetyError::ReparsePoint {
                path: self.path.clone(),
            });
        }
        let current = identity_of(&self.path)?;
        match (self.identity, current) {
            (Some(expected), Some(now)) if expected == now => Ok(()),
            (None, _) | (_, None) => Ok(()), // degraded: nothing to compare
            _ => Err(FsSafetyError::OutputRootIdentityChanged {
                root: self.path.clone(),
            }),
        }
    }

    /// Inspect every component of a destination without following anything.
    pub fn inspect_path_components(
        &self,
        relative: &SafeRelativePath,
    ) -> FsResult<Vec<ComponentInfo>> {
        let mut out = Vec::new();
        let mut current = self.path.clone();
        for component in relative.components() {
            current.push(component);
            let metadata = match std::fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    out.push(ComponentInfo {
                        path: current.clone(),
                        exists: false,
                        is_reparse_point: false,
                        is_dir: false,
                    });
                    continue;
                }
                Err(e) => return Err(FsSafetyError::io(&current, e)),
            };
            out.push(ComponentInfo {
                path: current.clone(),
                exists: true,
                is_reparse_point: metadata_is_reparse_point(&metadata),
                is_dir: metadata.is_dir(),
            });
        }
        Ok(out)
    }

    /// Resolve a destination, refusing to traverse any link.
    ///
    /// This is the heart of the boundary: every *existing* component must be a
    /// plain directory. A component that does not exist yet cannot redirect us,
    /// and will be created by [`Self::create_directory_secure`], which checks
    /// again as it goes.
    pub fn resolve_destination_without_following_links(
        &self,
        relative: &SafeRelativePath,
    ) -> FsResult<SecureDestination> {
        self.revalidate()?;
        let components = self.inspect_path_components(relative)?;
        for (index, info) in components.iter().enumerate() {
            if info.is_reparse_point {
                return Err(FsSafetyError::ReparsePoint {
                    path: info.path.clone(),
                });
            }
            // Every component but the last must be a directory if it exists.
            let is_last = index + 1 == components.len();
            if info.exists && !is_last && !info.is_dir {
                return Err(FsSafetyError::InvalidRelativePath {
                    path: info.path.clone(),
                    reason: "an intermediate component exists and is not a directory".to_string(),
                });
            }
        }
        let absolute = self.path.join(relative.to_path());
        // Belt and braces: the assembled path must still be under the root
        // textually. The link checks above are what actually protect us.
        if !absolute.starts_with(&self.path) {
            return Err(FsSafetyError::OutsideOutputRoot {
                resolved: absolute,
                root: self.path.clone(),
            });
        }
        Ok(SecureDestination {
            absolute,
            relative: relative.clone(),
        })
    }

    /// Create a directory (and its missing parents) checking every step.
    ///
    /// Unlike `create_dir_all`, this refuses to walk through a component that
    /// is a reparse point, and re-checks after each level so a directory that
    /// turns into a junction mid-way is caught.
    pub fn create_directory_secure(&self, relative: &SafeRelativePath) -> FsResult<PathBuf> {
        self.revalidate()?;
        let mut current = self.path.clone();
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if metadata_is_reparse_point(&metadata) {
                        return Err(FsSafetyError::ReparsePoint { path: current });
                    }
                    if !metadata.is_dir() {
                        return Err(FsSafetyError::InvalidRelativePath {
                            path: current,
                            reason: "exists and is not a directory".to_string(),
                        });
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::create_dir(&current) {
                        Ok(()) => {}
                        // Someone else created it in the meantime: accept it
                        // only if it is a plain directory.
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            let metadata = std::fs::symlink_metadata(&current)
                                .map_err(|e| FsSafetyError::io(&current, e))?;
                            if metadata_is_reparse_point(&metadata) {
                                return Err(FsSafetyError::ReparsePoint { path: current });
                            }
                            if !metadata.is_dir() {
                                return Err(FsSafetyError::InvalidRelativePath {
                                    path: current,
                                    reason: "appeared and is not a directory".to_string(),
                                });
                            }
                        }
                        Err(e) => return Err(FsSafetyError::io(&current, e)),
                    }
                }
                Err(e) => return Err(FsSafetyError::io(&current, e)),
            }
        }
        Ok(current)
    }

    /// Create the partial file for a destination, refusing links.
    pub fn create_partial_secure(&self, partial: &SafeRelativePath) -> FsResult<std::fs::File> {
        self.revalidate()?;
        let destination = self.resolve_destination_without_following_links(partial)?;
        // create_new: never reuse or truncate an existing partial.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.absolute())
            .map_err(|e| FsSafetyError::io(destination.absolute(), e))
    }
}

/// A destination proven to live inside its output root.
#[derive(Debug, Clone)]
pub struct SecureDestination {
    absolute: PathBuf,
    relative: SafeRelativePath,
}

impl SecureDestination {
    pub fn absolute(&self) -> &Path {
        &self.absolute
    }

    pub fn relative(&self) -> &SafeRelativePath {
        &self.relative
    }
}

/// Finalize a partial into its destination **without ever replacing**.
///
/// The guarantee comes from the platform, not from a prior `exists()` check:
/// an `exists()` check is a race (threat T3/T4). On Windows this is
/// `MoveFileExW` *without* `MOVEFILE_REPLACE_EXISTING`, so the kernel itself
/// fails if the destination appeared meanwhile.
///
/// Note `std::fs::rename` must NOT be used for this: on Windows it passes
/// `MOVEFILE_REPLACE_EXISTING` and silently overwrites.
pub fn finalize_no_replace(partial: &Path, destination: &Path) -> FsResult<()> {
    platform::finalize_no_replace(partial, destination)
}

/// Capture the physical fingerprint of a file (RFC-0001 §14.1, ADR-0019).
///
/// Always produces a **v2** fingerprint. When the filesystem cannot supply an
/// identity the fingerprint is v2-but-degraded, never a v1: the caller can
/// then see, from the value itself, that a same-size same-mtime substitution
/// would go unnoticed here.
///
/// The file is opened for metadata only and never followed if it is a reparse
/// point, so this cannot be tricked into fingerprinting a link's target.
pub fn capture_fingerprint(path: &Path) -> FsResult<df_domain::FileFingerprint> {
    platform::capture_fingerprint(path)
}

/// Is this already-read metadata a reparse point?
///
/// For callers that already hold `symlink_metadata` and must not pay for a
/// second stat (e.g. a directory walk). Note the metadata **must** come from
/// `symlink_metadata`: `metadata()` follows links and would report the target.
pub fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    platform::metadata_is_reparse_point(metadata)
}

/// Is this path a reparse point? Never follows it.
pub fn is_reparse_point(path: &Path) -> FsResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata_is_reparse_point(&metadata)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(FsSafetyError::io(path, e)),
    }
}

/// Reject a source root that is itself a symlink, junction or mount point.
///
/// Directory walkers inspect every child with `symlink_metadata`, but the
/// starting root is supplied by configuration. Without this explicit check,
/// `read_dir(root)` follows a junction before the walker gets a chance to
/// apply its no-follow policy.
pub fn ensure_root_is_not_reparse(path: &Path) -> FsResult<()> {
    if is_reparse_point(path)? {
        return Err(FsSafetyError::ReparsePoint {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Prove that two absolute roots are physically disjoint without creating
/// either path.
///
/// Existing prefixes are canonicalized, which resolves aliases such as
/// junctions, 8.3 names and alternate spellings. Any not-yet-existing suffix
/// is then appended to that physical prefix before the containment check. This
/// lets callers reject `source -> output` aliases *before* creating an output
/// directory inside the source tree.
pub fn ensure_physical_roots_disjoint(left: &Path, right: &Path) -> FsResult<()> {
    let resolved_left = resolve_physical_path_without_creating(left)?;
    let resolved_right = resolve_physical_path_without_creating(right)?;
    if physical_paths_overlap(&resolved_left, &resolved_right) {
        return Err(FsSafetyError::PhysicalRootOverlap {
            left: left.to_path_buf(),
            right: right.to_path_buf(),
        });
    }
    Ok(())
}

/// Resolve the deepest existing ancestor and preserve the missing suffix.
/// This function is deliberately read-only: security validation must happen
/// before `SafeOutputRoot::validate` is allowed to create the output root.
fn resolve_physical_path_without_creating(path: &Path) -> FsResult<PathBuf> {
    let invalid = |reason: &str| FsSafetyError::InvalidRootPath {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if !path.is_absolute() {
        return Err(invalid("must be absolute"));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(invalid("must not contain `..`"));
    }

    let mut existing = path.to_path_buf();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut resolved = std::fs::canonicalize(&existing)
                    .map_err(|error| FsSafetyError::io(&existing, error))?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = existing
                    .file_name()
                    .ok_or_else(|| invalid("has no reachable existing ancestor"))?
                    .to_os_string();
                missing.push(component);
                if !existing.pop() {
                    return Err(invalid("has no reachable existing ancestor"));
                }
            }
            Err(error) => return Err(FsSafetyError::io(&existing, error)),
        }
    }
}

#[cfg(windows)]
fn physical_paths_overlap(left: &Path, right: &Path) -> bool {
    let components = |path: &Path| -> Vec<String> {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let left = components(left);
    let right = components(right);
    let shorter = left.len().min(right.len());
    left[..shorter] == right[..shorter]
}

#[cfg(not(windows))]
fn physical_paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Physical identity of a path, or `None` when the filesystem cannot give one.
pub fn identity_of(path: &Path) -> FsResult<Option<FileIdentity>> {
    platform::identity_of(path)
}

fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    platform::metadata_is_reparse_point(metadata)
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod platform {
    use super::{FileIdentity, FsResult, FsSafetyError};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, MoveFileExW, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        MOVEFILE_WRITE_THROUGH,
    };

    pub(super) fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
        // Covers symlink, junction and mount point alike: Windows flags them
        // all with the same attribute, which is exactly what we want to refuse.
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    fn to_wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// Open a handle to query identity, without following a reparse point and
    /// without requiring read access to the contents.
    fn open_for_query(path: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    pub(super) fn identity_of(path: &Path) -> FsResult<Option<FileIdentity>> {
        let file = match open_for_query(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(FsSafetyError::io(path, e)),
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `file` owns a live handle for the duration of the call and
        // `info` is a properly sized, zeroed output struct.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
        if ok == 0 {
            // Some filesystems (notably certain network redirectors) do not
            // provide file indices. That is a degraded identity, not an error.
            return Ok(None);
        }
        let file_index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        if file_index == 0 {
            return Ok(None);
        }
        Ok(Some(FileIdentity {
            volume_serial: info.dwVolumeSerialNumber as u64,
            file_index,
        }))
    }

    /// FILETIME (100-ns ticks since 1601-01-01) -> Unix milliseconds.
    fn filetime_to_unix_ms(ticks: i64) -> Option<i64> {
        const EPOCH_DIFF_TICKS: i64 = 116_444_736_000_000_000;
        if ticks <= 0 {
            return None;
        }
        Some((ticks - EPOCH_DIFF_TICKS) / 10_000)
    }

    pub(super) fn capture_fingerprint(path: &Path) -> FsResult<df_domain::FileFingerprint> {
        use windows_sys::Win32::Storage::FileSystem::{
            FileBasicInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO,
        };

        let file = open_for_query(path).map_err(|e| FsSafetyError::io(path, e))?;
        let handle = file.as_raw_handle() as HANDLE;

        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: live handle, properly sized zeroed output.
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(FsSafetyError::io(path, std::io::Error::last_os_error()));
        }

        let size_bytes = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
        let file_index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        let identity = if file_index == 0 {
            // No file id: degraded, and the fingerprint says so.
            None
        } else {
            Some(df_domain::PhysicalIdentity {
                volume_serial: info.dwVolumeSerialNumber as u64,
                file_id: file_index,
            })
        };
        let modified_at_ms = filetime_to_unix_ms(
            ((info.ftLastWriteTime.dwHighDateTime as i64) << 32)
                | info.ftLastWriteTime.dwLowDateTime as i64,
        );

        // Change time needs the Ex call; it moves even when a writer restores
        // the modification time, so it is worth the extra syscall.
        let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: live handle; buffer size matches the requested class.
        let change_time_ms = if unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                &mut basic as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            None
        } else {
            filetime_to_unix_ms(basic.ChangeTime)
        };

        Ok(df_domain::FileFingerprint::V2(df_domain::FingerprintV2 {
            size_bytes,
            modified_at_ms,
            change_time_ms,
            attributes: info.dwFileAttributes,
            identity,
        }))
    }

    pub(super) fn finalize_no_replace(partial: &Path, destination: &Path) -> FsResult<()> {
        let from = to_wide(partial);
        let to = to_wide(destination);
        // No MOVEFILE_REPLACE_EXISTING: the kernel refuses if `to` exists.
        // MOVEFILE_WRITE_THROUGH asks NTFS to flush the metadata change before
        // returning (see ADR-0021 on what this does and does not guarantee).
        // SAFETY: both buffers are NUL-terminated and live for the call.
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            return match error.kind() {
                std::io::ErrorKind::AlreadyExists => Err(FsSafetyError::DestinationExists {
                    path: destination.to_path_buf(),
                }),
                _ => Err(FsSafetyError::io(destination, error)),
            };
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Non-Windows: no safe implementation yet — refuse rather than pretend.
// ---------------------------------------------------------------------------
#[cfg(not(windows))]
mod platform {
    use super::{FileIdentity, FsResult, FsSafetyError};
    use std::path::Path;

    pub(super) fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
        metadata.file_type().is_symlink()
    }

    pub(super) fn identity_of(_path: &Path) -> FsResult<Option<FileIdentity>> {
        Ok(None)
    }

    pub(super) fn finalize_no_replace(_partial: &Path, _destination: &Path) -> FsResult<()> {
        Err(FsSafetyError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
    }

    /// Degraded capture: size and mtime only, and honestly labelled as such
    /// (no identity), since this platform has no implementation yet.
    pub(super) fn capture_fingerprint(path: &Path) -> FsResult<df_domain::FileFingerprint> {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| FsSafetyError::io(path, e))?;
        let modified_at_ms = metadata.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
        });
        Ok(df_domain::FileFingerprint::V2(df_domain::FingerprintV2 {
            size_bytes: metadata.len(),
            modified_at_ms,
            change_time_ms: None,
            attributes: 0,
            identity: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_reject_traversal_and_absolutes() {
        assert!(SafeRelativePath::parse(Path::new("a/b.txt")).is_ok());
        assert!(SafeRelativePath::parse(Path::new("../x")).is_err());
        assert!(SafeRelativePath::parse(Path::new("a/../b")).is_err());
        assert!(SafeRelativePath::parse(Path::new("")).is_err());
        assert!(SafeRelativePath::parse(Path::new("/abs")).is_err());
        #[cfg(windows)]
        {
            assert!(SafeRelativePath::parse(Path::new("C:\\abs")).is_err());
            assert!(SafeRelativePath::parse(Path::new("\\\\?\\C:\\x")).is_err());
        }
    }

    #[test]
    fn trailing_dots_and_spaces_are_rejected() {
        // Windows strips these, so `informe ` and `informe` would collide.
        assert!(SafeRelativePath::parse(Path::new("informe ")).is_err());
        assert!(SafeRelativePath::parse(Path::new("informe.")).is_err());
        assert!(SafeRelativePath::parse(Path::new("ok/informe .txt")).is_ok());
    }

    #[test]
    fn windows_device_names_and_illegal_characters_are_rejected_everywhere() {
        for component in [
            "CON",
            "con.txt",
            "PRN",
            "AUX.log",
            "NUL",
            "COM1",
            "com9.bin",
            "LPT1",
            "lpt9.txt",
            "name:stream",
            "bad?.txt",
            "bad*.txt",
            "bad|name",
            "bad<name>",
            "CONIN$",
            "conout$.txt",
            "CLOCK$",
            "COM¹.txt",
            "LPT³",
        ] {
            assert!(
                SafeRelativePath::parse(Path::new(component)).is_err(),
                "`{component}` must be rejected on every build platform"
            );
        }
        for component in ["CONTRATO.txt", "COM10", "LPT0", "auxiliar", "normal.txt"] {
            assert!(
                SafeRelativePath::parse(Path::new(component)).is_ok(),
                "`{component}` is not a reserved device"
            );
        }
        #[cfg(not(windows))]
        assert!(SafeRelativePath::parse(Path::new("back\\slash.txt")).is_err());
        let too_long = format!("{}.txt", "a".repeat(252));
        assert!(SafeRelativePath::parse(Path::new(&too_long)).is_err());
    }

    #[test]
    fn parent_and_with_file_name_stay_inside() {
        let rel = SafeRelativePath::parse(Path::new("a/b/c.txt")).unwrap();
        assert_eq!(rel.file_name(), "c.txt");
        assert_eq!(
            rel.parent().unwrap().components(),
            &["a".to_string(), "b".to_string()]
        );
        let sibling = rel.with_file_name(".c.txt.partial").unwrap();
        assert_eq!(sibling.components(), &["a", "b", ".c.txt.partial"]);
        // A crafted name cannot smuggle a separator or traversal.
        assert!(rel.with_file_name("../escape").is_err());
        assert!(rel.with_file_name("sub/escape").is_err());
        // A single-component path has no parent.
        assert!(SafeRelativePath::parse(Path::new("x"))
            .unwrap()
            .parent()
            .is_none());
    }

    #[test]
    fn current_dir_components_are_ignored() {
        let parsed = SafeRelativePath::parse(Path::new("./a/./b.txt")).unwrap();
        assert_eq!(parsed.components(), &["a".to_string(), "b.txt".to_string()]);
    }

    #[test]
    fn normal_sibling_roots_are_physically_disjoint_even_before_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left").join("future");
        let right = tmp.path().join("right").join("future");
        ensure_physical_roots_disjoint(&left, &right).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_refuses_to_validate_an_output_root() {
        let tmp = tempfile::tempdir().unwrap();
        let err = SafeOutputRoot::validate(tmp.path()).unwrap_err();
        assert!(matches!(err, FsSafetyError::UnsupportedPlatform { .. }));
    }

    #[cfg(windows)]
    mod windows {
        use super::*;

        fn root() -> (tempfile::TempDir, SafeOutputRoot) {
            let tmp = tempfile::tempdir().unwrap();
            let root = SafeOutputRoot::validate(tmp.path()).unwrap();
            (tmp, root)
        }

        #[test]
        fn a_normal_destination_resolves_and_creates() {
            let (_tmp, root) = root();
            let rel = SafeRelativePath::parse(Path::new("origen/sub")).unwrap();
            root.create_directory_secure(&rel).unwrap();
            let file = SafeRelativePath::parse(Path::new("origen/sub/x.txt")).unwrap();
            let dest = root
                .resolve_destination_without_following_links(&file)
                .unwrap();
            assert!(dest.absolute().starts_with(root.path()));
        }

        /// P0-4 / threat T6: the reason v2 exists. Swap a file for a different
        /// one of the same size, restoring the mtime the way any copy tool
        /// would. v1 (`size` + `mtime`) saw nothing; v2 catches it via file id.
        #[test]
        fn a_same_size_same_mtime_substitution_is_detected() {
            use df_domain::{FileFingerprint, FingerprintVerdict};

            let (_tmp, root) = root();
            let victim = root.path().join("contrato.txt");
            std::fs::write(&victim, b"ORIGINAL-1234567890").unwrap();
            let before = capture_fingerprint(&victim).unwrap();

            // Replace it with a different file of identical length, then put
            // the original modification time back.
            let stolen_mtime = std::fs::metadata(&victim).unwrap().modified().unwrap();
            std::fs::remove_file(&victim).unwrap();
            std::fs::write(&victim, b"FALSIFICADO-7890123").unwrap();
            let handle = std::fs::OpenOptions::new()
                .write(true)
                .open(&victim)
                .unwrap();
            handle.set_modified(stolen_mtime).unwrap();
            drop(handle);

            let after = capture_fingerprint(&victim).unwrap();

            // The trap: on these two, v1's fields agree.
            assert_eq!(before.size_bytes(), after.size_bytes(), "sizes must match");
            assert_eq!(
                before.modified_at_ms(),
                after.modified_at_ms(),
                "the test must actually restore the mtime, or it proves nothing"
            );

            // v2 still catches it, because the file id changed.
            assert_eq!(
                FileFingerprint::compare(&before, &after),
                FingerprintVerdict::Changed,
                "a same-size same-mtime substitution went undetected"
            );
        }

        #[test]
        fn a_captured_fingerprint_carries_physical_identity() {
            use df_domain::FingerprintGuarantee;

            let (_tmp, root) = root();
            let file = root.path().join("x.txt");
            std::fs::write(&file, b"hola").unwrap();
            let fp = capture_fingerprint(&file).unwrap();
            assert_eq!(fp.guarantee(), FingerprintGuarantee::Physical);
            assert_eq!(fp.size_bytes(), 4);
            // And it round-trips through its stored token.
            assert_eq!(df_domain::FileFingerprint::parse(&fp.token()).unwrap(), fp);
        }

        #[test]
        fn the_output_root_has_a_physical_identity() {
            let (_tmp, root) = root();
            assert_eq!(root.identity_level(), IdentityLevel::Physical);
            assert!(root.identity().is_some());
            root.revalidate().expect("identity is stable");
        }

        #[test]
        fn identity_distinguishes_two_directories() {
            let (_tmp, root) = root();
            let a = root.path().join("a");
            let b = root.path().join("b");
            std::fs::create_dir(&a).unwrap();
            std::fs::create_dir(&b).unwrap();
            let ia = identity_of(&a).unwrap().unwrap();
            let ib = identity_of(&b).unwrap().unwrap();
            assert_ne!(ia, ib);
        }

        #[test]
        fn a_source_junction_is_rejected_and_cannot_hide_physical_overlap() {
            let tmp = tempfile::tempdir().unwrap();
            let real = tmp.path().join("real");
            let link = tmp.path().join("link");
            std::fs::create_dir(&real).unwrap();
            if !make_junction(&link, &real) {
                eprintln!("SKIP: could not create a junction on this system");
                return;
            }

            assert!(matches!(
                ensure_root_is_not_reparse(&link).unwrap_err(),
                FsSafetyError::ReparsePoint { .. }
            ));
            assert!(matches!(
                ensure_physical_roots_disjoint(&link, &real).unwrap_err(),
                FsSafetyError::PhysicalRootOverlap { .. }
            ));
            // The output may not exist yet: resolving the deepest existing
            // ancestor must still expose that it would be created in `real`.
            assert!(matches!(
                ensure_physical_roots_disjoint(&link.join("future"), &real).unwrap_err(),
                FsSafetyError::PhysicalRootOverlap { .. }
            ));
        }

        #[test]
        fn finalize_no_replace_refuses_an_existing_destination() {
            let (_tmp, root) = root();
            let partial = root.path().join("p.tmp");
            let destination = root.path().join("d.txt");
            std::fs::write(&partial, b"new").unwrap();
            std::fs::write(&destination, b"original").unwrap();

            let err = finalize_no_replace(&partial, &destination).unwrap_err();
            assert!(
                matches!(err, FsSafetyError::DestinationExists { .. }),
                "{err}"
            );
            // The preexisting file is untouched and the partial still exists.
            assert_eq!(std::fs::read(&destination).unwrap(), b"original");
            assert!(partial.exists());
        }

        #[test]
        fn finalize_no_replace_moves_when_the_destination_is_free() {
            let (_tmp, root) = root();
            let partial = root.path().join("p.tmp");
            let destination = root.path().join("d.txt");
            std::fs::write(&partial, b"payload").unwrap();
            finalize_no_replace(&partial, &destination).unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), b"payload");
            assert!(!partial.exists());
        }

        #[test]
        fn create_partial_secure_never_reuses_an_existing_file() {
            let (_tmp, root) = root();
            let rel = SafeRelativePath::parse(Path::new("p.tmp")).unwrap();
            let _first = root.create_partial_secure(&rel).unwrap();
            // A second attempt must not truncate the first.
            assert!(root.create_partial_secure(&rel).is_err());
        }

        #[test]
        fn a_reparse_point_root_is_rejected() {
            // A junction *as* the output root is as bad as one inside it.
            let tmp = tempfile::tempdir().unwrap();
            let real = tmp.path().join("real");
            let link = tmp.path().join("link");
            std::fs::create_dir(&real).unwrap();
            if !make_junction(&link, &real) {
                eprintln!("SKIP: could not create a junction on this system");
                return;
            }
            let err = SafeOutputRoot::validate(&link).unwrap_err();
            assert!(matches!(err, FsSafetyError::ReparsePoint { .. }), "{err}");
        }

        #[test]
        fn a_junction_component_inside_the_output_is_rejected() {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tmp.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            let out_dir = tmp.path().join("out");
            std::fs::create_dir(&out_dir).unwrap();
            let root = SafeOutputRoot::validate(&out_dir).unwrap();

            // out/clientes -> outside   (the attack from the threat model)
            let planted = out_dir.join("clientes");
            if !make_junction(&planted, &outside) {
                eprintln!("SKIP: could not create a junction on this system");
                return;
            }

            let rel = SafeRelativePath::parse(Path::new("clientes/secreto.pdf")).unwrap();
            let err = root
                .resolve_destination_without_following_links(&rel)
                .unwrap_err();
            assert!(matches!(err, FsSafetyError::ReparsePoint { .. }), "{err}");

            // And creating the directory through it is refused too.
            let dir = SafeRelativePath::parse(Path::new("clientes/sub")).unwrap();
            assert!(matches!(
                root.create_directory_secure(&dir).unwrap_err(),
                FsSafetyError::ReparsePoint { .. }
            ));
            // Nothing was written outside.
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        }

        #[test]
        fn inspect_reports_components_without_following() {
            let (_tmp, root) = root();
            std::fs::create_dir(root.path().join("a")).unwrap();
            let rel = SafeRelativePath::parse(Path::new("a/b/c.txt")).unwrap();
            let info = root.inspect_path_components(&rel).unwrap();
            assert_eq!(info.len(), 3);
            assert!(info[0].exists && info[0].is_dir && !info[0].is_reparse_point);
            assert!(!info[1].exists);
            assert!(!info[2].exists);
        }

        /// Create a directory junction with the `mklink /J` shell builtin.
        /// Returns false when the environment does not allow it, so the test
        /// can skip loudly instead of passing silently.
        fn make_junction(link: &Path, target: &Path) -> bool {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            matches!(status, Ok(s) if s.success()) && link.exists()
        }
    }
}
