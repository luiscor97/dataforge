//! Structural analysis (RFC-0001 §19): folder Merkle signatures and the
//! exact tree-clone sets derived from them.
//!
//! A folder signature is computed bottom-up (§19.2):
//!
//! ```text
//! folder_signature = BLAKE3( sorted( entry(child) for child in folder ) )
//! entry(file)   = "F\0" + normalized_name + "\0" + sha256
//! entry(folder) = "D\0" + normalized_name + "\0" + child_folder_signature
//! ```
//!
//! BLAKE3 is DataForge's tree hash (ADR-0007 / ADR-0023). NUL cannot appear
//! in a filename, so the separator is injection-proof. A folder is *complete*
//! only when every descendant file has a content hash and the subtree has no
//! error entry or unfollowed reparse point; only complete folders may form a
//! clone set, so a partially-scanned branch is never claimed identical to
//! another (safety, §19.4).

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use df_domain::{
    Actor, ProjectId, RawPath, ScanEntryStatus, SnapshotId, TreeCloneSet, TreeCloneSetId,
    TreeRelationship,
};
use df_error::DfResult;
use rusqlite::params;

use crate::repository::{append_event, to_stored_timestamp};
use crate::{db_err, Db};

/// Audit event emitted when structural analysis finishes.
pub const EVENT_STRUCTURE_ANALYZED: &str = "STRUCTURE_ANALYZED";

/// Counts returned by [`compute_folder_signatures`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct StructureSummary {
    /// Folders that received a signature row (every folder in the snapshot).
    pub folders_signed: u64,
    /// Of those, how many have a complete (trustworthy) signature.
    pub complete_folders: u64,
    /// Groups of two or more complete folders that share a signature.
    pub tree_clone_sets: u64,
}

