/** DTOs mirrored from `df-facade` (crates/df-facade/src/lib.rs). */

export interface CreateProjectRequest {
  name: string;
  project_dir: string;
  output_root: string;
  audit_root?: string | null;
  source_roots: string[];
  profile?: string | null;
}

export interface SourceRootView {
  id: string;
  absolute_path: string;
  filesystem: string;
  read_only_policy: boolean;
}

export interface EventView {
  sequence: number;
  event_type: string;
  timestamp: string;
  actor: string;
}

export interface IntegrityReport {
  database_ok: boolean;
  foreign_keys_ok: boolean;
  migrations_ok: boolean;
  ledger_ok: boolean;
  problems: string[];
}

export interface InventorySummary {
  files: number;
  folders: number;
  bytes: number;
  scan_errors: number;
  reparse_points: number;
  hash_pending: number;
  hash_done: number;
  hash_failed: number;
  hash_source_changed: number;
}

/** Compact M0.2 evidence summary for the latest complete snapshot. */
export interface StructuralDiagnostics {
  analysis_complete: boolean;
  folder_signatures: number;
  exact_tree_clone_sets: number;
  partial_tree_clones: number;
  embedded_trees: number;
  repeated_components: number;
  candidate_cap_reached: boolean;
  generic_folders: number;
  protected_boundaries: number;
  rule_matches: number;
  anomalies: number;
  high_anomalies: number;
  pending_review: number;
}

export interface SimilarityCounters {
  contents_total: number;
  contents_chunked: number;
  contents_skipped: number;
  chunks_total: number;
  candidates_total: number;
  relations_total: number;
}

export interface SimilarityRelationView {
  id: string;
  content_a: string;
  content_b: string;
  path_a: string;
  path_b: string;
  kind:
    | "LIKELY_VERSION"
    | "TRUNCATED_VARIANT"
    | "RECOMPOSED_CONTENT"
    | "SIMILAR_CONTENT";
  direction: "A_TO_B" | "B_TO_A" | "UNKNOWN";
  similarity: number;
  shared_chunks: number;
  shared_bytes: number;
  union_bytes: number;
  estimated_similarity: number;
  confidence: number;
  evidence: unknown;
}

export interface SimilarityOptions {
  min_chunk_bytes: number;
  avg_chunk_bytes: number;
  max_chunk_bytes: number;
  min_file_bytes: number;
  threshold: number;
  min_shared_chunks: number;
  min_shared_bytes: number;
  minhash_permutations: number;
  lsh_bands: number;
  max_bucket_contents: number;
  max_candidates: number;
}

export interface SimilarityConfiguration {
  algorithm_version: string;
  options: SimilarityOptions;
}

export interface SimilarityStatus {
  run_id: string;
  snapshot_id: string;
  algorithm_version: string;
  config_digest: string;
  config: SimilarityConfiguration;
  counters: SimilarityCounters;
  candidate_cap_reached: boolean;
  relationships: SimilarityRelationView[];
  relationships_truncated: boolean;
}

export interface SimilarityOutcome {
  run_id: string;
  snapshot_id: string;
  status: string;
  algorithm_version: string;
  config_digest: string;
  config: SimilarityConfiguration;
  counters: SimilarityCounters;
  candidate_cap_reached: boolean;
  cancelled: boolean;
}

export interface ContentExtractionCounters {
  contents_total: number;
  extracted: number;
  unsupported: number;
  limited: number;
  failed: number;
  text_subjects: number;
  text_segments: number;
  mail_messages: number;
  mail_threads: number;
  mail_attachments: number;
  archive_entries: number;
}

export interface ContentExtractionOutcome {
  run_id: string;
  snapshot_id: string;
  status: string;
  extractor_version: string;
  config_digest: string;
  counters: ContentExtractionCounters;
  processed_this_invocation: number;
  reused_this_invocation: number;
  threads_built_this_invocation: number;
  error: string | null;
}

export interface SearchIndexView {
  id: string;
  run_id: string;
  snapshot_id: string;
  schema_version: string;
  relative_path: string;
  content_digest: string;
  documents: number;
  created_at: string;
}

export interface AnalyticalSnapshotView {
  id: string;
  run_id: string;
  snapshot_id: string;
  schema_version: string;
  relative_path: string;
  sha256: string;
  rows: number;
  created_at: string;
}

export interface ContentArtifactBuildOutcome {
  run_id: string;
  search_index: SearchIndexView;
  analytical_snapshot: AnalyticalSnapshotView;
}

export interface ContentSearchRequest {
  query: string;
  limit: number;
  offset: number;
  snippet_chars: number;
}

export interface ContentSearchHit {
  subject_id: string;
  content_id: string;
  score: number;
  file_name: string;
  relative_path: string;
  representative_path: string;
  virtual_path: string | null;
  subject: string;
  context: string;
  mime: string;
  snippet: string;
}

export interface ContentSearchOutcome {
  run_id: string;
  index: SearchIndexView;
  query: string;
  hits: ContentSearchHit[];
}

export interface QueryColumn {
  name: string;
  data_type: string;
}

export interface QueryResult {
  columns: QueryColumn[];
  rows: Array<Array<string | null>>;
}

export interface ContentQueryOutcome {
  run_id: string;
  snapshot: AnalyticalSnapshotView;
  result: QueryResult;
}

export interface ProjectStatus {
  project_id: string;
  name: string;
  state: string;
  profile: string;
  app_version: string;
  created_at: string;
  updated_at: string;
  project_dir: string;
  output_root: string;
  audit_root: string;
  source_roots: SourceRootView[];
  event_count: number;
  last_event: EventView | null;
  latest_snapshot_id: string | null;
  inventory: InventorySummary | null;
  structural_diagnostics: StructuralDiagnostics | null;
  similarity: SimilarityStatus | null;
  integrity: IntegrityReport | null;
}

export interface ErrorDto {
  code: string;
  message: string;
}

export function isErrorDto(value: unknown): value is ErrorDto {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ErrorDto).code === "string" &&
    typeof (value as ErrorDto).message === "string"
  );
}
