/**
 * Where the guided flow picks a project up from.
 *
 * A run over a real archive takes minutes to hours, so being interrupted —
 * a closed window, a reboot, a machine that went to sleep — is ordinary, not
 * exceptional. The engine is built to be resumed; this table is what lets the
 * guided screen do it without asking the user to understand the lifecycle.
 *
 * Every entry mirrors a guard that actually exists in the engine, so the flow
 * never calls a stage that would refuse it:
 *
 *   scan     `df-scan`     CREATED, VALIDATING, READY, SCAN_PAUSED and the
 *                          stable checkpoints HASHED/ANALYZED/COMPLETED
 *   hash     `df-hash`     SCANNED, HASH_PAUSED
 *   analyze  `df-planner`  HASHED, ANALYZING, (ANALYZED = already done)
 *   plan     `df-planner`  ANALYZED, PLANNING
 *   approve  `df-planner`  PLAN_READY, PLAN_REVIEW
 *   execute  `df-executor` PLAN_APPROVED, EXECUTION_PAUSED, EXECUTING
 *   verify   `df-verifier` EXECUTED
 *
 * States the engine cannot continue on its own (a crash caught mid-transition,
 * an archived project) are reported as `manual` rather than guessed at: sending
 * the user to the detail view with the real state name beats a raw error, and
 * beats inventing a recovery the engine does not offer.
 */

export type Resume =
  /** Run the remaining inspection stages, then show the findings. */
  | {
      kind: "review";
      scan: boolean;
      hash: boolean;
      analyze: boolean;
      plan: boolean;
    }
  /**
   * The plan was already accepted; carry the copy to the end. Verification
   * always runs: it re-reads the output from primary evidence, so it is the
   * only thing that can turn "we copied it" into "we checked it".
   */
  | { kind: "copy"; approve: boolean; execute: boolean }
  /** Nothing left to do: this destination already holds a finished run. */
  | { kind: "finished"; state: string }
  /** The engine offers no automatic continuation from here. */
  | { kind: "manual"; state: string };

const REVIEW = (
  scan: boolean,
  hash: boolean,
  analyze: boolean,
  plan: boolean,
): Resume => ({ kind: "review", scan, hash, analyze, plan });

const COPY = (approve: boolean, execute: boolean): Resume => ({
  kind: "copy",
  approve,
  execute,
});

const TABLE: Record<string, Resume> = {
  // Nothing has been recorded yet: the whole inspection runs.
  CREATED: REVIEW(true, true, true, true),
  VALIDATING: REVIEW(true, true, true, true),
  READY: REVIEW(true, true, true, true),
  // A cancelled scan resumes into a fresh scan run.
  SCAN_PAUSED: REVIEW(true, true, true, true),
  SCANNED: REVIEW(false, true, true, true),
  HASH_PAUSED: REVIEW(false, true, true, true),
  HASHED: REVIEW(false, false, true, true),
  // The analysis marker commits just before the state transition; re-running
  // analyze from ANALYZING only finishes that transition.
  ANALYZING: REVIEW(false, false, true, true),
  ANALYZED: REVIEW(false, false, false, true),
  PLANNING: REVIEW(false, false, false, true),
  // The plan exists and is waiting for the user's answer.
  PLAN_READY: REVIEW(false, false, false, false),
  PLAN_REVIEW: REVIEW(false, false, false, false),
  // Past this point the user already said yes, so we do not ask again.
  PLAN_APPROVED: COPY(false, true),
  EXECUTING: COPY(false, true),
  EXECUTION_PAUSED: COPY(false, true),
  // Everything is on disk; only the independent check is missing.
  EXECUTED: COPY(false, false),
  COMPLETED: { kind: "finished", state: "COMPLETED" },
  COMPLETED_WITH_WARNINGS: {
    kind: "finished",
    state: "COMPLETED_WITH_WARNINGS",
  },
  FAILED: { kind: "finished", state: "FAILED" },
};

/**
 * What to do with a project found in `state`.
 *
 * `SCANNING`, `HASHING`, `ANALYSIS_PAUSED`, `VERIFYING` and `ARCHIVED` are
 * absent from the table on purpose: no engine stage accepts them, so they fall
 * through to `manual`. Unknown strings do the same — a newer engine state must
 * degrade to "open the detail view", never to a wrong guess.
 */
export function resumeFrom(state: string): Resume {
  return TABLE[state] ?? { kind: "manual", state };
}
