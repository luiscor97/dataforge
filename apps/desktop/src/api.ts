import { invoke } from "@tauri-apps/api/core";

import {
  type AnalyzeOutcome,
  type ApproveOutcome,
  type ContentArtifactBuildOutcome,
  type ContentExtractionOutcome,
  type ContentQueryOutcome,
  type ContentSearchOutcome,
  type ContentSearchRequest,
  type CreateProjectRequest,
  type DestinationGuarantees,
  type ErrorDto,
  type ExecuteOutcome,
  type HashOutcome,
  type MediaOutcome,
  type PlanOutcome,
  type PlanValidationReport,
  type ProjectStatus,
  type ScanOutcome,
  type SimilarityOutcome,
  type VerifyOutcome,
  isErrorDto,
} from "./types";

/** Normalise unknown invoke failures into an ErrorDto. */
function toErrorDto(error: unknown): ErrorDto {
  if (isErrorDto(error)) {
    return error;
  }
  return { code: "unknown", message: String(error) };
}

async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toErrorDto(error);
  }
}

export function createProject(
  request: CreateProjectRequest,
): Promise<ProjectStatus> {
  return call<ProjectStatus>("create_project", { request });
}

// --- The reconstruction pipeline -----------------------------------------
// Each call is one stage of RFC-0001's flow. The guided experience chains
// them so the user never has to know they exist.

export function scanProject(projectDir: string): Promise<ScanOutcome> {
  return call<ScanOutcome>("scan_project", { projectDir });
}

/** Live scan counters, cheap to poll while scanProject runs. */
export interface ScanProgress {
  files: number;
  folders: number;
  bytes: number;
}

export function scanProgress(projectDir: string): Promise<ScanProgress> {
  return call<ScanProgress>("scan_progress", { projectDir });
}

export function hashProject(projectDir: string): Promise<HashOutcome> {
  return call<HashOutcome>("hash_project", { projectDir });
}

export function analyzeProject(projectDir: string): Promise<AnalyzeOutcome> {
  return call<AnalyzeOutcome>("analyze_project", { projectDir });
}

export function createPlan(projectDir: string): Promise<PlanOutcome> {
  return call<PlanOutcome>("create_plan", { projectDir });
}

export function validatePlan(
  projectDir: string,
): Promise<PlanValidationReport> {
  return call<PlanValidationReport>("validate_plan", { projectDir });
}

export function approvePlan(projectDir: string): Promise<ApproveOutcome> {
  return call<ApproveOutcome>("approve_plan", { projectDir });
}

export function destinationGuarantees(
  projectDir: string,
): Promise<DestinationGuarantees> {
  return call<DestinationGuarantees>("destination_guarantees", { projectDir });
}

export function executePlan(projectDir: string): Promise<ExecuteOutcome> {
  return call<ExecuteOutcome>("execute_plan", { projectDir });
}

/**
 * Copy towards a volume without physical identity guarantees (ADR-0036).
 *
 * A separate function rather than a flag: the acknowledgement is a decision
 * the user has to have made, so it must be visible at every call site.
 */
export function executePlanOnDegradedDestination(
  projectDir: string,
): Promise<ExecuteOutcome> {
  return call<ExecuteOutcome>("execute_plan_on_degraded_destination", {
    projectDir,
  });
}

export function verifyProject(projectDir: string): Promise<VerifyOutcome> {
  return call<VerifyOutcome>("verify_project", { projectDir });
}

export function openProject(projectDir: string): Promise<ProjectStatus> {
  return call<ProjectStatus>("open_project", { projectDir });
}

export function projectStatus(projectDir: string): Promise<ProjectStatus> {
  return call<ProjectStatus>("project_status", { projectDir });
}

export function analyzeSimilarity(
  projectDir: string,
): Promise<SimilarityOutcome> {
  return call<SimilarityOutcome>("analyze_similarity", { projectDir });
}

export function analyzeMedia(projectDir: string): Promise<MediaOutcome> {
  return call<MediaOutcome>("analyze_media", { projectDir });
}

export function extractContent(
  projectDir: string,
): Promise<ContentExtractionOutcome> {
  return call<ContentExtractionOutcome>("extract_content", { projectDir });
}

export function failContentExtraction(
  projectDir: string,
  runId: string,
  reason: string,
): Promise<ContentExtractionOutcome> {
  return call<ContentExtractionOutcome>("fail_content_extraction", {
    projectDir,
    runId,
    reason,
  });
}

export function buildContentArtifacts(
  projectDir: string,
  runId: string | null,
): Promise<ContentArtifactBuildOutcome> {
  return call<ContentArtifactBuildOutcome>("build_content_artifacts", {
    projectDir,
    runId,
  });
}

export function searchContent(
  projectDir: string,
  runId: string | null,
  request: ContentSearchRequest,
): Promise<ContentSearchOutcome> {
  return call<ContentSearchOutcome>("search_content", {
    projectDir,
    runId,
    request,
  });
}

export function queryContent(
  projectDir: string,
  runId: string | null,
  sql: string,
): Promise<ContentQueryOutcome> {
  return call<ContentQueryOutcome>("query_content", { projectDir, runId, sql });
}

export function engineVersion(): Promise<string> {
  return call<string>("engine_version");
}
