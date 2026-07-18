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

/// The exact path string handed to the operating system, extended-length
/// prefixed when it needs to be.
///
/// Win32 rejects paths at `MAX_PATH` (260) — 248 for directory creation —
/// unless they carry the `\\?\` verbatim prefix. The scanner and hasher
/// already extend their source paths, but the write path did not: at scale,
/// the partial file name pushed ~98% of copies of a deep corpus over the
/// limit and every one failed with `ERROR_PATH_NOT_FOUND`. The prefix is
/// applied to the **final composed string** at each syscall site — never to
/// an intermediate — because the limit is a property of what actually
/// reaches the API.
///
/// The prefix is only valid on absolute drive paths without `.`/`..`
/// (guaranteed here: roots are absolutized and relatives are validated).
/// UNC (`\\server\…`) is left untouched: it needs the `\\?\UNC\` form and
/// network roots are not a supported target yet.
pub fn extended_for_io(path: &Path) -> std::borrow::Cow<'_, Path> {
    #[cfg(windows)]
    {
        // Headroom below both limits (260 files / 248 directories).
        const THRESHOLD: usize = 240;
        let os = path.as_os_str();
        if os.len() >= THRESHOLD && path.is_absolute() {
            let text = os.to_string_lossy();
            if !text.starts_with(r"\\") {
                // Build via OsString so a non-Unicode path is not corrupted.
                let mut extended = OsString::from(r"\\?\");
                extended.push(os);
                return std::borrow::Cow::Owned(PathBuf::from(extended));
            }
        }
    }
    std::borrow::Cow::Borrowed(path)
}

const MAX_WINDOWS_COMPONENT_UTF16: usize = 255;

/// Build the deterministic `~df-<hash>` collision name within the portable
/// Windows component limit.
///
/// `file_name` is expected to be an already validated destination component.
/// The stem is shortened by UTF-16 units only when necessary; the extension is
/// preserved in full whenever it and the marker fit. A Unicode scalar is never
/// split, so supplementary characters remain valid surrogate pairs on disk.
pub fn deterministic_collision_file_name(file_name: &str, sha256: &str) -> String {
    let tag = &sha256[..8.min(sha256.len())];
    let marker = format!("~df-{tag}");
    match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => {
            let fixed_units = marker.encode_utf16().count() + 1 + extension.encode_utf16().count();
            if fixed_units <= MAX_WINDOWS_COMPONENT_UTF16 {
                let stem = truncate_to_utf16_units(stem, MAX_WINDOWS_COMPONENT_UTF16 - fixed_units);
                return format!("{stem}{marker}.{extension}");
            }
        }
        _ => {}
    }

    let file_name = truncate_to_utf16_units(
        file_name,
        MAX_WINDOWS_COMPONENT_UTF16 - marker.encode_utf16().count(),
    );
    format!("{file_name}{marker}")
}

