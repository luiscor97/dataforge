import { invoke } from "@tauri-apps/api/core";

import {
  type ContentArtifactBuildOutcome,
  type ContentExtractionOutcome,
  type ContentQueryOutcome,
  type ContentSearchOutcome,
  type ContentSearchRequest,
  type CreateProjectRequest,
  type ErrorDto,
  type MediaOutcome,
  type ProjectStatus,
  type SimilarityOutcome,
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
