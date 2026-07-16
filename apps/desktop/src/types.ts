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
  generic_folders: number;
  protected_boundaries: number;
  rule_matches: number;
  anomalies: number;
  high_anomalies: number;
  pending_review: number;
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
