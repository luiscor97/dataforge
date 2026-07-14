import { invoke } from "@tauri-apps/api/core";

import {
  type CreateProjectRequest,
  type ErrorDto,
  type ProjectStatus,
  isErrorDto,
} from "./types";

/** Normalise unknown invoke failures into an ErrorDto. */
function toErrorDto(error: unknown): ErrorDto {
  if (isErrorDto(error)) {
    return error;
  }
  return { code: "unknown", message: String(error) };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toErrorDto(error);
  }
}

export function createProject(request: CreateProjectRequest): Promise<ProjectStatus> {
  return call<ProjectStatus>("create_project", { request });
}

export function openProject(projectDir: string): Promise<ProjectStatus> {
  return call<ProjectStatus>("open_project", { projectDir });
}

export function projectStatus(projectDir: string): Promise<ProjectStatus> {
  return call<ProjectStatus>("project_status", { projectDir });
}

export function engineVersion(): Promise<string> {
  return call<string>("engine_version");
}
