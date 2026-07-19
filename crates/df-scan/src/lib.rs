//! Safe inventory scanner (RFC-0001 §12.1, §12.2, §13).
//!
//! Guarantees:
//! - the origin is only ever read (rule 1); nothing is created, modified or
//!   deleted inside a source root;
//! - reparse points (symlinks, junctions) are recorded, never followed;
//! - traversal is an iterative queue, so depth cannot overflow the stack;
//! - entries are persisted in bounded transactional batches;
//! - per-entry errors are recorded and never abort the scan;
//! - every phase change goes through the project state machine and lands in
//!   the audit ledger.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use df_db::inventory::{self, ScanBatch};
use df_db::{repository, Db};
use df_domain::{
    Actor, FileFingerprint, FolderId, FolderRecord, OccurrenceId, PathOccurrence, ProjectState,
    ScanCounters, ScanEntryStatus, ScanRun, ScanRunStatus, SnapshotId, SourceRoot, Timestamp,
};
use df_error::{DfError, DfResult};
use serde::Serialize;
use sha2::Digest;

/// Tuning knobs of one scan (RFC-0001 §13.3).
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Entries accumulated before a transactional flush (1 000–10 000).
    pub batch_entries: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            batch_entries: 1_000,
        }
    }
}

/// Result of a finished (or cancelled) scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScanOutcome {
    pub snapshot_id: String,
    pub scan_run_id: String,
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub errors: u64,
    pub reparse_points: u64,
    pub cancelled: bool,
    /// Project state after the scan (`SCANNED`, or `SCAN_PAUSED` when
    /// cancelled).
    pub state: String,
}

/// Validate a project's roots and move it `CREATED → VALIDATING → READY`
/// (RFC-0001 §12.1).
///
/// On a validation failure the project intentionally stays in `VALIDATING`:
/// `FAILED` is terminal, and a missing network drive must not kill the
/// project. Fixing the environment and validating again reaches `READY`.
pub fn validate_project(db: &mut Db, actor: Actor) -> DfResult<()> {
    let project = repository::load_project(db)?;
    match project.state {
        ProjectState::Created => {
            repository::update_project_state(db, ProjectState::Validating, actor)?;
        }
        ProjectState::Validating => {}
        other => {
            return Err(DfError::Validation(format!(
                "cannot validate a project in state {other}"
            )));
        }
    }

    let roots = repository::load_source_roots(db, project.id)?;
    if roots.is_empty() {
        return Err(DfError::Validation(
            "the project has no source roots; create it with at least one --source".to_string(),
        ));
    }
    for root in &roots {
        let path = &root.absolute_path;
        // `read_dir` follows the starting directory. Reject a junction root
        // before the walker can cross it, then prove it is physically separate
        // from every configured write root.
        df_fs_safety::ensure_root_is_not_reparse(path)?;
        if !path.is_dir() {
            return Err(DfError::Validation(format!(
                "source root `{}` does not exist or is not a directory",
                path.display()
            )));
        }
        // Readability probe: listing the root must work.
        std::fs::read_dir(path).map_err(|e| DfError::io(path.clone(), e))?;
        df_fs_safety::ensure_physical_roots_disjoint(path, &project.output_root)?;
        df_fs_safety::ensure_physical_roots_disjoint(path, &project.audit_root)?;
    }
    for (i, a) in roots.iter().enumerate() {
        for b in roots.iter().skip(i + 1) {
            if paths_overlap(&a.absolute_path, &b.absolute_path) {
                return Err(DfError::Validation(format!(
                    "source roots `{}` and `{}` overlap",
                    a.absolute_path.display(),
                    b.absolute_path.display()
                )));
            }
            df_fs_safety::ensure_physical_roots_disjoint(&a.absolute_path, &b.absolute_path)?;
        }
    }

    repository::update_project_state(db, ProjectState::Ready, actor)?;
    Ok(())
}