/// The Merkle hash of one folder from its already-encoded child entries.
///
/// Pure and deterministic: the entries are sorted, then hashed with a
/// trailing newline per entry.
fn folder_signature(mut entries: Vec<String>) -> String {
    entries.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    for entry in &entries {
        hasher.update(entry.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// Stable Merkle name for the final component of an exact raw relative path.
///
/// The `raw:` namespace cannot collide with the `display:` compatibility
/// namespace below. Encoding the UTF-16LE bytes preserves unpaired surrogate
/// values on every platform, including when tests run somewhere other than
/// Windows. Only the basename participates: otherwise identical subtrees at
/// different parents would receive different signatures.
fn raw_basename_key(raw: &RawPath) -> String {
    let blob = raw.to_blob();
    let mut start = 0usize;
    for (index, unit) in blob.chunks_exact(2).enumerate() {
        let value = u16::from_le_bytes([unit[0], unit[1]]);
        if value == b'/' as u16 || value == b'\\' as u16 {
            start = (index + 1) * 2;
        }
    }
    let mut key = String::with_capacity(4 + (blob.len() - start) * 2);
    key.push_str("raw:");
    for byte in &blob[start..] {
        use std::fmt::Write as _;
        write!(&mut key, "{byte:02x}").expect("writing to a String cannot fail");
    }
    key
}

/// Select the authoritative child name used by a Merkle entry.
///
/// Legacy snapshots can lack a raw path, in which case a non-lossy display
/// name remains usable under its own namespace. A known lossy name without
/// raw evidence is not safe to identify structurally and makes the subtree
/// incomplete instead.
fn merkle_child_name(
    raw_relative_path: Option<&RawPath>,
    normalized_name: &str,
    name_is_lossy: bool,
) -> Option<String> {
    match raw_relative_path {
        Some(raw) => Some(raw_basename_key(raw)),
        None if name_is_lossy || normalized_name.contains('\u{fffd}') => None,
        None => Some(format!("display:{normalized_name}")),
    }
}

/// Bottom-up state of one folder while computing signatures.
struct Computed {
    signature: Option<String>,
    is_complete: bool,
    subtree_files: u64,
    subtree_bytes: u64,
}

struct FolderRow {
    id: String,
    source_root_id: String,
    relative_path: String,
    normalized_name: String,
    raw_relative_path: Option<RawPath>,
    status: ScanEntryStatus,
    depth: i64,
}

struct FileRow {
    source_root_id: String,
    parent_relative_path: String,
    normalized_name: String,
    raw_relative_path: Option<RawPath>,
    name_is_lossy: bool,
    size_bytes: i64,
    scan_status: ScanEntryStatus,
    sha256: Option<String>,
}

/// Compute and persist the folder signatures of a snapshot, then materialise
/// its exact tree-clone sets. Idempotent: existing rows for the snapshot are
/// replaced, so a re-run after more hashing simply refreshes the evidence.
pub fn compute_folder_signatures(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    actor: Actor,
) -> DfResult<StructureSummary> {
    let snapshot = snapshot_id.to_string();

    // --- read the tree (folders + file occurrences with their content) -----
    let folders: Vec<FolderRow> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT id, source_root_id, relative_path, normalized_name, depth,
                        status, raw_relative_path
                 FROM folders WHERE snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows.into_iter()
            .map(
                |(id, source_root_id, relative_path, normalized_name, depth, status, raw)| {
                    Ok(FolderRow {
                        id,
                        source_root_id,
                        relative_path,
                        normalized_name,
                        raw_relative_path: raw.as_deref().map(RawPath::from_blob).transpose()?,
                        status: ScanEntryStatus::parse(&status)?,
                        depth,
                    })
                },
            )
            .collect::<DfResult<Vec<_>>>()?
    };

    let files: Vec<FileRow> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT o.source_root_id, o.parent_relative_path, o.normalized_name,
                        o.size_bytes, o.scan_status, c.sha256, o.raw_relative_path,
                        o.name_is_lossy
                 FROM path_occurrences o
                 LEFT JOIN occurrence_content oc ON oc.occurrence_id = o.id
                 LEFT JOIN content_objects c ON c.id = oc.content_id
                 WHERE o.snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows.into_iter()
            .map(
                |(
                    source_root_id,
                    parent_relative_path,
                    normalized_name,
                    size_bytes,
                    scan_status,
                    sha256,
                    raw,
                    name_is_lossy,
                )| {
                    Ok(FileRow {
                        source_root_id,
                        parent_relative_path,
                        normalized_name,
                        raw_relative_path: raw.as_deref().map(RawPath::from_blob).transpose()?,
                        name_is_lossy: name_is_lossy != 0,
                        size_bytes,
                        scan_status: ScanEntryStatus::parse(&scan_status)?,
                        sha256,
                    })
                },
            )
            .collect::<DfResult<Vec<_>>>()?
    };

    // --- index children by (source_root_id, parent_relative_path) ----------
    let mut files_by_parent: HashMap<(String, String), Vec<&FileRow>> = HashMap::new();
    for file in &files {
        files_by_parent
            .entry((
                file.source_root_id.clone(),
                file.parent_relative_path.clone(),
            ))
            .or_default()
            .push(file);
    }
    // Child folders of a folder are those whose parent path equals this
    // folder's relative_path (same source root). The root's own relative_path
    // is the empty string, matching its children's parent path.
    let mut child_folders: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (index, folder) in folders.iter().enumerate() {
        // The root folder (relative_path == "") has no parent, so it must not
        // be registered as a child: its own parent path is also the empty
        // string, which would otherwise list it as a child of itself and break
        // the bottom-up ordering. Every other folder's parent path is a prefix,
        // so grouping by relative_path lets a parent find its children.
        if folder.relative_path.is_empty() {
            continue;
        }
        child_folders
            .entry((
                folder.source_root_id.clone(),
                parent_path(&folder.relative_path),
            ))
            .or_default()
            .push(index);
    }

    // --- compute bottom-up (deepest folders first) -------------------------
    let mut order: Vec<usize> = (0..folders.len()).collect();
    order.sort_by(|&a, &b| folders[b].depth.cmp(&folders[a].depth));

    let mut computed: HashMap<String, Computed> = HashMap::new();
    for &index in &order {
        let folder = &folders[index];
        let mut entries: Vec<String> = Vec::new();
        let mut complete = folder.status == ScanEntryStatus::Ok;
        let mut subtree_files: u64 = 0;
        let mut subtree_bytes: u64 = 0;

        if let Some(children) =
            files_by_parent.get(&(folder.source_root_id.clone(), folder.relative_path.clone()))
        {
            for file in children {
                subtree_files += 1;
                subtree_bytes += file.size_bytes.max(0) as u64;
                let name = merkle_child_name(
                    file.raw_relative_path.as_ref(),
                    &file.normalized_name,
                    file.name_is_lossy,
                );
                match (&file.sha256, file.scan_status, name) {
                    (Some(sha), ScanEntryStatus::Ok, Some(name)) => {
                        entries.push(format!("F\0{name}\0{sha}"));
                    }
                    _ => complete = false,
                }
            }
        }

        if let Some(children) =
            child_folders.get(&(folder.source_root_id.clone(), folder.relative_path.clone()))
        {
            for &child_index in children {
                let child = &folders[child_index];
                let child_computed = computed
                    .get(&child.id)
                    .expect("children are computed before their parent");
                subtree_files += child_computed.subtree_files;
                subtree_bytes += child_computed.subtree_bytes;
                let name = merkle_child_name(
                    child.raw_relative_path.as_ref(),
                    &child.normalized_name,
                    false,
                );
                match (&child_computed.signature, child_computed.is_complete, name) {
                    (Some(sig), true, Some(name)) => {
                        entries.push(format!("D\0{name}\0{sig}"));
                    }
                    _ => complete = false,
                }
            }
        }

        let signature = if complete {
            Some(folder_signature(entries))
        } else {
            None
        };
        computed.insert(
            folder.id.clone(),
            Computed {
                signature,
                is_complete: complete,
                subtree_files,
                subtree_bytes,
            },
        );
    }

    // --- persist -----------------------------------------------------------
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "DELETE FROM tree_clone_sets WHERE snapshot_id = ?1",
        [&snapshot],
    )
    .map_err(db_err)?;
    tx.execute(
        "DELETE FROM folder_signatures WHERE snapshot_id = ?1",
        [&snapshot],
    )
    .map_err(db_err)?;

    let mut complete_folders: u64 = 0;
    // Signature -> (folder_count, subtree_files, subtree_bytes) for clone sets.
    let mut groups: HashMap<String, (u64, u64, u64)> = HashMap::new();
    for folder in &folders {
        let c = &computed[&folder.id];
        if c.is_complete {
            complete_folders += 1;
        }
        if let Some(sig) = &c.signature {
            if c.subtree_files >= 1 {
                let entry =
                    groups
                        .entry(sig.clone())
                        .or_insert((0, c.subtree_files, c.subtree_bytes));
                entry.0 += 1;
            }
        }
        tx.execute(
            "INSERT INTO folder_signatures
                (folder_id, snapshot_id, source_root_id, relative_path,
                 signature, is_complete, subtree_files, subtree_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                folder.id,
                snapshot,
                folder.source_root_id,
                folder.relative_path,
                c.signature,
                c.is_complete as i64,
                c.subtree_files as i64,
                c.subtree_bytes as i64,
                now,
            ],
        )
        .map_err(db_err)?;
    }

    let mut tree_clone_sets: u64 = 0;
    for (signature, (folder_count, subtree_files, subtree_bytes)) in &groups {
        if *folder_count >= 2 {
            tree_clone_sets += 1;
            tx.execute(
                "INSERT INTO tree_clone_sets
                    (id, snapshot_id, signature, relationship, folder_count,
                     subtree_files, subtree_bytes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    TreeCloneSetId::new().to_string(),
                    snapshot,
                    signature,
                    TreeRelationship::ExactClone.as_str(),
                    *folder_count as i64,
                    *subtree_files as i64,
                    *subtree_bytes as i64,
                    now,
                ],
            )
            .map_err(db_err)?;
        }
    }

    let summary = StructureSummary {
        folders_signed: folders.len() as u64,
        complete_folders,
        tree_clone_sets,
    };
    let payload = serde_json::json!({
        "snapshot_id": snapshot,
        "folders_signed": summary.folders_signed,
        "complete_folders": summary.complete_folders,
        "tree_clone_sets": summary.tree_clone_sets,
    });
    append_event(&tx, project_id, EVENT_STRUCTURE_ANALYZED, &payload, actor)?;
    tx.commit().map_err(db_err)?;

    Ok(summary)
}

