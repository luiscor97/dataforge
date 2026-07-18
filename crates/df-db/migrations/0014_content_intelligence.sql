-- Migration 0014 — document extraction, mail/archive evidence and rebuildable
-- search/analytical artifact registry (Milestone 0.4).
--
-- SQLite remains the transactional source of truth. Normalized text is
-- segmented and bounded here so Tantivy and Parquet can always be rebuilt.
-- No binary source or attachment bytes are persisted.

CREATE TABLE extraction_runs (
    id                       TEXT PRIMARY KEY,
    project_id               TEXT NOT NULL REFERENCES projects(id),
    snapshot_id              TEXT NOT NULL REFERENCES snapshots(id),
    status                   TEXT NOT NULL CHECK (status IN ('RUNNING', 'COMPLETED', 'FAILED')),
    extractor_version        TEXT NOT NULL CHECK (length(extractor_version) > 0),
    config_digest            TEXT NOT NULL CHECK (length(config_digest) = 64),
    config_json              TEXT NOT NULL CHECK (json_valid(config_json)),
    max_input_bytes          INTEGER NOT NULL CHECK (max_input_bytes >= 1),
    max_text_chars           INTEGER NOT NULL CHECK (max_text_chars >= 1),
    text_segment_chars       INTEGER NOT NULL CHECK (text_segment_chars >= 1),
    max_archive_entries      INTEGER NOT NULL CHECK (max_archive_entries >= 1),
    max_archive_entry_bytes  INTEGER NOT NULL CHECK (max_archive_entry_bytes >= 1),
    max_archive_total_bytes  INTEGER NOT NULL CHECK (max_archive_total_bytes >= max_archive_entry_bytes),
    max_archive_ratio        REAL NOT NULL CHECK (max_archive_ratio >= 1.0),
    max_archive_depth        INTEGER NOT NULL CHECK (max_archive_depth >= 1),
    contents_total           INTEGER NOT NULL DEFAULT 0 CHECK (contents_total >= 0),
    extracted                INTEGER NOT NULL DEFAULT 0 CHECK (extracted >= 0),
    unsupported              INTEGER NOT NULL DEFAULT 0 CHECK (unsupported >= 0),
    limited                  INTEGER NOT NULL DEFAULT 0 CHECK (limited >= 0),
    failed                   INTEGER NOT NULL DEFAULT 0 CHECK (failed >= 0),
    text_subjects            INTEGER NOT NULL DEFAULT 0 CHECK (text_subjects >= 0),
    text_segments            INTEGER NOT NULL DEFAULT 0 CHECK (text_segments >= 0),
    mail_messages            INTEGER NOT NULL DEFAULT 0 CHECK (mail_messages >= 0),
    mail_threads             INTEGER NOT NULL DEFAULT 0 CHECK (mail_threads >= 0),
    mail_attachments         INTEGER NOT NULL DEFAULT 0 CHECK (mail_attachments >= 0),
    archive_entries          INTEGER NOT NULL DEFAULT 0 CHECK (archive_entries >= 0),
    error                    TEXT,
    started_at               TEXT NOT NULL,
    finished_at              TEXT,
    created_at               TEXT NOT NULL,
    UNIQUE (snapshot_id, extractor_version, config_digest),
    CHECK (
        (status = 'RUNNING' AND finished_at IS NULL AND error IS NULL)
        OR (status = 'COMPLETED' AND finished_at IS NOT NULL AND error IS NULL)
        OR (status = 'FAILED' AND finished_at IS NOT NULL AND error IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_extraction_runs_project_snapshot
    ON extraction_runs(project_id, snapshot_id, status);

CREATE TRIGGER extraction_runs_validate_insert
BEFORE INSERT ON extraction_runs
WHEN NEW.status <> 'RUNNING'
  OR NEW.extracted <> 0 OR NEW.unsupported <> 0 OR NEW.limited <> 0
  OR NEW.failed <> 0 OR NEW.text_subjects <> 0 OR NEW.text_segments <> 0
  OR NEW.mail_messages <> 0 OR NEW.mail_threads <> 0
  OR NEW.mail_attachments <> 0 OR NEW.archive_entries <> 0
  OR NEW.contents_total <> (
      SELECT COUNT(DISTINCT oc.content_id)
      FROM occurrence_content oc
      JOIN path_occurrences o ON o.id = oc.occurrence_id
      JOIN content_objects c ON c.id = oc.content_id
      WHERE o.snapshot_id = NEW.snapshot_id AND o.scan_status = 'OK'
        AND c.hash_state = 'HASHED' AND c.sha256 IS NOT NULL
  )
  OR NOT EXISTS (
    SELECT 1 FROM snapshots s
    JOIN analysis_completions a ON a.snapshot_id = s.id
    WHERE s.id = NEW.snapshot_id AND s.project_id = NEW.project_id
      AND s.status = 'COMPLETE'
)
BEGIN SELECT RAISE(ABORT, 'extraction requires an exact running manifest over a completed analysed snapshot'); END;

CREATE TABLE document_representations (
    id                       TEXT PRIMARY KEY,
    content_id               TEXT NOT NULL REFERENCES content_objects(id),
    extractor_version        TEXT NOT NULL,
    config_digest            TEXT NOT NULL CHECK (length(config_digest) = 64),
    format                   TEXT NOT NULL CHECK (format IN ('PDF', 'DOCX', 'TXT', 'HTML', 'EML', 'ZIP', 'UNSUPPORTED')),
    mime                     TEXT NOT NULL CHECK (length(mime) > 0),
    status                   TEXT NOT NULL CHECK (status IN ('EXTRACTED', 'UNSUPPORTED', 'LIMITED', 'FAILED')),
    title                    TEXT,
    normalized_text_sha256   TEXT CHECK (normalized_text_sha256 IS NULL OR length(normalized_text_sha256) = 64),
    normalized_chars         INTEGER NOT NULL CHECK (normalized_chars >= 0),
    text_truncated           INTEGER NOT NULL CHECK (text_truncated IN (0, 1)),
    metadata_json            TEXT NOT NULL CHECK (json_valid(metadata_json)),
    error                    TEXT,
    source_sha256            TEXT NOT NULL CHECK (length(source_sha256) = 64),
    created_at               TEXT NOT NULL,
    UNIQUE (content_id, extractor_version, config_digest),
    UNIQUE (id, content_id),
    CHECK (
        (status = 'EXTRACTED' AND error IS NULL AND format <> 'UNSUPPORTED')
        OR (status = 'UNSUPPORTED' AND error IS NULL AND format = 'UNSUPPORTED')
        OR (status IN ('LIMITED', 'FAILED') AND error IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_document_representations_content
    ON document_representations(content_id, extractor_version, config_digest);

CREATE TABLE extraction_run_contents (
    run_id             TEXT NOT NULL REFERENCES extraction_runs(id),
    content_id         TEXT NOT NULL REFERENCES content_objects(id),
    representation_id TEXT NOT NULL,
    status             TEXT NOT NULL CHECK (status IN ('EXTRACTED', 'UNSUPPORTED', 'LIMITED', 'FAILED')),
    created_at         TEXT NOT NULL,
    PRIMARY KEY (run_id, content_id),
    FOREIGN KEY (representation_id, content_id)
        REFERENCES document_representations(id, content_id)
) STRICT;

CREATE TABLE text_subjects (
    id                       TEXT PRIMARY KEY,
    representation_id        TEXT NOT NULL REFERENCES document_representations(id),
    kind                     TEXT NOT NULL CHECK (kind IN ('DOCUMENT', 'MAIL_ATTACHMENT', 'ARCHIVE_ENTRY')),
    parent_subject_id        TEXT REFERENCES text_subjects(id),
    display_name             TEXT NOT NULL CHECK (length(display_name) > 0),
    virtual_path             TEXT,
    mime                     TEXT NOT NULL CHECK (length(mime) > 0),
    size_bytes               INTEGER NOT NULL CHECK (size_bytes >= 0),
    normalized_text_sha256   TEXT CHECK (normalized_text_sha256 IS NULL OR length(normalized_text_sha256) = 64),
    normalized_chars         INTEGER NOT NULL CHECK (normalized_chars >= 0),
    text_truncated           INTEGER NOT NULL CHECK (text_truncated IN (0, 1)),
    metadata_json            TEXT NOT NULL CHECK (json_valid(metadata_json)),
    created_at               TEXT NOT NULL,
    UNIQUE (representation_id, kind, virtual_path),
    CHECK (
        (kind = 'DOCUMENT' AND parent_subject_id IS NULL AND virtual_path IS NULL)
        OR (kind <> 'DOCUMENT' AND parent_subject_id IS NOT NULL AND virtual_path IS NOT NULL)
    )
) STRICT;

CREATE INDEX idx_text_subjects_representation
    ON text_subjects(representation_id, kind, id);

CREATE TABLE text_segments (
    subject_id    TEXT NOT NULL REFERENCES text_subjects(id),
    ordinal       INTEGER NOT NULL CHECK (ordinal >= 0),
    char_start    INTEGER NOT NULL CHECK (char_start >= 0),
    char_end      INTEGER NOT NULL CHECK (char_end >= char_start),
    text          TEXT NOT NULL,
    text_sha256   TEXT NOT NULL CHECK (length(text_sha256) = 64),
    created_at    TEXT NOT NULL,
    PRIMARY KEY (subject_id, ordinal),
    UNIQUE (subject_id, char_start),
    CHECK (char_end - char_start = length(text))
) STRICT;

CREATE TABLE mail_messages (
    representation_id TEXT PRIMARY KEY REFERENCES document_representations(id),
    message_id         TEXT,
    in_reply_to_json   TEXT NOT NULL CHECK (json_valid(in_reply_to_json)),
    references_json    TEXT NOT NULL CHECK (json_valid(references_json)),
    from_json          TEXT NOT NULL CHECK (json_valid(from_json)),
    to_json            TEXT NOT NULL CHECK (json_valid(to_json)),
    cc_json            TEXT NOT NULL CHECK (json_valid(cc_json)),
    sent_at            TEXT,
    subject            TEXT,
    normalized_subject TEXT,
    body_sha256        TEXT CHECK (body_sha256 IS NULL OR length(body_sha256) = 64),
    created_at         TEXT NOT NULL
) STRICT;

CREATE INDEX idx_mail_messages_message_id
    ON mail_messages(message_id) WHERE message_id IS NOT NULL;
CREATE INDEX idx_mail_messages_subject
    ON mail_messages(normalized_subject) WHERE normalized_subject IS NOT NULL;

CREATE TABLE mail_attachments (
    id                  TEXT PRIMARY KEY,
    representation_id   TEXT NOT NULL REFERENCES document_representations(id),
    subject_id          TEXT NOT NULL UNIQUE REFERENCES text_subjects(id),
    ordinal             INTEGER NOT NULL CHECK (ordinal >= 0),
    file_name           TEXT NOT NULL CHECK (length(file_name) > 0),
    mime                TEXT NOT NULL CHECK (length(mime) > 0),
    size_bytes          INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256              TEXT NOT NULL CHECK (length(sha256) = 64),
    extraction_status   TEXT NOT NULL CHECK (extraction_status IN ('EXTRACTED', 'UNSUPPORTED', 'LIMITED', 'FAILED')),
    created_at          TEXT NOT NULL,
    UNIQUE (representation_id, ordinal)
) STRICT;

CREATE TABLE archive_entries (
    id                  TEXT PRIMARY KEY,
    representation_id   TEXT NOT NULL REFERENCES document_representations(id),
    subject_id          TEXT UNIQUE REFERENCES text_subjects(id),
    ordinal             INTEGER NOT NULL CHECK (ordinal >= 0),
    virtual_path        TEXT NOT NULL CHECK (length(virtual_path) > 0),
    compressed_bytes    INTEGER NOT NULL CHECK (compressed_bytes >= 0),
    size_bytes          INTEGER NOT NULL CHECK (size_bytes >= 0),
    crc32               INTEGER NOT NULL CHECK (crc32 >= 0 AND crc32 <= 4294967295),
    encrypted           INTEGER NOT NULL CHECK (encrypted IN (0, 1)),
    directory           INTEGER NOT NULL CHECK (directory IN (0, 1)),
    sha256              TEXT CHECK (sha256 IS NULL OR length(sha256) = 64),
    extraction_status   TEXT NOT NULL CHECK (extraction_status IN ('EXTRACTED', 'UNSUPPORTED', 'LIMITED', 'FAILED')),
    created_at          TEXT NOT NULL,
    UNIQUE (representation_id, ordinal),
    UNIQUE (representation_id, virtual_path),
    CHECK ((directory = 1 AND subject_id IS NULL AND sha256 IS NULL) OR directory = 0)
) STRICT;

CREATE TABLE mail_threads (
    id               TEXT PRIMARY KEY,
    run_id           TEXT NOT NULL REFERENCES extraction_runs(id),
    snapshot_id      TEXT NOT NULL REFERENCES snapshots(id),
    root_message_id  TEXT,
    normalized_subject TEXT,
    message_count    INTEGER NOT NULL CHECK (message_count >= 1),
    created_at       TEXT NOT NULL,
    UNIQUE (id, run_id)
) STRICT;

CREATE TABLE mail_thread_members (
    thread_id               TEXT NOT NULL REFERENCES mail_threads(id),
    run_id                  TEXT NOT NULL REFERENCES extraction_runs(id),
    representation_id       TEXT NOT NULL REFERENCES mail_messages(representation_id),
    parent_representation_id TEXT REFERENCES mail_messages(representation_id),
    ordinal                 INTEGER NOT NULL CHECK (ordinal >= 0),
    created_at              TEXT NOT NULL,
    PRIMARY KEY (thread_id, representation_id),
    FOREIGN KEY (thread_id, run_id) REFERENCES mail_threads(id, run_id),
    UNIQUE (thread_id, ordinal),
    UNIQUE (run_id, representation_id),
    CHECK (parent_representation_id IS NULL OR parent_representation_id <> representation_id)
) STRICT;

CREATE TABLE search_indexes (
    id              TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES extraction_runs(id),
    snapshot_id     TEXT NOT NULL REFERENCES snapshots(id),
    schema_version  TEXT NOT NULL CHECK (length(schema_version) > 0),
    relative_path   TEXT NOT NULL CHECK (length(relative_path) > 0),
    content_digest  TEXT NOT NULL CHECK (length(content_digest) = 64),
    documents       INTEGER NOT NULL CHECK (documents >= 0),
    created_at      TEXT NOT NULL,
    UNIQUE (relative_path),
    UNIQUE (run_id, schema_version, content_digest)
) STRICT;

CREATE INDEX idx_search_indexes_run
    ON search_indexes(run_id, created_at DESC, id DESC);

CREATE TABLE analytical_snapshots (
    id              TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES extraction_runs(id),
    snapshot_id     TEXT NOT NULL REFERENCES snapshots(id),
    schema_version  TEXT NOT NULL CHECK (length(schema_version) > 0),
    relative_path   TEXT NOT NULL CHECK (length(relative_path) > 0),
    sha256          TEXT NOT NULL CHECK (length(sha256) = 64),
    rows            INTEGER NOT NULL CHECK (rows >= 0),
    created_at      TEXT NOT NULL,
    UNIQUE (relative_path),
    UNIQUE (run_id, schema_version, sha256)
) STRICT;

CREATE INDEX idx_analytical_snapshots_run
    ON analytical_snapshots(run_id, created_at DESC, id DESC);

-- Global extraction evidence is immutable. A parser/configuration change
-- receives a new version/digest instead of reinterpreting old evidence.
CREATE TRIGGER document_representations_no_update BEFORE UPDATE ON document_representations
BEGIN SELECT RAISE(ABORT, 'document representations are append-only'); END;
CREATE TRIGGER document_representations_no_delete BEFORE DELETE ON document_representations
BEGIN SELECT RAISE(ABORT, 'document representations are append-only'); END;
CREATE TRIGGER extraction_run_contents_no_update BEFORE UPDATE ON extraction_run_contents
BEGIN SELECT RAISE(ABORT, 'extraction run contents are append-only'); END;
CREATE TRIGGER extraction_run_contents_no_delete BEFORE DELETE ON extraction_run_contents
BEGIN SELECT RAISE(ABORT, 'extraction run contents are append-only'); END;
CREATE TRIGGER text_subjects_no_update BEFORE UPDATE ON text_subjects
BEGIN SELECT RAISE(ABORT, 'text subjects are append-only'); END;
CREATE TRIGGER text_subjects_no_delete BEFORE DELETE ON text_subjects
BEGIN SELECT RAISE(ABORT, 'text subjects are append-only'); END;
CREATE TRIGGER text_segments_no_update BEFORE UPDATE ON text_segments
BEGIN SELECT RAISE(ABORT, 'text segments are append-only'); END;
CREATE TRIGGER text_segments_no_delete BEFORE DELETE ON text_segments
BEGIN SELECT RAISE(ABORT, 'text segments are append-only'); END;
CREATE TRIGGER mail_messages_no_update BEFORE UPDATE ON mail_messages
BEGIN SELECT RAISE(ABORT, 'mail messages are append-only'); END;
CREATE TRIGGER mail_messages_no_delete BEFORE DELETE ON mail_messages
BEGIN SELECT RAISE(ABORT, 'mail messages are append-only'); END;
CREATE TRIGGER mail_attachments_no_update BEFORE UPDATE ON mail_attachments
BEGIN SELECT RAISE(ABORT, 'mail attachments are append-only'); END;
CREATE TRIGGER mail_attachments_no_delete BEFORE DELETE ON mail_attachments
BEGIN SELECT RAISE(ABORT, 'mail attachments are append-only'); END;
CREATE TRIGGER archive_entries_no_update BEFORE UPDATE ON archive_entries
BEGIN SELECT RAISE(ABORT, 'archive entries are append-only'); END;
CREATE TRIGGER archive_entries_no_delete BEFORE DELETE ON archive_entries
BEGIN SELECT RAISE(ABORT, 'archive entries are append-only'); END;
CREATE TRIGGER mail_threads_no_update BEFORE UPDATE ON mail_threads
BEGIN SELECT RAISE(ABORT, 'mail threads are append-only'); END;
CREATE TRIGGER mail_threads_no_delete BEFORE DELETE ON mail_threads
BEGIN SELECT RAISE(ABORT, 'mail threads are append-only'); END;
CREATE TRIGGER mail_thread_members_no_update BEFORE UPDATE ON mail_thread_members
BEGIN SELECT RAISE(ABORT, 'mail thread members are append-only'); END;
CREATE TRIGGER mail_thread_members_no_delete BEFORE DELETE ON mail_thread_members
BEGIN SELECT RAISE(ABORT, 'mail thread members are append-only'); END;
CREATE TRIGGER search_indexes_no_update BEFORE UPDATE ON search_indexes
BEGIN SELECT RAISE(ABORT, 'search index registry is append-only'); END;
CREATE TRIGGER search_indexes_no_delete BEFORE DELETE ON search_indexes
BEGIN SELECT RAISE(ABORT, 'search index registry is append-only'); END;
CREATE TRIGGER analytical_snapshots_no_update BEFORE UPDATE ON analytical_snapshots
BEGIN SELECT RAISE(ABORT, 'analytical snapshot registry is append-only'); END;
CREATE TRIGGER analytical_snapshots_no_delete BEFORE DELETE ON analytical_snapshots
BEGIN SELECT RAISE(ABORT, 'analytical snapshot registry is append-only'); END;

CREATE TRIGGER document_representations_validate_insert
BEFORE INSERT ON document_representations
WHEN NOT EXISTS (
    SELECT 1 FROM content_objects c
    WHERE c.id = NEW.content_id AND c.hash_state = 'HASHED'
      AND c.sha256 = NEW.source_sha256
) OR NOT EXISTS (
    SELECT 1 FROM extraction_runs r
    JOIN occurrence_content oc ON oc.content_id = NEW.content_id
    JOIN path_occurrences o ON o.id = oc.occurrence_id
    WHERE r.status = 'RUNNING'
      AND r.extractor_version = NEW.extractor_version
      AND r.config_digest = NEW.config_digest
      AND o.snapshot_id = r.snapshot_id AND o.scan_status = 'OK'
)
BEGIN SELECT RAISE(ABORT, 'representation does not match canonical content'); END;

CREATE TRIGGER extraction_run_contents_validate_insert
BEFORE INSERT ON extraction_run_contents
WHEN NOT EXISTS (
        SELECT 1 FROM extraction_runs r
        JOIN occurrence_content oc ON oc.content_id = NEW.content_id
        JOIN path_occurrences o ON o.id = oc.occurrence_id
        WHERE r.id = NEW.run_id AND r.status = 'RUNNING'
          AND o.snapshot_id = r.snapshot_id AND o.scan_status = 'OK'
    )
    OR NOT EXISTS (
        SELECT 1 FROM document_representations d
        JOIN extraction_runs r ON r.id = NEW.run_id
        WHERE d.id = NEW.representation_id AND d.content_id = NEW.content_id
          AND d.status = NEW.status
          AND d.extractor_version = r.extractor_version
          AND d.config_digest = r.config_digest
    )
BEGIN SELECT RAISE(ABORT, 'run content is outside the running extraction snapshot'); END;

CREATE TRIGGER text_subjects_validate_insert
BEFORE INSERT ON text_subjects
WHEN (NEW.kind = 'DOCUMENT' AND EXISTS (
          SELECT 1 FROM text_subjects s
          WHERE s.representation_id = NEW.representation_id AND s.kind = 'DOCUMENT'
      ))
   OR (NEW.parent_subject_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM text_subjects p
          WHERE p.id = NEW.parent_subject_id
            AND p.representation_id = NEW.representation_id
      ))
   OR EXISTS (
          SELECT 1 FROM extraction_run_contents rc
          WHERE rc.representation_id = NEW.representation_id
      )
BEGIN SELECT RAISE(ABORT, 'invalid text subject lineage'); END;

CREATE TRIGGER text_segments_validate_insert
BEFORE INSERT ON text_segments
WHEN EXISTS (
    SELECT 1 FROM text_subjects s
    JOIN extraction_run_contents rc
      ON rc.representation_id = s.representation_id
    WHERE s.id = NEW.subject_id
)
BEGIN SELECT RAISE(ABORT, 'representation text evidence is already sealed'); END;

CREATE TRIGGER mail_messages_validate_insert
BEFORE INSERT ON mail_messages
WHEN NOT EXISTS (
    SELECT 1 FROM document_representations d
    WHERE d.id = NEW.representation_id AND d.format = 'EML'
) OR EXISTS (
    SELECT 1 FROM extraction_run_contents rc
    WHERE rc.representation_id = NEW.representation_id
)
BEGIN SELECT RAISE(ABORT, 'mail message requires an EML representation'); END;

CREATE TRIGGER mail_attachments_validate_insert
BEFORE INSERT ON mail_attachments
WHEN NOT EXISTS (
    SELECT 1 FROM text_subjects s
    WHERE s.id = NEW.subject_id AND s.representation_id = NEW.representation_id
      AND s.kind = 'MAIL_ATTACHMENT'
) OR EXISTS (
    SELECT 1 FROM extraction_run_contents rc
    WHERE rc.representation_id = NEW.representation_id
)
BEGIN SELECT RAISE(ABORT, 'mail attachment subject lineage mismatch'); END;

CREATE TRIGGER archive_entries_validate_insert
BEFORE INSERT ON archive_entries
WHEN NOT EXISTS (
        SELECT 1 FROM document_representations d
        WHERE d.id = NEW.representation_id AND d.format = 'ZIP'
    )
    OR (NEW.subject_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM text_subjects s
        WHERE s.id = NEW.subject_id AND s.representation_id = NEW.representation_id
          AND s.kind = 'ARCHIVE_ENTRY'
    ))
    OR EXISTS (
        SELECT 1 FROM extraction_run_contents rc
        WHERE rc.representation_id = NEW.representation_id
    )
BEGIN SELECT RAISE(ABORT, 'archive entry lineage mismatch'); END;

CREATE TRIGGER mail_threads_validate_insert
BEFORE INSERT ON mail_threads
WHEN NOT EXISTS (
    SELECT 1 FROM extraction_runs r
    WHERE r.id = NEW.run_id AND r.snapshot_id = NEW.snapshot_id
      AND r.status = 'RUNNING'
)
BEGIN SELECT RAISE(ABORT, 'mail thread requires a running extraction'); END;

CREATE TRIGGER mail_thread_members_validate_insert
BEFORE INSERT ON mail_thread_members
WHEN NOT EXISTS (
    SELECT 1 FROM mail_threads t
    JOIN extraction_runs r ON r.id = t.run_id
    JOIN extraction_run_contents rc
      ON rc.run_id = r.id AND rc.representation_id = NEW.representation_id
    WHERE t.id = NEW.thread_id AND t.run_id = NEW.run_id
      AND r.id = NEW.run_id AND r.status = 'RUNNING'
)
OR (NEW.parent_representation_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM mail_thread_members parent
    WHERE parent.thread_id = NEW.thread_id AND parent.run_id = NEW.run_id
      AND parent.representation_id = NEW.parent_representation_id
))
BEGIN SELECT RAISE(ABORT, 'mail thread member is outside its running extraction'); END;

CREATE TRIGGER search_indexes_validate_insert
BEFORE INSERT ON search_indexes
WHEN NOT EXISTS (
    SELECT 1 FROM extraction_runs r
    WHERE r.id = NEW.run_id AND r.snapshot_id = NEW.snapshot_id
      AND r.status = 'COMPLETED'
)
BEGIN SELECT RAISE(ABORT, 'search index requires a completed matching extraction'); END;

CREATE TRIGGER analytical_snapshots_validate_insert
BEFORE INSERT ON analytical_snapshots
WHEN NOT EXISTS (
    SELECT 1 FROM extraction_runs r
    WHERE r.id = NEW.run_id AND r.snapshot_id = NEW.snapshot_id
      AND r.status = 'COMPLETED'
)
BEGIN SELECT RAISE(ABORT, 'analytical snapshot requires a completed matching extraction'); END;

CREATE TRIGGER extraction_runs_guard_update
BEFORE UPDATE ON extraction_runs
WHEN OLD.status <> 'RUNNING'
  OR NEW.status = 'RUNNING'
  OR NEW.id <> OLD.id
  OR NEW.project_id <> OLD.project_id
  OR NEW.snapshot_id <> OLD.snapshot_id
  OR NEW.extractor_version <> OLD.extractor_version
  OR NEW.config_digest <> OLD.config_digest
  OR NEW.config_json <> OLD.config_json
  OR NEW.max_input_bytes <> OLD.max_input_bytes
  OR NEW.max_text_chars <> OLD.max_text_chars
  OR NEW.text_segment_chars <> OLD.text_segment_chars
  OR NEW.max_archive_entries <> OLD.max_archive_entries
  OR NEW.max_archive_entry_bytes <> OLD.max_archive_entry_bytes
  OR NEW.max_archive_total_bytes <> OLD.max_archive_total_bytes
  OR NEW.max_archive_ratio <> OLD.max_archive_ratio
  OR NEW.max_archive_depth <> OLD.max_archive_depth
  OR NEW.started_at <> OLD.started_at
  OR NEW.created_at <> OLD.created_at
BEGIN SELECT RAISE(ABORT, 'extraction run identity is immutable or already sealed'); END;

CREATE TRIGGER extraction_runs_validate_completion
BEFORE UPDATE ON extraction_runs
WHEN NEW.status = 'COMPLETED' AND (
       NEW.contents_total <> (
           SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = OLD.id
       )
    OR NEW.extracted <> (
           SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = OLD.id AND rc.status = 'EXTRACTED'
       )
    OR NEW.unsupported <> (
           SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = OLD.id AND rc.status = 'UNSUPPORTED'
       )
    OR NEW.limited <> (
           SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = OLD.id AND rc.status = 'LIMITED'
       )
    OR NEW.failed <> (
           SELECT COUNT(*) FROM extraction_run_contents rc WHERE rc.run_id = OLD.id AND rc.status = 'FAILED'
       )
    OR NEW.text_subjects <> (
           SELECT COUNT(*) FROM text_subjects s
           JOIN extraction_run_contents rc ON rc.representation_id = s.representation_id
           WHERE rc.run_id = OLD.id
       )
    OR NEW.text_segments <> (
           SELECT COUNT(*) FROM text_segments g
           JOIN text_subjects s ON s.id = g.subject_id
           JOIN extraction_run_contents rc ON rc.representation_id = s.representation_id
           WHERE rc.run_id = OLD.id
       )
    OR NEW.mail_messages <> (
           SELECT COUNT(*) FROM mail_messages m
           JOIN extraction_run_contents rc ON rc.representation_id = m.representation_id
           WHERE rc.run_id = OLD.id
       )
    OR NEW.mail_threads <> (
           SELECT COUNT(*) FROM mail_threads t WHERE t.run_id = OLD.id
       )
    OR NEW.mail_attachments <> (
           SELECT COUNT(*) FROM mail_attachments a
           JOIN extraction_run_contents rc ON rc.representation_id = a.representation_id
           WHERE rc.run_id = OLD.id
       )
    OR NEW.archive_entries <> (
           SELECT COUNT(*) FROM archive_entries a
           JOIN extraction_run_contents rc ON rc.representation_id = a.representation_id
           WHERE rc.run_id = OLD.id
       )
    OR EXISTS (
           SELECT 1 FROM mail_threads t
           WHERE t.run_id = OLD.id
             AND t.message_count <> (
                 SELECT COUNT(*) FROM mail_thread_members m
                 WHERE m.thread_id = t.id AND m.run_id = t.run_id
             )
       )
)
BEGIN SELECT RAISE(ABORT, 'extraction completion summary does not match evidence'); END;

CREATE TRIGGER extraction_runs_no_delete BEFORE DELETE ON extraction_runs
BEGIN SELECT RAISE(ABORT, 'extraction runs are append-only'); END;