/// Run a full scan: validate if needed, snapshot every source root and move
/// the project to `SCANNED` (or `SCAN_PAUSED` when `cancel` fires).
pub fn scan_project(
    db: &mut Db,
    actor: Actor,
    options: &ScanOptions,
    cancel: Option<&AtomicBool>,
) -> DfResult<ScanOutcome> {
    if options.batch_entries == 0 {
        return Err(DfError::Validation(
            "batch_entries must be at least 1".to_string(),
        ));
    }

    let project = repository::load_project(db)?;
    match project.state {
        ProjectState::Created | ProjectState::Validating => validate_project(db, actor)?,
        ProjectState::Ready | ProjectState::ScanPaused => {}
        // M0.8: incremental rescans start a fresh snapshot from any stable
        // checkpoint without a plan in flight (ADR-0035).
        ProjectState::Hashed
        | ProjectState::Analyzed
        | ProjectState::Completed
        | ProjectState::CompletedWithWarnings => {}
        other => {
            return Err(DfError::Validation(format!(
                "cannot scan a project in state {other} \
                 (expected CREATED, VALIDATING, READY, SCAN_PAUSED or a \
                 stable checkpoint: HASHED, ANALYZED, COMPLETED)"
            )));
        }
    }

    let roots = repository::load_source_roots(db, project.id)?;
    repository::update_project_state(db, ProjectState::Scanning, actor)?;
    let (snapshot, run) = inventory::start_scan(db, project.id, actor)?;

    let mut walker = Walker {
        db,
        snapshot_id: snapshot.id,
        run: &run,
        options,
        cancel,
        batch: ScanBatch::default(),
        counters: ScanCounters::default(),
    };

    let mut cancelled = false;
    for root in &roots {
        match walker.walk_root(root) {
            Ok(WalkVerdict::Finished) => {}
            Ok(WalkVerdict::Cancelled) => {
                cancelled = true;
                break;
            }
            Err(error) => {
                // Infrastructure failure (e.g. the database rejected a
                // batch): close the run as FAILED and surface the error.
                let counters = walker.counters;
                let _ = inventory::finish_scan(db, &run, ScanRunStatus::Failed, counters, actor);
                let _ = repository::update_project_state(db, ProjectState::Failed, actor);
                return Err(error);
            }
        }
    }
    walker.flush()?;
    let counters = walker.counters;

    let (run_status, next_state) = if cancelled {
        (ScanRunStatus::Cancelled, ProjectState::ScanPaused)
    } else {
        (ScanRunStatus::Completed, ProjectState::Scanned)
    };
    inventory::finish_scan(db, &run, run_status, counters, actor)?;
    let project = repository::update_project_state(db, next_state, actor)?;

    Ok(ScanOutcome {
        snapshot_id: snapshot.id.to_string(),
        scan_run_id: run.id.to_string(),
        files: counters.files,
        folders: counters.folders,
        bytes: counters.bytes,
        errors: counters.errors,
        reparse_points: counters.reparse_points,
        cancelled,
        state: project.state.as_str().to_string(),
    })
}

enum WalkVerdict {
    Finished,
    Cancelled,
}

/// A directory waiting to be read. Its folder row is written when it is
/// popped, so the row's status reflects whether reading it worked.
struct QueuedDir {
    relative_path: String,
    /// Authoritative path used to reopen the directory. The display path is
    /// only a fallback for legacy/exceptional entries where capture failed.
    raw_relative_path: Option<df_domain::RawPath>,
    parent_relative_path: Option<String>,
    name: String,
    depth: u32,
}

struct Walker<'a> {
    db: &'a mut Db,
    snapshot_id: SnapshotId,
    run: &'a ScanRun,
    options: &'a ScanOptions,
    cancel: Option<&'a AtomicBool>,
    batch: ScanBatch,
    counters: ScanCounters,
}