/// Keep a UTF-8 string prefix that occupies at most `max_units` UTF-16 code
/// units. Iterating by `char` means a surrogate pair is never split.
fn truncate_to_utf16_units(value: &str, max_units: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > max_units {
                return false;
            }
            units = next;
            true
        })
        .collect()
}

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
            let metadata = match std::fs::symlink_metadata(extended_for_io(&current)) {
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

    /// Hold a strong, no-delete lease on every path component and a
    /// no-write/no-delete read handle on the final regular file.
    ///
    /// Consumers may hash the returned handle and then let another library
    /// reopen the same path for reading while this lease remains alive. On
    /// Windows the sharing modes prevent replacing or modifying the object in
    /// between those operations.
    pub fn lease_existing_file(&self, relative: &SafeRelativePath) -> FsResult<ReadLease> {
        self.revalidate()?;
        platform::lease_existing(&self.path, relative, false, false)
    }

    /// Pin a regular operational file while allowing cooperating processes to
    /// reopen it for read/write. Delete and rename remain denied. This narrow
    /// variant is intended for library lockfiles, never evidence payloads.
    pub fn lease_existing_mutable_file(&self, relative: &SafeRelativePath) -> FsResult<ReadLease> {
        self.revalidate()?;
        platform::lease_existing(&self.path, relative, false, true)
    }

    /// Hold a strong no-delete lease on an existing plain directory and all
    /// of its ancestors below the authorised output root.
    pub fn lease_existing_directory(&self, relative: &SafeRelativePath) -> FsResult<ReadLease> {
        self.revalidate()?;
        platform::lease_existing(&self.path, relative, true, false)
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
            match std::fs::symlink_metadata(extended_for_io(&current)) {
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
                    match std::fs::create_dir(extended_for_io(&current)) {
                        Ok(()) => {}
                        // Someone else created it in the meantime: accept it
                        // only if it is a plain directory.
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            let metadata = std::fs::symlink_metadata(extended_for_io(&current))
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
        // create_new: never reuse or truncate an existing partial. The partial
        // component holds two UUIDs (~92 chars), so this is the syscall
        // most likely to cross MAX_PATH: extend the exact final string.
        std::fs::OpenOptions::new()
            // Derived-artifact builders hash the exact claimed handle before
            // finalization; opening read+write avoids a path reopen/TOCTOU.
            .read(true)
            .write(true)
            .create_new(true)
            .open(extended_for_io(destination.absolute()))
            .map_err(|error| FsSafetyError::io(destination.absolute(), error))
    }

    /// Remove a partial covered by a durable executor lease.
    ///
    /// The caller must first prove ownership from its journal; a matching file
    /// name alone is never authority to delete. This boundary then revalidates
    /// the output root, refuses every reparse point and only removes a regular
    /// file. A missing leased partial is a successful no-op: the prior attempt
    /// may have crashed before creating it or after finalizing it.
    pub fn remove_leased_partial_secure(
        &self,
        partial: &SafeRelativePath,
        expected_identity: FileIdentity,
    ) -> FsResult<bool> {
        let destination = self.resolve_destination_without_following_links(partial)?;
        platform::remove_regular_file_if_identity_matches(destination.absolute(), expected_identity)
    }

    /// Atomically finalize the exact partial object claimed by the executor.
    ///
    /// The platform implementation opens the partial with rename access,
    /// validates its physical identity on that handle, and performs a
    /// no-replace rename through the same handle. There is no path-based
    /// check/rename window in which a foreign file could be substituted.
    pub fn finalize_claimed_partial_no_replace(
        &self,
        partial: &SafeRelativePath,
        destination: &SafeRelativePath,
        expected_identity: FileIdentity,
    ) -> FsResult<()> {
        let partial = self.resolve_destination_without_following_links(partial)?;
        let destination = self.resolve_destination_without_following_links(destination)?;
        platform::finalize_file_if_identity_matches(
            partial.absolute(),
            destination.absolute(),
            expected_identity,
        )
    }
}

/// A destination proven to live inside its output root.
#[derive(Debug, Clone)]
pub struct SecureDestination {
    absolute: PathBuf,
    relative: SafeRelativePath,
}

/// Live filesystem lease preventing a verified object (and its path
/// components) from being swapped while a derived artifact is consumed.
pub struct ReadLease {
    path: PathBuf,
    target: std::fs::File,
    _ancestors: Vec<std::fs::File>,
    identity: FileIdentity,
}

impl std::fmt::Debug for ReadLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadLease")
            .field("path", &self.path)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl ReadLease {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file(&self) -> &std::fs::File {
        &self.target
    }

    pub fn identity(&self) -> FileIdentity {
        self.identity
    }
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
    match std::fs::symlink_metadata(extended_for_io(path)) {
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

/// Physical identity of an already-open file.
///
/// This is the ownership primitive for executor partials: the identity is
/// captured from the exact handle returned by `create_new`, never by reopening
/// a path that another process could have replaced in between.
pub fn identity_of_open_file(
    file: &std::fs::File,
    context_path: &Path,
) -> FsResult<Option<FileIdentity>> {
    platform::identity_of_open_file(file, context_path)
}

fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    platform::metadata_is_reparse_point(metadata)
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod platform {
    use super::{FileIdentity, FsResult, FsSafetyError, ReadLease, SafeRelativePath};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, FileRenameInfo, GetFileInformationByHandle, MoveFileExW,
        SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_READ,
        FILE_SHARE_WRITE, MOVEFILE_WRITE_THROUGH,
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
            .open(super::extended_for_io(path))
    }

    fn file_information(file: &std::fs::File) -> std::io::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `file` owns a live handle for the duration of the call and
        // `info` is a properly sized, zeroed output struct.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(info)
        }
    }

    fn identity_from_information(info: &BY_HANDLE_FILE_INFORMATION) -> Option<FileIdentity> {
        let file_index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        (file_index != 0).then_some(FileIdentity {
            volume_serial: info.dwVolumeSerialNumber as u64,
            file_index,
        })
    }

    fn require_regular_identity(
        path: &Path,
        info: &BY_HANDLE_FILE_INFORMATION,
        expected: FileIdentity,
    ) -> FsResult<()> {
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(FsSafetyError::ReparsePoint {
                path: path.to_path_buf(),
            });
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "leased partial exists but is not a regular file".to_string(),
            });
        }
        if identity_from_information(info) != Some(expected) {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "leased partial physical identity no longer matches its durable claim"
                    .to_string(),
            });
        }
        Ok(())
    }

    fn open_directory_guard(path: &Path) -> FsResult<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            // Refuse FILE_SHARE_DELETE: the component cannot be renamed or
            // replaced while the handle lives. Writes inside the directory
            // remain possible for a builder holding its own unique path.
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(super::extended_for_io(path))
            .map_err(|error| FsSafetyError::io(path, error))?;
        let info = file_information(&file).map_err(|error| FsSafetyError::io(path, error))?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(FsSafetyError::ReparsePoint {
                path: path.to_path_buf(),
            });
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "leased path component is not a directory".to_string(),
            });
        }
        if identity_from_information(&info).is_none() {
            return Err(FsSafetyError::InvalidRootPath {
                path: path.to_path_buf(),
                reason: "filesystem did not provide a strong directory identity".to_string(),
            });
        }
        Ok(file)
    }

    fn open_regular_read_guard(path: &Path) -> FsResult<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            // Other readers (Tantivy/DataFusion) may reopen the path, but no
            // writer/deleter can obtain a conflicting handle until we finish.
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(super::extended_for_io(path))
            .map_err(|error| FsSafetyError::io(path, error))?;
        let info = file_information(&file).map_err(|error| FsSafetyError::io(path, error))?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(FsSafetyError::ReparsePoint {
                path: path.to_path_buf(),
            });
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "leased artifact is not a regular file".to_string(),
            });
        }
        if identity_from_information(&info).is_none() {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "filesystem did not provide a strong file identity".to_string(),
            });
        }
        Ok(file)
    }

    fn open_mutable_file_guard(path: &Path) -> FsResult<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            // Tantivy may reopen its lockfile for write; neither side may
            // delete/rename it while the artifact reader is alive.
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(super::extended_for_io(path))
            .map_err(|error| FsSafetyError::io(path, error))?;
        let info = file_information(&file).map_err(|error| FsSafetyError::io(path, error))?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(FsSafetyError::ReparsePoint {
                path: path.to_path_buf(),
            });
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "leased operational object is not a regular file".to_string(),
            });
        }
        if identity_from_information(&info).is_none() {
            return Err(FsSafetyError::InvalidRelativePath {
                path: path.to_path_buf(),
                reason: "leased operational file has no strong physical identity".to_string(),
            });
        }
        Ok(file)
    }

    pub(super) fn lease_existing(
        root: &Path,
        relative: &SafeRelativePath,
        target_is_directory: bool,
        target_is_mutable: bool,
    ) -> FsResult<ReadLease> {
        debug_assert!(!(target_is_directory && target_is_mutable));
        let mut ancestors = vec![open_directory_guard(root)?];
        let mut current = root.to_path_buf();
        let count = relative.components().len();
        let mut target = None;
        for (index, component) in relative.components().iter().enumerate() {
            current.push(component);
            let last = index + 1 == count;
            if last && !target_is_directory {
                target = Some(if target_is_mutable {
                    open_mutable_file_guard(&current)?
                } else {
                    open_regular_read_guard(&current)?
                });
            } else {
                let directory = open_directory_guard(&current)?;
                if last {
                    target = Some(directory);
                } else {
                    ancestors.push(directory);
                }
            }
        }
        let target = target.ok_or_else(|| FsSafetyError::InvalidRelativePath {
            path: relative.to_path(),
            reason: "leased path has no target component".to_string(),
        })?;
        let info = file_information(&target).map_err(|error| FsSafetyError::io(&current, error))?;
        let identity =
            identity_from_information(&info).ok_or_else(|| FsSafetyError::InvalidRelativePath {
                path: current.clone(),
                reason: "leased target has no strong physical identity".to_string(),
            })?;
        Ok(ReadLease {
            path: current,
            target,
            _ancestors: ancestors,
            identity,
        })
    }

    pub(super) fn identity_of(path: &Path) -> FsResult<Option<FileIdentity>> {
        let file = match open_for_query(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(FsSafetyError::io(path, e)),
        };
        let info = file_information(&file).map_err(|error| FsSafetyError::io(path, error))?;
        Ok(identity_from_information(&info))
    }

    pub(super) fn identity_of_open_file(
        file: &std::fs::File,
        context_path: &Path,
    ) -> FsResult<Option<FileIdentity>> {
        let info =
            file_information(file).map_err(|error| FsSafetyError::io(context_path, error))?;
        Ok(identity_from_information(&info))
    }

    pub(super) fn remove_regular_file_if_identity_matches(
        path: &Path,
        expected: FileIdentity,
    ) -> FsResult<bool> {
        // Open with DELETE access while refusing FILE_SHARE_DELETE. Once this
        // handle is acquired the named object cannot be swapped between the
        // identity check and handle-based disposition.
        let file = match std::fs::OpenOptions::new()
            .access_mode(DELETE | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(super::extended_for_io(path))
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(FsSafetyError::io(path, error)),
        };
        let info = file_information(&file).map_err(|error| FsSafetyError::io(path, error))?;
        require_regular_identity(path, &info, expected)?;

        let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
        // SAFETY: `file` is held open with DELETE access; the buffer and size
        // match FileDispositionInfo. Deletion applies to this handle's object,
        // not to a path that could have been replaced after validation.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                &disposition as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(FsSafetyError::io(path, std::io::Error::last_os_error()));
        }
        drop(file);
        Ok(true)
    }

    pub(super) fn finalize_file_if_identity_matches(
        partial: &Path,
        destination: &Path,
        expected: FileIdentity,
    ) -> FsResult<()> {
        // No FILE_SHARE_DELETE: once this handle is acquired, no other handle
        // can rename/delete the object between validation and finalization.
        let file = std::fs::OpenOptions::new()
            .access_mode(DELETE | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(super::extended_for_io(partial))
            .map_err(|error| FsSafetyError::io(partial, error))?;
        let info = file_information(&file).map_err(|error| FsSafetyError::io(partial, error))?;
        require_regular_identity(partial, &info, expected)?;

        // FILE_RENAME_INFO ends in a flexible UTF-16 array. A Vec<usize>
        // supplies sufficient alignment for the native struct and tail.
        let destination_wide: Vec<u16> = super::extended_for_io(destination)
            .as_os_str()
            .encode_wide()
            .collect();
        let file_name_offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        // FileNameLength excludes the terminator, but FILE_RENAME_INFO still
        // requires FileName itself to be NUL-terminated.
        let buffer_bytes = std::mem::size_of::<FILE_RENAME_INFO>()
            + (destination_wide.len() + 1) * std::mem::size_of::<u16>();
        debug_assert!(buffer_bytes >= file_name_offset + (destination_wide.len() + 1) * 2);
        let word_bytes = std::mem::size_of::<usize>();
        let mut storage = vec![0usize; buffer_bytes.div_ceil(word_bytes)];
        let rename = storage.as_mut_ptr() as *mut FILE_RENAME_INFO;
        // SAFETY: `storage` is aligned and large enough for the fixed fields
        // plus all UTF-16 units. ReplaceIfExists=false is the no-overwrite
        // contract enforced by the kernel.
        unsafe {
            (*rename).Anonymous.ReplaceIfExists = 0;
            (*rename).RootDirectory = std::ptr::null_mut();
            (*rename).FileNameLength = (destination_wide.len() * std::mem::size_of::<u16>()) as u32;
            std::ptr::copy_nonoverlapping(
                destination_wide.as_ptr(),
                std::ptr::addr_of_mut!((*rename).FileName) as *mut u16,
                destination_wide.len(),
            );
        }
        // SAFETY: live DELETE-capable handle and a correctly sized/aligned
        // FILE_RENAME_INFO buffer. The rename applies to the handle's object.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileRenameInfo,
                rename as *const core::ffi::c_void,
                buffer_bytes as u32,
            )
        };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_ALREADY_EXISTS) | Some(ERROR_FILE_EXISTS) => {
                    Err(FsSafetyError::DestinationExists {
                        path: destination.to_path_buf(),
                    })
                }
                _ => Err(FsSafetyError::io(destination, error)),
            };
        }
        Ok(())
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
        // MoveFileExW accepts \\?\ paths; extend the exact strings so a long
        // partial or destination does not fail at the very last step.
        let from = to_wide(&super::extended_for_io(partial));
        let to = to_wide(&super::extended_for_io(destination));
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
    use super::{FileIdentity, FsResult, FsSafetyError, ReadLease, SafeRelativePath};
    use std::path::Path;

    pub(super) fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
        metadata.file_type().is_symlink()
    }

    pub(super) fn identity_of(_path: &Path) -> FsResult<Option<FileIdentity>> {
        Ok(None)
    }

    pub(super) fn identity_of_open_file(
        _file: &std::fs::File,
        _context_path: &Path,
    ) -> FsResult<Option<FileIdentity>> {
        Ok(None)
    }

    pub(super) fn lease_existing(
        _root: &Path,
        _relative: &SafeRelativePath,
        _target_is_directory: bool,
        _target_is_mutable: bool,
    ) -> FsResult<ReadLease> {
        Err(FsSafetyError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
    }

    pub(super) fn remove_regular_file_if_identity_matches(
        _path: &Path,
        _expected: FileIdentity,
    ) -> FsResult<bool> {
        Err(FsSafetyError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
    }

    pub(super) fn finalize_file_if_identity_matches(
        _partial: &Path,
        _destination: &Path,
        _expected: FileIdentity,
    ) -> FsResult<()> {
        Err(FsSafetyError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
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

    #[cfg(windows)]
    #[test]
    fn read_lease_blocks_file_mutation_and_path_replacement() {
        use std::io::Read;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("artifact.bin");
        std::fs::write(&path, b"trusted").unwrap();
        let root = SafeOutputRoot::validate(temp.path()).unwrap();
        let relative = SafeRelativePath::parse(Path::new("artifact.bin")).unwrap();
        let lease = root.lease_existing_file(&relative).unwrap();
        let mut reader = lease.file().try_clone().unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"trusted");
        drop(reader);
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::rename(&path, temp.path().join("swapped.bin")).is_err());

        drop(lease);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn mutable_lease_allows_lockfile_writes_but_blocks_replacement() {
        use std::io::Write;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.lock");
        std::fs::write(&path, b"").unwrap();
        let root = SafeOutputRoot::validate(temp.path()).unwrap();
        let relative = SafeRelativePath::parse(Path::new("library.lock")).unwrap();
        let lease = root.lease_existing_mutable_file(&relative).unwrap();
        let mut cooperating = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        cooperating.write_all(b"locked").unwrap();
        drop(cooperating);
        assert!(std::fs::rename(&path, temp.path().join("swapped.lock")).is_err());
        drop(lease);
        std::fs::rename(&path, temp.path().join("swapped.lock")).unwrap();
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
    fn collision_names_fit_utf16_without_splitting_unicode() {
        let original = format!("{}documento.txt", "📄".repeat(121));
        assert_eq!(original.encode_utf16().count(), 255);
        let collision = deterministic_collision_file_name(&original, "abcdef1234567890");
        assert!(collision.encode_utf16().count() <= 255, "{collision}");
        assert!(collision.ends_with("~df-abcdef12.txt"), "{collision}");
        assert!(SafeRelativePath::parse(Path::new(&collision)).is_ok());
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

        fn create_claimed_partial(
            root: &SafeOutputRoot,
            relative: &SafeRelativePath,
            bytes: &[u8],
        ) -> FileIdentity {
            use std::io::Write as _;

            let mut handle = root.create_partial_secure(relative).unwrap();
            let absolute = root
                .resolve_destination_without_following_links(relative)
                .unwrap()
                .absolute()
                .to_path_buf();
            let identity = identity_of_open_file(&handle, &absolute)
                .unwrap()
                .expect("NTFS must expose a physical identity");
            handle.write_all(bytes).unwrap();
            handle.sync_all().unwrap();
            drop(handle);
            identity
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
        fn identity_is_captured_from_the_exact_open_handle() {
            let (_tmp, root) = root();
            let path = root.path().join("created.tmp");
            let handle = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            let from_handle = identity_of_open_file(&handle, &path).unwrap().unwrap();
            assert_eq!(identity_of(&path).unwrap(), Some(from_handle));
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
        fn claimed_finalize_moves_the_same_identity_when_destination_is_free() {
            let (_tmp, root) = root();
            let partial = SafeRelativePath::parse(Path::new("claimed.tmp")).unwrap();
            let destination = SafeRelativePath::parse(Path::new("final.txt")).unwrap();
            let identity = create_claimed_partial(&root, &partial, b"payload");

            root.finalize_claimed_partial_no_replace(&partial, &destination, identity)
                .unwrap();

            assert!(!root.path().join("claimed.tmp").exists());
            assert_eq!(
                std::fs::read(root.path().join("final.txt")).unwrap(),
                b"payload"
            );
            assert_eq!(
                identity_of(&root.path().join("final.txt")).unwrap(),
                Some(identity)
            );
        }

        #[test]
        fn claimed_finalize_never_replaces_an_existing_destination() {
            let (_tmp, root) = root();
            let partial = SafeRelativePath::parse(Path::new("claimed.tmp")).unwrap();
            let destination = SafeRelativePath::parse(Path::new("final.txt")).unwrap();
            let identity = create_claimed_partial(&root, &partial, b"new bytes");
            std::fs::write(root.path().join("final.txt"), b"foreign destination").unwrap();

            let error = root
                .finalize_claimed_partial_no_replace(&partial, &destination, identity)
                .unwrap_err();

            assert!(
                matches!(error, FsSafetyError::DestinationExists { .. }),
                "{error}"
            );
            assert_eq!(
                std::fs::read(root.path().join("final.txt")).unwrap(),
                b"foreign destination"
            );
            assert_eq!(
                std::fs::read(root.path().join("claimed.tmp")).unwrap(),
                b"new bytes"
            );
        }

        #[test]
        fn claimed_finalize_never_moves_a_substituted_partial() {
            let (_tmp, root) = root();
            let partial = SafeRelativePath::parse(Path::new("claimed.tmp")).unwrap();
            let destination = SafeRelativePath::parse(Path::new("final.txt")).unwrap();
            let claimed = create_claimed_partial(&root, &partial, b"our partial");
            std::fs::remove_file(root.path().join("claimed.tmp")).unwrap();
            std::fs::write(root.path().join("claimed.tmp"), b"foreign replacement").unwrap();

            let error = root
                .finalize_claimed_partial_no_replace(&partial, &destination, claimed)
                .unwrap_err();

            assert!(
                matches!(error, FsSafetyError::InvalidRelativePath { .. }),
                "{error}"
            );
            assert_eq!(
                std::fs::read(root.path().join("claimed.tmp")).unwrap(),
                b"foreign replacement"
            );
            assert!(!root.path().join("final.txt").exists());
        }

        #[test]
        fn claimed_finalize_rejects_a_directory() {
            let (_tmp, root) = root();
            let partial = SafeRelativePath::parse(Path::new("claimed.tmp")).unwrap();
            let destination = SafeRelativePath::parse(Path::new("final.txt")).unwrap();
            std::fs::create_dir(root.path().join("claimed.tmp")).unwrap();

            let error = root
                .finalize_claimed_partial_no_replace(
                    &partial,
                    &destination,
                    root.identity().unwrap(),
                )
                .unwrap_err();

            assert!(root.path().join("claimed.tmp").is_dir());
            assert!(!root.path().join("final.txt").exists());
            assert!(matches!(
                error,
                FsSafetyError::InvalidRelativePath { .. } | FsSafetyError::Io { .. }
            ));
        }

        #[test]
        fn claimed_finalize_rejects_a_reparse_point() {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tmp.path().join("outside");
            let output = tmp.path().join("output");
            std::fs::create_dir(&outside).unwrap();
            std::fs::create_dir(&output).unwrap();
            let root = SafeOutputRoot::validate(&output).unwrap();
            let planted = output.join("claimed.tmp");
            if !make_junction(&planted, &outside) {
                eprintln!("SKIP: could not create a junction on this system");
                return;
            }
            let partial = SafeRelativePath::parse(Path::new("claimed.tmp")).unwrap();
            let destination = SafeRelativePath::parse(Path::new("final.txt")).unwrap();

            let error = root
                .finalize_claimed_partial_no_replace(
                    &partial,
                    &destination,
                    root.identity().unwrap(),
                )
                .unwrap_err();

            assert!(matches!(error, FsSafetyError::ReparsePoint { .. }));
            assert!(metadata_is_reparse_point(
                &std::fs::symlink_metadata(&planted).unwrap()
            ));
            assert!(!output.join("final.txt").exists());
        }

        #[test]
        fn claimed_finalize_supports_extended_length_paths() {
            let (_tmp, root) = root();
            let deep = ["a".repeat(90), "b".repeat(90), "c".repeat(90)].join("/");
            let directory = SafeRelativePath::parse(Path::new(&deep)).unwrap();
            root.create_directory_secure(&directory).unwrap();
            let partial =
                SafeRelativePath::parse(Path::new(&format!("{deep}/claimed.tmp"))).unwrap();
            let destination =
                SafeRelativePath::parse(Path::new(&format!("{deep}/final.txt"))).unwrap();
            let identity = create_claimed_partial(&root, &partial, b"long payload");

            root.finalize_claimed_partial_no_replace(&partial, &destination, identity)
                .unwrap();

            assert_eq!(
                std::fs::read(extended_for_io(&root.path().join(destination.to_path()))).unwrap(),
                b"long payload"
            );
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
        fn a_leased_regular_partial_can_be_reclaimed_but_a_directory_cannot() {
            let (_tmp, root) = root();
            let file = SafeRelativePath::parse(Path::new("leased.tmp")).unwrap();
            std::fs::write(root.path().join("leased.tmp"), b"owned partial").unwrap();
            let identity = identity_of(&root.path().join("leased.tmp"))
                .unwrap()
                .unwrap();
            assert!(root.remove_leased_partial_secure(&file, identity).unwrap());
            assert!(!root.path().join("leased.tmp").exists());
            assert!(!root.remove_leased_partial_secure(&file, identity).unwrap());

            let directory = SafeRelativePath::parse(Path::new("not-a-file.tmp")).unwrap();
            std::fs::create_dir(root.path().join("not-a-file.tmp")).unwrap();
            let error = root
                .remove_leased_partial_secure(&directory, identity)
                .unwrap_err();
            assert!(matches!(error, FsSafetyError::InvalidRelativePath { .. }));
            assert!(root.path().join("not-a-file.tmp").is_dir());
        }

        #[test]
        fn a_replaced_leased_partial_is_never_removed() {
            let (_tmp, root) = root();
            let relative = SafeRelativePath::parse(Path::new("leased.tmp")).unwrap();
            let path = root.path().join("leased.tmp");
            std::fs::write(&path, b"our old partial").unwrap();
            let claimed = identity_of(&path).unwrap().unwrap();

            std::fs::remove_file(&path).unwrap();
            std::fs::write(&path, b"foreign replacement").unwrap();
            let replacement = identity_of(&path).unwrap().unwrap();
            assert_ne!(claimed, replacement, "the test must replace the object");

            let error = root
                .remove_leased_partial_secure(&relative, claimed)
                .unwrap_err();
            assert!(matches!(error, FsSafetyError::InvalidRelativePath { .. }));
            assert_eq!(std::fs::read(&path).unwrap(), b"foreign replacement");
        }

        #[test]
        fn a_leased_partial_reparse_point_is_never_removed_or_followed() {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tmp.path().join("outside");
            let output = tmp.path().join("output");
            std::fs::create_dir(&outside).unwrap();
            std::fs::create_dir(&output).unwrap();
            let root = SafeOutputRoot::validate(&output).unwrap();
            let planted = output.join("leased.tmp");
            if !make_junction(&planted, &outside) {
                eprintln!("SKIP: could not create a junction on this system");
                return;
            }

            let partial = SafeRelativePath::parse(Path::new("leased.tmp")).unwrap();
            let error = root
                .remove_leased_partial_secure(&partial, root.identity().unwrap())
                .unwrap_err();
            assert!(matches!(error, FsSafetyError::ReparsePoint { .. }));
            assert!(metadata_is_reparse_point(
                &std::fs::symlink_metadata(&planted).unwrap()
            ));
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
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
