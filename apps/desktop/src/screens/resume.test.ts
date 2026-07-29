import { describe, expect, test } from "vitest";

import { resumeFrom } from "./resume";

/**
 * Every state the engine can persist, from `df-domain::ProjectState`. Listed
 * in full and on purpose: a state missing from this list is a state nobody
 * decided what to do with, and the flow would meet it for the first time in
 * front of a user.
 */
const EVERY_STATE = [
  "CREATED",
  "VALIDATING",
  "READY",
  "SCANNING",
  "SCAN_PAUSED",
  "SCANNED",
  "HASHING",
  "HASH_PAUSED",
  "HASHED",
  "ANALYZING",
  "ANALYSIS_PAUSED",
  "ANALYZED",
  "PLANNING",
  "PLAN_READY",
  "PLAN_REVIEW",
  "PLAN_APPROVED",
  "EXECUTING",
  "EXECUTION_PAUSED",
  "EXECUTED",
  "VERIFYING",
  "COMPLETED",
  "COMPLETED_WITH_WARNINGS",
  "FAILED",
  "ARCHIVED",
];

describe("resumeFrom", () => {
  test("decides something for every state the engine can persist", () => {
    for (const state of EVERY_STATE) {
      expect(resumeFrom(state).kind, state).toBeTypeOf("string");
    }
  });

  test("a fresh project runs the whole inspection", () => {
    expect(resumeFrom("CREATED")).toEqual({
      kind: "review",
      scan: true,
      hash: true,
      analyze: true,
      plan: true,
    });
  });

  test("each finished stage is skipped, and no earlier one is re-run", () => {
    expect(resumeFrom("SCANNED")).toMatchObject({ scan: false, hash: true });
    expect(resumeFrom("HASHED")).toMatchObject({ hash: false, analyze: true });
    expect(resumeFrom("ANALYZED")).toMatchObject({
      analyze: false,
      plan: true,
    });
    expect(resumeFrom("PLAN_READY")).toMatchObject({ plan: false });
  });

  test("a paused stage runs again, because it did not finish", () => {
    expect(resumeFrom("SCAN_PAUSED")).toMatchObject({ scan: true });
    expect(resumeFrom("HASH_PAUSED")).toMatchObject({ hash: true });
  });

  // A killed run leaves the durable in-progress state behind with its queue
  // intact. Both stages that own a resumable queue continue from it rather
  // than stranding the work; ADR-0029 excludes a second live writer.
  test("a run killed mid-stage is continued, not stranded", () => {
    expect(resumeFrom("HASHING")).toMatchObject({ kind: "review", hash: true });
    expect(resumeFrom("EXECUTING")).toMatchObject({
      kind: "copy",
      execute: true,
    });
  });

  // The user's consent lives in the approved manifest. Re-approving would
  // either fail or, worse, look like the flow asking twice for one decision.
  test("an approved plan is never approved again", () => {
    for (const state of ["PLAN_APPROVED", "EXECUTING", "EXECUTION_PAUSED"]) {
      expect(resumeFrom(state), state).toEqual({
        kind: "copy",
        approve: false,
        execute: true,
      });
    }
  });

  test("an executed plan only needs the independent check", () => {
    expect(resumeFrom("EXECUTED")).toEqual({
      kind: "copy",
      approve: false,
      execute: false,
    });
  });

  test("a finished project is not re-run", () => {
    for (const state of ["COMPLETED", "COMPLETED_WITH_WARNINGS", "FAILED"]) {
      expect(resumeFrom(state), state).toEqual({ kind: "finished", state });
    }
  });

  // These are the states no engine stage accepts as an entry point. Guessing a
  // recovery for them would mean acting outside what the engine offers.
  test("states the engine cannot continue are handed to the user", () => {
    for (const state of [
      "SCANNING",
      "ANALYSIS_PAUSED",
      "VERIFYING",
      "ARCHIVED",
    ]) {
      expect(resumeFrom(state), state).toEqual({ kind: "manual", state });
    }
  });

  test("an unknown state degrades to manual instead of guessing", () => {
    expect(resumeFrom("SOME_FUTURE_STATE")).toEqual({
      kind: "manual",
      state: "SOME_FUTURE_STATE",
    });
  });
});