impl Walker<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    fn flush(&mut self) -> DfResult<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.batch);
        inventory::insert_scan_batch(self.db, self.run.id, &batch, self.counters)
    }

    fn flush_if_full(&mut self) -> DfResult<()> {
        if self.batch.len() >= self.options.batch_entries {
            self.flush()?;
        }
        Ok(())
    }

    /// Iterative breadth-first walk of one source root (RFC-0001 §13.2).
    fn walk_root(&mut self, root: &SourceRoot) -> DfResult<WalkVerdict> {
        let root_name = root
            .absolute_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.absolute_path.display().to_string());

        let mut queue: VecDeque<QueuedDir> = VecDeque::new();
        queue.push_back(QueuedDir {
            relative_path: String::new(),
            raw_relative_path: Some(df_domain::RawPath::from_os_str(std::ffi::OsStr::new(""))),
            parent_relative_path: None,
            name: root_name,
            depth: 0,
        });

        while let Some(dir) = queue.pop_front() {
            if self.cancelled() {
                self.flush()?;
                return Ok(WalkVerdict::Cancelled);
            }
            let dir_abs = dir
                .raw_relative_path
                .as_ref()
                .map(|raw| root.absolute_path.join(PathBuf::from(raw.to_os_string())))
                .unwrap_or_else(|| compose_path(&root.absolute_path, &dir.relative_path));
            let entries = match std::fs::read_dir(&dir_abs) {
                Ok(entries) => entries,
                Err(error) => {
                    self.counters.errors += 1;
                    self.push_folder(root, &dir, ScanEntryStatus::Error, Some(error.to_string()));
                    self.flush_if_full()?;
                    continue;
                }
            };
            self.counters.folders += 1;
            self.push_folder(root, &dir, ScanEntryStatus::Ok, None);

            let entries: Vec<_> = entries.collect();
            let entry_names: Vec<_> = entries
                .iter()
                .filter_map(|entry| entry.as_ref().ok())
                .map(|entry| {
                    let os_name = entry.file_name();
                    let raw = df_domain::RawPath::from_os_str(&os_name);
                    let lossy = os_name.to_str().is_none();
                    (raw, os_name.to_string_lossy().into_owned(), lossy)
                })
                .collect();
            let relative_components = collision_safe_entry_components(&entry_names);

            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        self.counters.errors += 1;
                        self.push_unreadable_entry(root, &dir, error.to_string());
                        self.flush_if_full()?;
                        continue;
                    }
                };
                let raw_name = df_domain::RawPath::from_os_str(&entry.file_name());
                let relative_component =
                    relative_components.get(&raw_name).cloned().ok_or_else(|| {
                        DfError::Database(
                            "scanner lost a deterministic entry-name mapping".to_string(),
                        )
                    })?;
                self.process_entry(root, &dir, &entry, relative_component, &mut queue);
                self.flush_if_full()?;
            }
        }
        Ok(WalkVerdict::Finished)
    }

    fn process_entry(
        &mut self,
        root: &SourceRoot,
        dir: &QueuedDir,
        entry: &std::fs::DirEntry,
        relative_component: String,
        queue: &mut VecDeque<QueuedDir>,
    ) {
        let raw_name = entry.file_name();
        let name_is_lossy = raw_name.to_str().is_none();
        let name = raw_name.to_string_lossy().into_owned();
        let rel = join_relative(&dir.relative_path, &relative_component);
        let raw_relative_path = entry
            .path()
            .strip_prefix(&root.absolute_path)
            .ok()
            .map(|path| df_domain::RawPath::from_os_str(path.as_os_str()));
        let child_depth = dir.depth + 1;

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                self.counters.errors += 1;
                self.push_occurrence(
                    root,
                    rel,
                    raw_relative_path,
                    &dir.relative_path,
                    name,
                    None,
                    child_depth,
                    ScanEntryStatus::Error,
                    Some(error.to_string()),
                    name_is_lossy,
                );
                return;
            }
        };

        if is_reparse_point(&metadata) {
            // Recorded, never followed (RFC-0001 §13.6).
            self.counters.reparse_points += 1;
            if metadata.is_dir() {
                self.batch.folders.push(FolderRecord {
                    id: FolderId::new(),
                    snapshot_id: self.snapshot_id,
                    source_root_id: root.id,
                    relative_path: rel,
                    raw_relative_path,
                    parent_relative_path: Some(dir.relative_path.clone()),
                    normalized_name: name.to_lowercase(),
                    name,
                    depth: child_depth,
                    status: ScanEntryStatus::ReparseNotFollowed,
                    error: None,
                });
            } else {
                self.push_occurrence(
                    root,
                    rel,
                    raw_relative_path,
                    &dir.relative_path,
                    name,
                    Some(&metadata),
                    child_depth,
                    ScanEntryStatus::ReparseNotFollowed,
                    None,
                    name_is_lossy,
                );
            }
            return;
        }

        if metadata.is_dir() {
            queue.push_back(QueuedDir {
                relative_path: rel,
                raw_relative_path,
                parent_relative_path: Some(dir.relative_path.clone()),
                name,
                depth: child_depth,
            });
        } else {
            self.counters.files += 1;
            self.counters.bytes += metadata.len();
            self.push_occurrence(
                root,
                rel,
                raw_relative_path,
                &dir.relative_path,
                name,
                Some(&metadata),
                child_depth,
                ScanEntryStatus::Ok,
                None,
                name_is_lossy,
            );
        }
    }

    fn push_folder(
        &mut self,
        root: &SourceRoot,
        dir: &QueuedDir,
        status: ScanEntryStatus,
        error: Option<String>,
    ) {
        self.batch.folders.push(FolderRecord {
            id: FolderId::new(),
            snapshot_id: self.snapshot_id,
            source_root_id: root.id,
            relative_path: dir.relative_path.clone(),
            raw_relative_path: dir.raw_relative_path.clone(),
            parent_relative_path: dir.parent_relative_path.clone(),
            name: dir.name.clone(),
            normalized_name: dir.name.to_lowercase(),
            depth: dir.depth,
            status,
            error,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_occurrence(
        &mut self,
        root: &SourceRoot,
        relative_path: String,
        raw_relative_path: Option<df_domain::RawPath>,
        parent: &str,
        name: String,
        metadata: Option<&std::fs::Metadata>,
        depth: u32,
        status: ScanEntryStatus,
        error: Option<String>,
        name_is_lossy: bool,
    ) {
        let size_bytes = metadata.map(|m| m.len()).unwrap_or(0);
        let created_at_fs = metadata.and_then(|m| m.created().ok()).map(to_timestamp);
        let modified_at_fs = metadata.and_then(|m| m.modified().ok()).map(to_timestamp);
        // v2 fingerprint with physical identity when the filesystem offers
        // one (ADR-0019). A stat failure degrades to size+mtime rather than
        // aborting the scan: a partial record beats no record.
        let absolute = raw_relative_path
            .as_ref()
            .map(|raw| root.absolute_path.join(PathBuf::from(raw.to_os_string())))
            .unwrap_or_else(|| compose_path(&root.absolute_path, &relative_path));
        let fingerprint = df_fs_safety::capture_fingerprint(&absolute)
            .map(|fp| fp.token())
            .unwrap_or_else(|_| {
                FileFingerprint::V2(df_domain::FingerprintV2 {
                    size_bytes,
                    modified_at_ms: modified_at_fs.map(|t: Timestamp| t.timestamp_millis()),
                    change_time_ms: None,
                    attributes: 0,
                    identity: None,
                })
                .token()
            });
        let extension = Path::new(&name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase());

        self.batch.occurrences.push(PathOccurrence {
            id: OccurrenceId::new(),
            snapshot_id: self.snapshot_id,
            source_root_id: root.id,
            relative_path,
            raw_relative_path,
            parent_relative_path: parent.to_string(),
            normalized_name: name.to_lowercase(),
            file_name: name,
            extension,
            size_bytes,
            created_at_fs,
            modified_at_fs,
            attributes: file_attributes(metadata),
            path_length: os_str_utf16_len(absolute.as_os_str()),
            depth,
            fingerprint,
            scan_status: status,
            error,
            name_is_lossy,
        });
    }

    /// A `DirEntry` iteration error: the child's name is unknown, only the
    /// parent directory is.
    fn push_unreadable_entry(&mut self, root: &SourceRoot, dir: &QueuedDir, error: String) {
        self.push_occurrence(
            root,
            join_relative(&dir.relative_path, "<unreadable>"),
            None,
            &dir.relative_path,
            "<unreadable>".to_string(),
            None,
            dir.depth + 1,
            ScanEntryStatus::Error,
            Some(error),
            false,
        );
    }
}

/// Join a relative path and a child name using the platform separator.
fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}{}{name}", std::path::MAIN_SEPARATOR)
    }
}