/// Parent path key of a relative path: the root ("") groups under "".
fn parent_path(relative_path: &str) -> String {
    match relative_path.rfind(['/', '\\']) {
        Some(pos) => relative_path[..pos].to_string(),
        None => String::new(),
    }
}

/// Read the materialised exact tree-clone sets of a snapshot, largest waste
/// first, each with its member folders' absolute paths.
pub fn tree_clone_sets(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<TreeCloneSet>> {
    let snapshot = snapshot_id.to_string();
    let mut sets: Vec<TreeCloneSet> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT id, signature, relationship, subtree_files, subtree_bytes
                 FROM tree_clone_sets WHERE snapshot_id = ?1
                 ORDER BY subtree_bytes DESC, signature",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows.into_iter()
            .map(|(id, signature, relationship, files, bytes)| {
                Ok(TreeCloneSet {
                    id: TreeCloneSetId::from_str(&id)?,
                    snapshot_id,
                    signature,
                    relationship: TreeRelationship::parse(&relationship)?,
                    folders: Vec::new(),
                    subtree_files: files as u64,
                    subtree_bytes: bytes as u64,
                })
            })
            .collect::<DfResult<Vec<_>>>()?
    };

    // Attach member folders (absolute paths) for each set.
    let mut members = db
        .conn()
        .prepare(
            "SELECT r.absolute_path, fs.relative_path
             FROM folder_signatures fs
             JOIN source_roots r ON r.id = fs.source_root_id
             WHERE fs.snapshot_id = ?1 AND fs.signature = ?2
             ORDER BY r.absolute_path, fs.relative_path",
        )
        .map_err(db_err)?;
    for set in &mut sets {
        let folders = members
            .query_map(params![snapshot, set.signature], |row| {
                let root: String = row.get(0)?;
                let relative: String = row.get(1)?;
                Ok(join_absolute(&root, &relative))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        set.folders = folders;
    }
    Ok(sets)
}

fn join_absolute(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        root.to_string()
    } else {
        format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_signature_is_order_independent_and_deterministic() {
        let a = folder_signature(vec!["F\0a.txt\0aa".to_string(), "F\0b.txt\0bb".to_string()]);
        let b = folder_signature(vec!["F\0b.txt\0bb".to_string(), "F\0a.txt\0aa".to_string()]);
        assert_eq!(a, b, "child order must not change the signature");
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn folder_signature_changes_with_content() {
        let a = folder_signature(vec!["F\0a.txt\0aa".to_string()]);
        let b = folder_signature(vec!["F\0a.txt\0ab".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn empty_folder_has_a_stable_signature() {
        assert_eq!(folder_signature(vec![]), folder_signature(vec![]));
    }

    fn raw_from_units(units: &[u16]) -> RawPath {
        let blob: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        RawPath::from_blob(&blob).unwrap()
    }

    #[test]
    fn raw_merkle_names_use_only_the_basename_and_preserve_invalid_utf16() {
        let first = raw_from_units(&[b'A' as u16, b'\\' as u16, 0xd800]);
        let same_name_elsewhere = raw_from_units(&[b'B' as u16, b'/' as u16, 0xd800]);
        let different_raw_name = raw_from_units(&[b'B' as u16, b'/' as u16, 0xd801]);

        assert_eq!(
            raw_basename_key(&first),
            raw_basename_key(&same_name_elsewhere),
            "the parent path must not affect a child Merkle name"
        );
        assert_ne!(
            raw_basename_key(&first),
            raw_basename_key(&different_raw_name),
            "distinct raw UTF-16 names must never collapse through U+FFFD"
        );
    }

    #[test]
    fn bounded_candidate_pairs_are_deterministic_and_count_unique_omissions() {
        let key = |path: &str| StableFolderKey::new("root", path);
        let mut first = HashMap::new();
        first.insert("shared-1".to_string(), vec![key("c"), key("a"), key("b")]);
        // a/b share two contents: this must still be one candidate pair.
        first.insert("shared-2".to_string(), vec![key("b"), key("a")]);

        let mut reversed = HashMap::new();
        reversed.insert("shared-2".to_string(), vec![key("a"), key("b")]);
        reversed.insert("shared-1".to_string(), vec![key("b"), key("c"), key("a")]);

        let expected = limited_candidate_pairs(&first, 32, 2);
        let reordered = limited_candidate_pairs(&reversed, 32, 2);
        assert_eq!(expected, reordered);
        assert_eq!(expected.0.len(), 2);
        assert_eq!(expected.1, 1, "only the distinct third pair was omitted");
        assert_eq!(expected.0, vec![(key("a"), key("b")), (key("a"), key("c"))]);
    }

    #[test]
    fn candidate_cap_is_stable_across_rescans_with_new_folder_ids() {
        fn folder(folder_id: &str, relative_path: &str) -> (StableFolderKey, SubtreeContents) {
            (
                StableFolderKey::new("source-root", relative_path),
                SubtreeContents {
                    folder_id: folder_id.to_string(),
                    contents: ["shared-content".to_string()].into_iter().collect(),
                    bytes_by_content: HashMap::new(),
                },
            )
        }

        // The per-snapshot UUID ordering is deliberately reversed. An id-based
        // cap would select different logical pairs in these two scans.
        let first_scan = HashMap::from([
            folder("folder-z", "a"),
            folder("folder-y", "b"),
            folder("folder-x", "c"),
        ]);
        let rescanned = HashMap::from([
            folder("folder-x", "a"),
            folder("folder-y2", "b"),
            folder("folder-z", "c"),
        ]);

        for key in first_scan.keys() {
            assert_ne!(first_scan[key].folder_id, rescanned[key].folder_id);
        }

        let expected = limited_candidate_pairs_for_folders(&first_scan, 32, 2);
        let actual = limited_candidate_pairs_for_folders(&rescanned, 32, 2);
        assert_eq!(actual, expected);
        assert_eq!(actual.1, 1, "the cap must omit the same logical pair");
        assert_eq!(
            actual.0,
            vec![
                (
                    StableFolderKey::new("source-root", "a"),
                    StableFolderKey::new("source-root", "b"),
                ),
                (
                    StableFolderKey::new("source-root", "a"),
                    StableFolderKey::new("source-root", "c"),
                ),
            ]
        );
    }

    #[test]
    fn parent_path_groups_root_children_under_empty() {
        assert_eq!(parent_path(""), "");
        assert_eq!(parent_path("sub"), "");
        assert_eq!(parent_path("sub\\deep"), "sub");
        assert_eq!(parent_path("a/b/c"), "a/b");
    }

    // --- DB integration ----------------------------------------------------

    use std::path::PathBuf;

    use df_domain::{ProfileRef, Project, SourceRoot, SourceRootId};

    use crate::repository;

    struct Seed {
        snapshot: SnapshotId,
        root: SourceRootId,
    }

    fn seed(db: &mut Db) -> (ProjectId, Seed) {
        let project = Project::new(
            "p",
            ProfileRef::default(),
            PathBuf::from("D:/out"),
            PathBuf::from("D:/audit"),
            "test",
        );
        let root = SourceRoot::new(project.id, PathBuf::from("D:/in"));
        let root_id = root.id;
        repository::create_project(db, &project, &[root], Actor::Test).unwrap();
        let (snapshot, _run) = crate::inventory::start_scan(db, project.id, Actor::Test).unwrap();
        (
            project.id,
            Seed {
                snapshot: snapshot.id,
                root: root_id,
            },
        )
    }

    fn add_folder(db: &Db, s: &Seed, rel: &str, parent: Option<&str>, name: &str) {
        add_folder_with_status(db, s, rel, parent, name, "OK");
    }

    fn add_folder_with_status(
        db: &Db,
        s: &Seed,
        rel: &str,
        parent: Option<&str>,
        name: &str,
        status: &str,
    ) {
        db.conn()
            .execute(
                "INSERT INTO folders
                    (id, snapshot_id, source_root_id, relative_path,
                     parent_relative_path, name, normalized_name, depth, status,
                     created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'t')",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    s.snapshot.to_string(),
                    s.root.to_string(),
                    rel,
                    parent,
                    name,
                    name.to_lowercase(),
                    rel.matches(['/', '\\']).count() as i64 + if rel.is_empty() { 0 } else { 1 },
                    status,
                ],
            )
            .unwrap();
    }

    /// Add a file occurrence; when `sha` is Some, also store its content and
    /// link it (a hashed file). `None` leaves the file unhashed (incomplete).
    fn add_file(db: &Db, s: &Seed, parent: &str, name: &str, sha: Option<&str>) {
        let occ = uuid::Uuid::new_v4().to_string();
        let rel = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        db.conn()
            .execute(
                "INSERT INTO path_occurrences
                    (id, snapshot_id, source_root_id, relative_path,
                     parent_relative_path, file_name, normalized_name, extension,
                     size_bytes, attributes, path_length, depth, fingerprint,
                     scan_status, name_is_lossy, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,10,0,?8,1,'v1:10:0','OK',0,'t')",
                params![
                    occ,
                    s.snapshot.to_string(),
                    s.root.to_string(),
                    rel,
                    parent,
                    name,
                    name.to_lowercase(),
                    rel.len() as i64,
                ],
            )
            .unwrap();
        if let Some(sha) = sha {
            let content = uuid::Uuid::new_v4().to_string();
            db.conn()
                .execute(
                    "INSERT OR IGNORE INTO content_objects
                        (id, size_bytes, sha256, blake3, first_seen_snapshot,
                         hash_state, created_at)
                     VALUES (?1,10,?2,?3,?4,'HASHED','t')",
                    params![content, sha, sha, s.snapshot.to_string()],
                )
                .unwrap();
            // Resolve the actual content id (the row above may have been ignored
            // because the sha already exists).
            let content_id: String = db
                .conn()
                .query_row(
                    "SELECT id FROM content_objects WHERE sha256 = ?1",
                    [sha],
                    |r| r.get(0),
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO occurrence_content (occurrence_id, content_id, created_at)
                     VALUES (?1, ?2, 't')",
                    params![occ, content_id],
                )
                .unwrap();
        }
    }

    #[test]
    fn identical_subtrees_are_detected_as_an_exact_clone() {
        let sha_x = "a".repeat(64);
        let sha_y = "b".repeat(64);
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, s) = seed(&mut db);
        // root with three subfolders; A and B are identical, C differs.
        add_folder(&db, &s, "", None, "in");
        add_folder(&db, &s, "A", Some(""), "A");
        add_folder(&db, &s, "B", Some(""), "B");
        add_folder(&db, &s, "C", Some(""), "C");
        add_file(&db, &s, "A", "x.txt", Some(&sha_x));
        add_file(&db, &s, "B", "x.txt", Some(&sha_x));
        add_file(&db, &s, "C", "y.txt", Some(&sha_y));

        let summary =
            compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
        assert_eq!(summary.folders_signed, 4);
        assert_eq!(summary.complete_folders, 4);
        assert_eq!(summary.tree_clone_sets, 1);

        let sets = tree_clone_sets(&db, s.snapshot).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].relationship, TreeRelationship::ExactClone);
        assert_eq!(sets[0].folders.len(), 2);
        assert!(sets[0].folders.iter().any(|f| f.ends_with('A')));
        assert!(sets[0].folders.iter().any(|f| f.ends_with('B')));
        assert_eq!(sets[0].subtree_files, 1);
        // One redundant copy of a 10-byte file.
        assert_eq!(sets[0].redundant_bytes(), 10);
    }

    #[test]
    fn recompute_is_idempotent() {
        let sha = "c".repeat(64);
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, s) = seed(&mut db);
        add_folder(&db, &s, "", None, "in");
        add_folder(&db, &s, "A", Some(""), "A");
        add_folder(&db, &s, "B", Some(""), "B");
        add_file(&db, &s, "A", "f", Some(&sha));
        add_file(&db, &s, "B", "f", Some(&sha));

        let first =
            compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
        let second =
            compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
        assert_eq!(first, second);
        // No duplicated rows after a second run.
        assert_eq!(tree_clone_sets(&db, s.snapshot).unwrap().len(), 1);
    }

    #[test]
    fn an_unhashed_file_makes_its_folder_incomplete_and_unclonable() {
        let sha = "d".repeat(64);
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, s) = seed(&mut db);
        add_folder(&db, &s, "", None, "in");
        add_folder(&db, &s, "A", Some(""), "A");
        add_folder(&db, &s, "B", Some(""), "B");
        // A is fully hashed; B has the same file but unhashed → incomplete.
        add_file(&db, &s, "A", "f", Some(&sha));
        add_file(&db, &s, "B", "f", None);

        let summary =
            compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
        // root and B are incomplete (B unhashed, root contains B); only A is
        // complete.
        assert_eq!(summary.complete_folders, 1);
        // A and B never match, so no clone despite identical names.
        assert_eq!(summary.tree_clone_sets, 0);
        assert!(tree_clone_sets(&db, s.snapshot).unwrap().is_empty());
    }

    #[test]
    fn an_error_or_unfollowed_reparse_folder_makes_every_ancestor_incomplete() {
        for blocked_status in ["ERROR", "REPARSE_NOT_FOLLOWED"] {
            let sha = "e".repeat(64);
            let mut db = Db::open_in_memory().unwrap();
            let (project_id, s) = seed(&mut db);
            add_folder(&db, &s, "", None, "in");
            add_folder(&db, &s, "A", Some(""), "A");
            add_folder(&db, &s, "B", Some(""), "B");
            add_folder_with_status(&db, &s, "B/hidden", Some("B"), "hidden", blocked_status);
            add_file(&db, &s, "A", "same.txt", Some(&sha));
            add_file(&db, &s, "B", "same.txt", Some(&sha));

            let summary =
                compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
            assert_eq!(summary.complete_folders, 1, "status {blocked_status}");
            assert_eq!(summary.tree_clone_sets, 0, "status {blocked_status}");

            let b_complete: i64 = db
                .conn()
                .query_row(
                    "SELECT fs.is_complete
                     FROM folder_signatures fs
                     JOIN folders f ON f.id = fs.folder_id
                     WHERE f.snapshot_id = ?1 AND f.relative_path = 'B'",
                    [s.snapshot.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(b_complete, 0, "status {blocked_status} must propagate");
        }
    }

    #[test]
    fn a_lossy_file_without_raw_identity_makes_the_branch_incomplete() {
        let sha = "f".repeat(64);
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, s) = seed(&mut db);
        add_folder(&db, &s, "", None, "in");
        add_folder(&db, &s, "A", Some(""), "A");
        add_file(&db, &s, "A", "damaged\u{fffd}.txt", Some(&sha));
        db.conn()
            .execute(
                "UPDATE path_occurrences
                 SET name_is_lossy = 1, raw_relative_path = NULL
                 WHERE snapshot_id = ?1 AND relative_path = 'A/damaged�.txt'",
                [s.snapshot.to_string()],
            )
            .unwrap();

        let summary =
            compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
        assert_eq!(summary.complete_folders, 0);
        assert_eq!(summary.tree_clone_sets, 0);
    }

    #[test]
    fn different_raw_file_names_do_not_form_a_false_exact_clone() {
        let sha = "1".repeat(64);
        let mut db = Db::open_in_memory().unwrap();
        let (project_id, s) = seed(&mut db);
        add_folder(&db, &s, "", None, "in");
        add_folder(&db, &s, "A", Some(""), "A");
        add_folder(&db, &s, "B", Some(""), "B");
        let display = "damaged\u{fffd}.txt";
        add_file(&db, &s, "A", display, Some(&sha));
        add_file(&db, &s, "B", display, Some(&sha));

        let raw_a = raw_from_units(&[
            b'A' as u16,
            b'/' as u16,
            b'd' as u16,
            0xd800,
            b'.' as u16,
            b't' as u16,
            b'x' as u16,
            b't' as u16,
        ]);
        let raw_b = raw_from_units(&[
            b'B' as u16,
            b'/' as u16,
            b'd' as u16,
            0xd801,
            b'.' as u16,
            b't' as u16,
            b'x' as u16,
            b't' as u16,
        ]);
        for (relative, raw) in [("A/damaged�.txt", raw_a), ("B/damaged�.txt", raw_b)] {
            db.conn()
                .execute(
                    "UPDATE path_occurrences
                     SET name_is_lossy = 1, raw_relative_path = ?1
                     WHERE snapshot_id = ?2 AND relative_path = ?3",
                    params![raw.to_blob(), s.snapshot.to_string(), relative],
                )
                .unwrap();
        }

        let summary =
            compute_folder_signatures(&mut db, project_id, s.snapshot, Actor::Test).unwrap();
        assert_eq!(summary.complete_folders, 3);
        assert_eq!(summary.tree_clone_sets, 0);
    }
}

// ---------------------------------------------------------------------------
// Pairwise tree relations (RFC-0001 §19.3, §19.4)
// ---------------------------------------------------------------------------

/// Tuning knobs of [`compute_tree_relations`].
#[derive(Debug, Clone)]
pub struct TreeRelationOptions {
    /// Ignore folders whose subtree holds fewer distinct contents than this.
    /// Two folders sharing a single file are not "almost the same folder".
    pub min_subtree_contents: usize,
    /// A content present in more than this many folders is a *component*
    /// (a logo, a template, a licence): it says nothing about two folders
    /// being related, and pairing every holder of it explodes quadratically.
    pub max_folders_per_content: usize,
    /// Only report pairs at or above this Jaccard similarity. Below it the
    /// overlap is indistinguishable from coincidence
    /// (`REPEATED_COMPONENT_ONLY`).
    pub min_similarity: f64,
    /// Hard ceiling on candidate pairs examined and persisted. Distinct
    /// candidates beyond it are counted in `pairs_skipped`.
    pub max_pairs: usize,
}

impl Default for TreeRelationOptions {
    fn default() -> Self {
        Self {
            min_subtree_contents: 2,
            max_folders_per_content: 32,
            min_similarity: 0.5,
            max_pairs: 200_000,
        }
    }
}

/// Counts returned by [`compute_tree_relations`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct TreeRelationSummary {
    pub partial_clones: u64,
    pub embedded: u64,
    /// Candidate pairs left unexamined because `max_pairs` was reached.
    pub pairs_skipped: u64,
    /// Pass-through containers suppressed: ancestors whose subtree content is
    /// identical to a descendant folder's. Their relations would duplicate
    /// the descendant's, so only the deepest, most specific folder reports.
    pub pass_through_suppressed: u64,
}

