import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  buildContentArtifacts,
  extractContent,
  failContentExtraction,
  queryContent,
  searchContent,
} from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue({});
});

describe("M0.4 Tauri command contract", () => {
  test("maps extraction and explicit failure arguments", async () => {
    await extractContent("C:\\project");
    expect(invoke).toHaveBeenLastCalledWith("extract_content", {
      projectDir: "C:\\project",
    });

    await failContentExtraction("C:\\project", "run-1", "origen irrecuperable");
    expect(invoke).toHaveBeenLastCalledWith("fail_content_extraction", {
      projectDir: "C:\\project",
      runId: "run-1",
      reason: "origen irrecuperable",
    });
  });

  test("maps build, search and query without changing their values", async () => {
    await buildContentArtifacts("C:\\project", null);
    expect(invoke).toHaveBeenLastCalledWith("build_content_artifacts", {
      projectDir: "C:\\project",
      runId: null,
    });

    const request = {
      query: "contrato final",
      limit: 25,
      offset: 0,
      snippet_chars: 320,
    };
    await searchContent("C:\\project", "run-1", request);
    expect(invoke).toHaveBeenLastCalledWith("search_content", {
      projectDir: "C:\\project",
      runId: "run-1",
      request,
    });

    await queryContent("C:\\project", "run-1", "SELECT * FROM content LIMIT 1");
    expect(invoke).toHaveBeenLastCalledWith("query_content", {
      projectDir: "C:\\project",
      runId: "run-1",
      sql: "SELECT * FROM content LIMIT 1",
    });
  });
});