/// A Windows-safe, collision-resistant display component for an exact raw
/// name that cannot be represented as Unicode. Kept below 240 UTF-16 units so
/// the planner can validate it before execution. The 128-bit raw-name digest
/// distinguishes names that share the same U+FFFD rendering.
fn collision_safe_lossy_component(display: &str, raw_name: &df_domain::RawPath) -> String {
    let digest = hex::encode(sha2::Sha256::digest(raw_name.to_blob()));
    let tag = &digest[..32];
    let (stem, extension) = display
        .rfind('.')
        .filter(|index| *index > 0)
        .map(|index| (&display[..index], Some(&display[index + 1..])))
        .unwrap_or((display, None));
    let take_utf16 = |text: &str, limit: usize| {
        let mut units = 0usize;
        text.chars()
            .take_while(|character| {
                let next = units + character.len_utf16();
                if next <= limit {
                    units = next;
                    true
                } else {
                    false
                }
            })
            .collect::<String>()
    };
    let stem = take_utf16(stem, 140);
    let extension = extension.map(|value| take_utf16(value, 32));
    match extension.as_deref().filter(|value| !value.is_empty()) {
        Some(extension) => format!("{stem}~df-raw-{tag}.{extension}"),
        None => format!("{stem}~df-raw-{tag}"),
    }
}