/// Every ancestor folder path of a file, from its parent up to the root.
/// `"a/b"` yields `["a/b", "a", ""]`.
fn ancestors_of(parent_relative_path: &str) -> Vec<String> {
    let mut out = vec![parent_relative_path.to_string()];
    let mut current = parent_relative_path;
    while let Some((head, _)) = current.rsplit_once(['/', '\\']) {
        out.push(head.to_string());
        current = head;
    }
    if !out.iter().any(|p| p.is_empty()) {
        out.push(String::new());
    }
    out
}

/// Is `ancestor` a path-ancestor of `descendant` within the same root?
///
/// A folder shares every content with its own parent, so those pairs are
/// noise: without this check every folder would be "embedded" in its parent.
fn is_ancestor_of(ancestor: &str, descendant: &str) -> bool {
    if ancestor == descendant {
        return false;
    }
    if ancestor.is_empty() {
        return true; // the root contains everything
    }
    descendant
        .strip_prefix(ancestor)
        .is_some_and(|rest| rest.starts_with(['/', '\\']))
}

/// Snapshot-independent identity used to order folders and apply candidate
/// limits reproducibly across rescans of the same project.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct StableFolderKey {
    source_root_id: String,
    relative_path: String,
}

impl StableFolderKey {
    fn new(source_root_id: impl Into<String>, relative_path: impl Into<String>) -> Self {
        Self {
            source_root_id: source_root_id.into(),
            relative_path: relative_path.into(),
        }
    }
}

