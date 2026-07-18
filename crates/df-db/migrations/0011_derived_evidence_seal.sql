-- Migration 0011 — seal every automatic derived-evidence table (M0.2).
--
-- `analysis_completions` is the final contract for one immutable snapshot.
-- Once any completion exists for that snapshot, neither an interrupted replay
-- nor a direct SQLite writer may add, rewrite, move or delete evidence behind
-- the recorded summary. Human review decisions intentionally remain outside
-- this seal: they are the append-only post-analysis decision stream.

CREATE TRIGGER duplicate_sets_sealed_insert
BEFORE INSERT ON duplicate_sets
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: duplicate_sets');
END;

CREATE TRIGGER duplicate_sets_sealed_update
BEFORE UPDATE ON duplicate_sets
WHEN EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
     )
   OR EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
     )
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: duplicate_sets');
END;

CREATE TRIGGER duplicate_sets_sealed_delete
BEFORE DELETE ON duplicate_sets
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: duplicate_sets');
END;

CREATE TRIGGER folder_signatures_sealed_insert
BEFORE INSERT ON folder_signatures
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: folder_signatures');
END;

CREATE TRIGGER folder_signatures_sealed_update
BEFORE UPDATE ON folder_signatures
WHEN EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
     )
   OR EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
     )
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: folder_signatures');
END;

CREATE TRIGGER folder_signatures_sealed_delete
BEFORE DELETE ON folder_signatures
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: folder_signatures');
END;

CREATE TRIGGER tree_clone_sets_sealed_insert
BEFORE INSERT ON tree_clone_sets
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: tree_clone_sets');
END;

CREATE TRIGGER tree_clone_sets_sealed_update
BEFORE UPDATE ON tree_clone_sets
WHEN EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
     )
   OR EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
     )
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: tree_clone_sets');
END;

CREATE TRIGGER tree_clone_sets_sealed_delete
BEFORE DELETE ON tree_clone_sets
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: tree_clone_sets');
END;

CREATE TRIGGER folder_contexts_sealed_insert
BEFORE INSERT ON folder_contexts
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: folder_contexts');
END;

CREATE TRIGGER folder_contexts_sealed_update
BEFORE UPDATE ON folder_contexts
WHEN EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
     )
   OR EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
     )
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: folder_contexts');
END;

CREATE TRIGGER folder_contexts_sealed_delete
BEFORE DELETE ON folder_contexts
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: folder_contexts');
END;

CREATE TRIGGER tree_relations_sealed_insert
BEFORE INSERT ON tree_relations
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: tree_relations');
END;

CREATE TRIGGER tree_relations_sealed_update
BEFORE UPDATE ON tree_relations
WHEN EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
     )
   OR EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
     )
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: tree_relations');
END;

CREATE TRIGGER tree_relations_sealed_delete
BEFORE DELETE ON tree_relations
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: tree_relations');
END;

CREATE TRIGGER duplicate_representatives_sealed_insert
BEFORE INSERT ON duplicate_representatives
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: duplicate_representatives');
END;

CREATE TRIGGER duplicate_representatives_sealed_update
BEFORE UPDATE ON duplicate_representatives
WHEN EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
     )
   OR EXISTS (
         SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = NEW.snapshot_id
     )
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: duplicate_representatives');
END;

CREATE TRIGGER duplicate_representatives_sealed_delete
BEFORE DELETE ON duplicate_representatives
WHEN EXISTS (
    SELECT 1 FROM analysis_completions c WHERE c.snapshot_id = OLD.snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'completed derived evidence is sealed: duplicate_representatives');
END;