/// Resolve the synthetic display keys for all siblings as one deterministic
/// set. A valid Unicode filename is always kept verbatim; if it happens to be
/// equal to a lossy name's hash rendering, the lossy key receives a numbered
/// suffix. This closes the UNIQUE(relative_path) collision without ever using
/// display text to reopen the source.
fn collision_safe_entry_components(
    names: &[(df_domain::RawPath, String, bool)],
) -> HashMap<df_domain::RawPath, String> {
    let mut mapped = HashMap::new();
    let mut occupied = HashSet::new();
    for (raw, display, lossy) in names {
        if !lossy {
            occupied.insert(display.clone());
            mapped.insert(raw.clone(), display.clone());
        }
    }

    let mut lossy: Vec<_> = names.iter().filter(|(_, _, is_lossy)| *is_lossy).collect();
    lossy.sort_by_key(|(raw, _, _)| raw.to_blob());
    for (raw, display, _) in lossy {
        let base = collision_safe_lossy_component(display, raw);
        let mut candidate = base.clone();
        let mut ordinal = 1_u64;
        while !occupied.insert(candidate.clone()) {
            let suffix = format!("~{ordinal}");
            candidate = match base.rfind('.').filter(|index| *index > 0) {
                Some(index) => format!("{}{}{}", &base[..index], suffix, &base[index..]),
                None => format!("{base}{suffix}"),
            };
            ordinal += 1;
        }
        mapped.insert(raw.clone(), candidate);
    }
    mapped
}

/// Absolute path of an entry, extended-length prefixed on Windows when it
/// exceeds the legacy `MAX_PATH` limit (RFC-0001 §13.1 long paths).
fn compose_path(root: &Path, relative: &str) -> PathBuf {
    let joined = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    extend_long_path(joined)
}

#[cfg(windows)]
fn extend_long_path(path: PathBuf) -> PathBuf {
    const LEGACY_MAX_PATH: usize = 260;
    let text = path.as_os_str().to_string_lossy();
    // Drive-letter paths only; UNC paths would need the `\\?\UNC\` form and
    // verbatim network scanning is deferred until network roots land.
    if text.len() >= LEGACY_MAX_PATH && !text.starts_with(r"\\") {
        PathBuf::from(format!(r"\\?\{text}"))
    } else {
        path
    }
}