/// A folder's subtree, as the set of distinct contents it holds.
struct SubtreeContents {
    folder_id: String,
    contents: std::collections::HashSet<String>,
    bytes_by_content: HashMap<String, u64>,
}

/// Build the bounded candidate set in a stable order and report the number of
/// distinct candidates omitted by the bound.
///
/// A pair can share several contents, so counting each rejected insertion
/// overstates truncation. Materialising the distinct set first both removes
/// that ambiguity and makes selection independent of `HashMap` randomisation.
fn limited_candidate_pairs(
    by_content: &HashMap<String, Vec<StableFolderKey>>,
    max_folders_per_content: usize,
    max_pairs: usize,
) -> (Vec<(StableFolderKey, StableFolderKey)>, u64) {
    let mut distinct: BTreeSet<(StableFolderKey, StableFolderKey)> = BTreeSet::new();
    for holders in by_content.values() {
        if holders.len() > max_folders_per_content {
            continue;
        }
        let mut holders = holders.clone();
        holders.sort_unstable();
        holders.dedup();
        for (index, a) in holders.iter().enumerate() {
            for b in holders.iter().skip(index + 1) {
                distinct.insert((a.clone(), b.clone()));
            }
        }
    }

    let pairs_skipped = distinct.len().saturating_sub(max_pairs) as u64;
    let selected = distinct.into_iter().take(max_pairs).collect();
    (selected, pairs_skipped)
}

