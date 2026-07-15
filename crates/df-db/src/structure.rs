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

use std::collections::HashMap;
use std::str::FromStr;

use df_domain::{Actor, ProjectId, SnapshotId, TreeCloneSet, TreeCloneSetId, TreeRelationship};
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
    depth: i64,
}

struct FileRow {
    source_root_id: String,
    parent_relative_path: String,
    normalized_name: String,
    size_bytes: i64,
    scan_status: String,
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
                "SELECT id, source_root_id, relative_path, normalized_name, depth
                 FROM folders WHERE snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok(FolderRow {
                    id: row.get(0)?,
                    source_root_id: row.get(1)?,
                    relative_path: row.get(2)?,
                    normalized_name: row.get(3)?,
                    depth: row.get(4)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
    };

    let files: Vec<FileRow> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT o.source_root_id, o.parent_relative_path, o.normalized_name,
                        o.size_bytes, o.scan_status, c.sha256
                 FROM path_occurrences o
                 LEFT JOIN occurrence_content oc ON oc.occurrence_id = o.id
                 LEFT JOIN content_objects c ON c.id = oc.content_id
                 WHERE o.snapshot_id = ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([&snapshot], |row| {
                Ok(FileRow {
                    source_root_id: row.get(0)?,
                    parent_relative_path: row.get(1)?,
                    normalized_name: row.get(2)?,
                    size_bytes: row.get(3)?,
                    scan_status: row.get(4)?,
                    sha256: row.get(5)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows
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
        let mut complete = true;
        let mut subtree_files: u64 = 0;
        let mut subtree_bytes: u64 = 0;

        if let Some(children) =
            files_by_parent.get(&(folder.source_root_id.clone(), folder.relative_path.clone()))
        {
            for file in children {
                subtree_files += 1;
                subtree_bytes += file.size_bytes.max(0) as u64;
                match (&file.sha256, file.scan_status.as_str()) {
                    (Some(sha), "OK") => {
                        entries.push(format!("F\0{}\0{}", file.normalized_name, sha));
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
                match (&child_computed.signature, child_computed.is_complete) {
                    (Some(sig), true) => {
                        entries.push(format!("D\0{}\0{}", child.normalized_name, sig));
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
        db.conn()
            .execute(
                "INSERT INTO folders
                    (id, snapshot_id, source_root_id, relative_path,
                     parent_relative_path, name, normalized_name, depth, status,
                     created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'OK','t')",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    s.snapshot.to_string(),
                    s.root.to_string(),
                    rel,
                    parent,
                    name,
                    name.to_lowercase(),
                    rel.matches(['/', '\\']).count() as i64 + if rel.is_empty() { 0 } else { 1 },
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
}