#[cfg(not(windows))]
fn extend_long_path(path: PathBuf) -> PathBuf {
    path
}

/// Reparse points must be recorded but never followed (RFC-0001 §13.6).
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

#[cfg(windows)]
fn file_attributes(metadata: Option<&std::fs::Metadata>) -> u32 {
    use std::os::windows::fs::MetadataExt;
    metadata.map(|m| m.file_attributes()).unwrap_or(0)
}

#[cfg(not(windows))]
fn file_attributes(_metadata: Option<&std::fs::Metadata>) -> u32 {
    0
}

fn to_timestamp(time: std::time::SystemTime) -> Timestamp {
    time.into()
}

/// UTF-16 length: the unit Windows path limits are expressed in.
#[cfg(windows)]
fn os_str_utf16_len(value: &std::ffi::OsStr) -> u32 {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().count() as u32
}

#[cfg(not(windows))]
fn os_str_utf16_len(value: &std::ffi::OsStr) -> u32 {
    value.to_string_lossy().encode_utf16().count() as u32
}

/// Lexical overlap check between two absolute roots (case-insensitive,
/// matching the comparison policy of `df-facade`).
fn paths_overlap(a: &Path, b: &Path) -> bool {
    let components = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let a = components(a);
    let b = components(b);
    let shorter = a.len().min(b.len());
    a[..shorter] == b[..shorter]
}

#[cfg(test)]
mod tests {
    use df_db::inventory::{inventory_summary, list_folders, list_occurrences};
    use df_domain::ProfileRef;

    use super::*;