fn limited_candidate_pairs_for_folders(
    folders: &HashMap<StableFolderKey, SubtreeContents>,
    max_folders_per_content: usize,
    max_pairs: usize,
) -> (Vec<(StableFolderKey, StableFolderKey)>, u64) {
    let mut by_content: HashMap<String, Vec<StableFolderKey>> = HashMap::new();
    for (key, folder) in folders {
        for content in &folder.contents {
            by_content
                .entry(content.clone())
                .or_default()
                .push(key.clone());
        }
    }
    limited_candidate_pairs(&by_content, max_folders_per_content, max_pairs)
}

/// Compute pairwise `PARTIAL_TREE_CLONE` / `TREE_EMBEDDED` relations between
/// the complete folders of a snapshot (§19.3) and persist them together with
/// the evidence of what each side holds uniquely (§19.4).
///
/// Evidence only: nothing here proposes consolidating anything. A pair with
/// unique content on both sides is precisely a reason *not* to drop either
/// branch. Idempotent: rows for the snapshot are replaced.
pub fn compute_tree_relations(
    db: &mut Db,
    project_id: ProjectId,
    snapshot_id: SnapshotId,
    options: &TreeRelationOptions,
    actor: Actor,
) -> DfResult<TreeRelationSummary> {
    let snapshot = snapshot_id.to_string();

    // Complete folders only: a partially scanned branch must never be claimed
    // similar to another (§19.4).
    let folder_rows: Vec<(String, String, String)> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT folder_id, source_root_id, relative_path
                 FROM folder_signatures
                 WHERE snapshot_id = ?1 AND is_complete = 1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    // A source-root/path pair is stable across snapshots of the same project;
    // folder ids are newly generated per scan and must not drive bounded
    // candidate selection.
    let mut folders: HashMap<StableFolderKey, SubtreeContents> = HashMap::new();
    for (folder_id, root_id, relative) in folder_rows {
        folders.insert(
            StableFolderKey::new(root_id, relative),
            SubtreeContents {
                folder_id,
                contents: std::collections::HashSet::new(),
                bytes_by_content: HashMap::new(),
            },
        );
    }

    let occurrences: Vec<(String, String, String, u64)> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT o.source_root_id, o.parent_relative_path, oc.content_id,
                        c.size_bytes
                 FROM path_occurrences o
                 JOIN occurrence_content oc ON oc.occurrence_id = o.id
                 JOIN content_objects c ON c.id = oc.content_id
                 WHERE o.snapshot_id = ?1 AND o.scan_status = 'OK'",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? as u64,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    // Roll each occurrence up into every ancestor folder's content set:
    // O(occurrences x depth) instead of a quadratic path match.
    for (root_id, parent, content_id, size) in occurrences {
        for ancestor in ancestors_of(&parent) {
            let key = StableFolderKey::new(root_id.clone(), ancestor);
            if let Some(folder) = folders.get_mut(&key) {
                folder.bytes_by_content.insert(content_id.clone(), size);
                folder.contents.insert(content_id.clone());
            }
        }
    }
    folders.retain(|_, f| f.contents.len() >= options.min_subtree_contents);

    // A pass-through container — an ancestor whose subtree content set is
    // identical to a descendant folder's (e.g. `Backup/` holding only
    // `Backup/Expediente 77/`) — relates to exactly the same third folders
    // as that descendant, duplicating every one of its relations. Report the
    // deepest, most specific folder only. Within one root an ancestor's set
    // always contains the descendant's, so equal cardinality means equal
    // sets. Deterministic: driven by stable keys, not map order.
    let pass_through: std::collections::HashSet<StableFolderKey> = folders
        .keys()
        .flat_map(|key| {
            let descendant_len = folders[key].contents.len();
            ancestors_of(&key.relative_path)
                .into_iter()
                .skip(1) // the chain starts at the folder itself
                .map(|ancestor| StableFolderKey::new(key.source_root_id.clone(), ancestor))
                .filter(|ancestor_key| {
                    folders
                        .get(ancestor_key)
                        .is_some_and(|ancestor| ancestor.contents.len() == descendant_len)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let pass_through_suppressed = pass_through.len() as u64;
    folders.retain(|key, _| !pass_through.contains(key));

    // Inverted index content -> folders, so only folders that actually share
    // something get paired.
    let (candidates, pairs_skipped) = limited_candidate_pairs_for_folders(
        &folders,
        options.max_folders_per_content,
        options.max_pairs,
    );

    let mut relations: Vec<(df_domain::TreeRelation, Option<&'static str>)> = Vec::new();
    for (a_key, b_key) in &candidates {
        let (a, b) = (&folders[a_key], &folders[b_key]);
        // Same root and one inside the other: trivially "shared", not a clone.
        if a_key.source_root_id == b_key.source_root_id
            && (is_ancestor_of(&a_key.relative_path, &b_key.relative_path)
                || is_ancestor_of(&b_key.relative_path, &a_key.relative_path))
        {
            continue;
        }

        let shared: Vec<&String> = a.contents.intersection(&b.contents).collect();
        let shared_files = shared.len() as u64;
        let unique_a = (a.contents.len() - shared.len()) as u64;
        let unique_b = (b.contents.len() - shared.len()) as u64;
        // Exact clones are already reported as clone sets (migration 0006).
        if unique_a == 0 && unique_b == 0 {
            continue;
        }
        let union = shared_files + unique_a + unique_b;
        let similarity = if union == 0 {
            0.0
        } else {
            shared_files as f64 / union as f64
        };
        if similarity < options.min_similarity {
            continue; // REPEATED_COMPONENT_ONLY territory: not reported
        }
        let shared_bytes: u64 = shared
            .iter()
            .filter_map(|c| a.bytes_by_content.get(c.as_str()))
            .sum();

        // Candidate endpoints are already ordered by the stable source/path
        // key, so A/B is independent of the per-snapshot folder UUIDs.
        debug_assert!(a_key < b_key);
        let (fa, fb, ua, ub) = (a, b, unique_a, unique_b);
        let (relationship, contained) = match (ua, ub) {
            (0, _) => (TreeRelationship::Embedded, Some("A")),
            (_, 0) => (TreeRelationship::Embedded, Some("B")),
            _ => (TreeRelationship::PartialClone, None),
        };
        relations.push((
            df_domain::TreeRelation {
                snapshot_id,
                folder_a: df_domain::FolderId::from_str(&fa.folder_id)?,
                folder_b: df_domain::FolderId::from_str(&fb.folder_id)?,
                relationship,
                shared_files,
                unique_a_files: ua,
                unique_b_files: ub,
                shared_bytes,
                similarity,
            },
            contained,
        ));
    }

    let mut summary = TreeRelationSummary {
        pairs_skipped,
        pass_through_suppressed,
        ..Default::default()
    };
    let now = to_stored_timestamp(chrono::Utc::now());
    let tx = db.conn_mut().transaction().map_err(db_err)?;
    tx.execute(
        "DELETE FROM tree_relations WHERE snapshot_id = ?1",
        [&snapshot],
    )
    .map_err(db_err)?;
    for (relation, contained) in &relations {
        match relation.relationship {
            TreeRelationship::PartialClone => summary.partial_clones += 1,
            TreeRelationship::Embedded => summary.embedded += 1,
            _ => {}
        }
        tx.execute(
            "INSERT INTO tree_relations
                (id, snapshot_id, folder_a, folder_b, relationship, contained,
                 shared_files, unique_a_files, unique_b_files, shared_bytes,
                 similarity, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                uuid::Uuid::new_v4().to_string(),
                snapshot,
                relation.folder_a.to_string(),
                relation.folder_b.to_string(),
                relation.relationship.as_str(),
                contained,
                relation.shared_files as i64,
                relation.unique_a_files as i64,
                relation.unique_b_files as i64,
                relation.shared_bytes as i64,
                relation.similarity,
                now,
            ],
        )
        .map_err(db_err)?;
    }
    let payload = serde_json::json!({
        "snapshot_id": snapshot,
        "partial_clones": summary.partial_clones,
        "embedded": summary.embedded,
        "pairs_skipped": summary.pairs_skipped,
        "pass_through_suppressed": summary.pass_through_suppressed,
    });
    append_event(&tx, project_id, EVENT_STRUCTURE_ANALYZED, &payload, actor)?;
    tx.commit().map_err(db_err)?;
    Ok(summary)
}

/// Read the pairwise tree relations of a snapshot, most similar first.
///
/// Evidence for review (§19.3): each row says what the two folders share and,
/// crucially, what each one holds that the other does not (§19.4).
pub fn tree_relations(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<df_domain::TreeRelation>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT folder_a, folder_b, relationship, shared_files,
                    unique_a_files, unique_b_files, shared_bytes, similarity
             FROM tree_relations
             WHERE snapshot_id = ?1
             ORDER BY similarity DESC, shared_files DESC",
        )
        .map_err(db_err)?;
    let rows: Vec<DfResult<df_domain::TreeRelation>> = stmt
        .query_map([snapshot_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, f64>(7)?,
            ))
        })
        .map_err(db_err)?
        .map(|raw| {
            let (a, b, relationship, shared, ua, ub, bytes, similarity) = raw.map_err(db_err)?;
            Ok(df_domain::TreeRelation {
                snapshot_id,
                folder_a: df_domain::FolderId::from_str(&a)?,
                folder_b: df_domain::FolderId::from_str(&b)?,
                relationship: TreeRelationship::parse(&relationship)?,
                shared_files: shared as u64,
                unique_a_files: ua as u64,
                unique_b_files: ub as u64,
                shared_bytes: bytes as u64,
                similarity,
            })
        })
        .collect();
    rows.into_iter().collect()
}

/// A tree relation with both folder paths resolved, ready to show a human.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TreeRelationView {
    pub path_a: String,
    pub path_b: String,
    /// `PARTIAL_TREE_CLONE` or `TREE_EMBEDDED`.
    pub relationship: String,
    /// For `TREE_EMBEDDED`, which side is inside the other (`A` or `B`).
    pub contained: Option<String>,
    pub shared_files: u64,
    /// Contents only in `path_a`: what would be lost by dropping that side.
    pub unique_a_files: u64,
    /// Contents only in `path_b`: what would be lost by dropping that side.
    pub unique_b_files: u64,
    pub shared_bytes: u64,
    pub similarity: f64,
}

/// Read the tree relations of a snapshot with their folder paths resolved,
/// most similar first (§19.3).
pub fn tree_relation_views(db: &Db, snapshot_id: SnapshotId) -> DfResult<Vec<TreeRelationView>> {
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT ra.absolute_path, fa.relative_path,
                    rb.absolute_path, fb.relative_path,
                    tr.relationship, tr.contained, tr.shared_files,
                    tr.unique_a_files, tr.unique_b_files, tr.shared_bytes,
                    tr.similarity
             FROM tree_relations tr
             JOIN folders fa ON fa.id = tr.folder_a
             JOIN folders fb ON fb.id = tr.folder_b
             JOIN source_roots ra ON ra.id = fa.source_root_id
             JOIN source_roots rb ON rb.id = fb.source_root_id
             WHERE tr.snapshot_id = ?1
             ORDER BY tr.similarity DESC, tr.shared_files DESC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([snapshot_id.to_string()], |row| {
            let join = |root: String, relative: String| {
                if relative.is_empty() {
                    root
                } else {
                    format!("{root}{}{relative}", std::path::MAIN_SEPARATOR)
                }
            };
            Ok(TreeRelationView {
                path_a: join(row.get(0)?, row.get(1)?),
                path_b: join(row.get(2)?, row.get(3)?),
                relationship: row.get(4)?,
                contained: row.get(5)?,
                shared_files: row.get::<_, i64>(6)? as u64,
                unique_a_files: row.get::<_, i64>(7)? as u64,
                unique_b_files: row.get::<_, i64>(8)? as u64,
                shared_bytes: row.get::<_, i64>(9)? as u64,
                similarity: row.get(10)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}