    /// Create a project on disk with one populated source root and return
    /// (db, origin path).
    fn project_with_origin(tmp: &Path) -> (Db, PathBuf) {
        let origin = tmp.join("origen");
        std::fs::create_dir_all(origin.join("casos").join("2020")).unwrap();
        std::fs::create_dir_all(origin.join("vacía")).unwrap();
        std::fs::write(origin.join("raíz.txt"), b"root file").unwrap();
        std::fs::write(origin.join("casos").join("demanda.pdf"), b"pdf bytes").unwrap();
        std::fs::write(
            origin.join("casos").join("2020").join("acta \u{00f1}.docx"),
            b"doc",
        )
        .unwrap();

        let db_path = tmp.join("state.sqlite");
        let mut db = Db::open(&db_path).unwrap();
        let project = df_domain::Project::new(
            "Prueba scan",
            ProfileRef::default(),
            tmp.join("salida"),
            tmp.join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, origin.clone())];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        (db, origin)
    }

    #[cfg(windows)]
    fn make_junction(link: &Path, target: &Path) -> bool {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        matches!(status, Ok(status) if status.success()) && link.exists()
    }

    fn snapshot_of(outcome: &ScanOutcome) -> SnapshotId {
        outcome.snapshot_id.parse().unwrap()
    }

    fn raw_path_from_units(units: &[u16]) -> df_domain::RawPath {
        let blob: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        df_domain::RawPath::from_blob(&blob).unwrap()
    }

    #[test]
    fn lossy_display_components_keep_distinct_raw_names_distinct() {
        let first = raw_path_from_units(&[
            b'a' as u16,
            0xD800,
            b'b' as u16,
            b'.' as u16,
            b't' as u16,
            b'x' as u16,
            b't' as u16,
        ]);
        let second = raw_path_from_units(&[
            b'a' as u16,
            0xD801,
            b'b' as u16,
            b'.' as u16,
            b't' as u16,
            b'x' as u16,
            b't' as u16,
        ]);
        assert!(first.is_lossy());
        assert!(second.is_lossy());
        assert_eq!(first.display(), second.display());

        let first_key = collision_safe_lossy_component(&first.display(), &first);
        let second_key = collision_safe_lossy_component(&second.display(), &second);
        assert_ne!(first_key, second_key);
        assert!(first_key.ends_with(".txt"));
        assert!(second_key.ends_with(".txt"));
        assert!(first_key.encode_utf16().count() < 240);
        assert!(second_key.encode_utf16().count() < 240);

        let literal = df_domain::RawPath::from_os_str(std::ffi::OsStr::new(&first_key));
        let mapped = collision_safe_entry_components(&[
            (first.clone(), first.display(), true),
            (second.clone(), second.display(), true),
            (literal.clone(), first_key.clone(), false),
        ]);
        assert_eq!(mapped.get(&literal), Some(&first_key));
        assert_ne!(mapped.get(&first), Some(&first_key));
        assert_eq!(mapped.values().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn scan_inventories_files_and_folders_without_touching_the_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, origin) = project_with_origin(tmp.path());

        let before = walk_all(&origin);
        let outcome = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let after = walk_all(&origin);

        assert_eq!(outcome.files, 3);
        // origin root + casos + 2020 + vacía
        assert_eq!(outcome.folders, 4);
        assert_eq!(outcome.errors, 0);
        assert!(!outcome.cancelled);
        assert_eq!(outcome.state, "SCANNED");
        assert_eq!(before, after, "the origin must not change (rule 1)");

        let summary = inventory_summary(&db, snapshot_of(&outcome)).unwrap();
        assert_eq!(summary.files, 3);
        assert_eq!(summary.folders, 4);
        assert_eq!(summary.bytes, outcome.bytes);
    }

    #[cfg(windows)]
    #[test]
    fn validation_rejects_a_source_root_junction_before_walking_it() {
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("datos");
        let source_alias = tmp.path().join("alias");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("origen.txt"), b"source bytes").unwrap();
        if !make_junction(&source_alias, &output) {
            eprintln!("SKIP: this environment cannot create junctions (mklink /J failed)");
            return;
        }

        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = df_domain::Project::new(
            "Alias source",
            ProfileRef::default(),
            output.clone(),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, source_alias)];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();

        let error = validate_project(&mut db, Actor::Test).unwrap_err();
        assert!(
            matches!(&error, DfError::Validation(message) if message.contains("reparse point") || message.contains("overlap physically")),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            repository::load_project(&db).unwrap().state,
            ProjectState::Validating
        );
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
    }

    #[test]
    fn scan_records_unicode_names_extensions_and_fingerprints() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = project_with_origin(tmp.path());
        let outcome = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();

        let occurrences = list_occurrences(&db, snapshot_of(&outcome)).unwrap();
        assert_eq!(occurrences.len(), 3);
        let acta = occurrences
            .iter()
            .find(|o| o.file_name == "acta \u{00f1}.docx")
            .expect("unicode name preserved");
        assert_eq!(acta.extension.as_deref(), Some("docx"));
        assert_eq!(acta.size_bytes, 3);
        assert_eq!(acta.depth, 3);
        assert!(!acta.name_is_lossy);
        // The scanner now records a v2 fingerprint (ADR-0019): it parses, its
        // size matches, and on a real NTFS volume it carries the physical
        // identity that makes a same-size same-mtime swap detectable.
        let fingerprint = FileFingerprint::parse(&acta.fingerprint).expect("fingerprint parses");
        assert!(matches!(fingerprint, FileFingerprint::V2(_)));
        assert_eq!(fingerprint.size_bytes(), 3);
        #[cfg(windows)]
        assert_eq!(
            fingerprint.guarantee(),
            df_domain::FingerprintGuarantee::Physical,
            "a local NTFS file must yield a physical identity"
        );
        assert!(acta.modified_at_fs.is_some());
        assert!(acta.path_length > 0);

        let folders = list_folders(&db, snapshot_of(&outcome)).unwrap();
        assert!(
            folders
                .iter()
                .all(|folder| folder.raw_relative_path.is_some()),
            "new scans must persist an exact path for every folder, including the root"
        );
        let year = folders
            .iter()
            .find(|folder| folder.relative_path.ends_with("2020"))
            .expect("nested folder inventoried");
        assert_eq!(
            year.raw_relative_path
                .as_ref()
                .expect("raw folder path")
                .to_os_string(),
            PathBuf::from("casos").join("2020").into_os_string()
        );
    }

    #[test]
    fn small_batches_flush_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = project_with_origin(tmp.path());
        let options = ScanOptions { batch_entries: 1 };
        let outcome = scan_project(&mut db, Actor::Test, &options, None).unwrap();
        assert_eq!(outcome.files, 3);
        assert_eq!(outcome.folders, 4);
    }

    #[test]
    fn cancellation_pauses_the_project_and_fails_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = project_with_origin(tmp.path());
        let cancel = AtomicBool::new(true); // cancel before the first folder
        let outcome =
            scan_project(&mut db, Actor::Test, &ScanOptions::default(), Some(&cancel)).unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.state, "SCAN_PAUSED");

        // A cancelled snapshot is FAILED, so no COMPLETE snapshot exists.
        let project = repository::load_project(&db).unwrap();
        assert!(inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .is_none());

        // Resuming produces a fresh, complete snapshot.
        let outcome = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        assert_eq!(outcome.state, "SCANNED");
        assert!(inventory::latest_complete_snapshot(&db, project.id)
            .unwrap()
            .is_some());
    }

    #[test]
    fn validation_rejects_projects_without_roots_but_does_not_kill_them() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = df_domain::Project::new(
            "Sin orígenes",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        repository::create_project(&mut db, &project, &[], Actor::Test).unwrap();

        let err = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)), "{err}");
        // The project must remain recoverable, not FAILED.
        let reloaded = repository::load_project(&db).unwrap();
        assert_eq!(reloaded.state, ProjectState::Validating);
    }

    #[test]
    fn missing_root_is_a_validation_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = Db::open(&tmp.path().join("state.sqlite")).unwrap();
        let project = df_domain::Project::new(
            "Origen inexistente",
            ProfileRef::default(),
            tmp.path().join("salida"),
            tmp.path().join("auditoria"),
            "test",
        );
        let roots = vec![SourceRoot::new(project.id, tmp.path().join("no-existe"))];
        repository::create_project(&mut db, &project, &roots, Actor::Test).unwrap();
        let err = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[test]
    fn rescanning_a_scanned_project_is_rejected_by_the_state_machine() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = project_with_origin(tmp.path());
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let err = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap_err();
        assert!(matches!(err, DfError::Validation(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_symlinks_are_recorded_but_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, origin) = project_with_origin(tmp.path());
        // Creating symlinks needs Developer Mode or elevation; skip cleanly
        // when the environment does not allow it.
        let target = origin.join("casos");
        let link = origin.join("enlace-casos");
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            eprintln!("skipping: symlink creation not permitted in this environment");
            return;
        }
        let outcome = scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        assert_eq!(outcome.reparse_points, 1);
        // Contents under the link must not be duplicated: still 3 files.
        assert_eq!(outcome.files, 3);
    }

    #[test]
    fn scan_emits_audit_events_and_keeps_the_ledger_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, _origin) = project_with_origin(tmp.path());
        scan_project(&mut db, Actor::Test, &ScanOptions::default(), None).unwrap();
        let project = repository::load_project(&db).unwrap();
        let events = repository::list_events(&db, project.id).unwrap();
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert!(types.contains(&"SCAN_STARTED"));
        assert!(types.contains(&"SCAN_COMPLETED"));
        df_ledger::verify_chain(&events).expect("ledger stays valid");
    }

    fn walk_all(root: &Path) -> Vec<(PathBuf, u64)> {
        let mut out = Vec::new();
        let mut queue = vec![root.to_path_buf()];
        while let Some(dir) = queue.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    queue.push(entry.path());
                    out.push((entry.path(), 0));
                } else {
                    out.push((entry.path(), meta.len()));
                }
            }
        }
        out.sort();
        out
    }
}
