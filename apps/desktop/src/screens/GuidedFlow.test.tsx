import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  analyzeProject,
  approvePlan,
  createPlan,
  createProject,
  executePlan,
  hashProject,
  openProject,
  projectStatus,
  scanProject,
  validatePlan,
  verifyProject,
} from "../api";
import { GuidedFlow } from "./GuidedFlow";

vi.mock("../api", () => ({
  analyzeProject: vi.fn(),
  approvePlan: vi.fn(),
  createPlan: vi.fn(),
  createProject: vi.fn(),
  executePlan: vi.fn(),
  hashProject: vi.fn(),
  openProject: vi.fn(),
  projectStatus: vi.fn(),
  scanProject: vi.fn(),
  validatePlan: vi.fn(),
  verifyProject: vi.fn(),
}));

// The webview module is only resolvable inside Tauri. Leaving it unmocked is
// deliberate in the first test below, which pins that the screen survives it.
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => {
    throw new Error("no Tauri runtime");
  },
}));

const SOURCE = "D:\\origen";
const DESTINATION = "D:\\salida";
const PROJECT_DIR = "D:\\salida-dataforge";

const noop = (): void => {};

function renderFlow(): void {
  render(<GuidedFlow onOpenAdvanced={noop} onExit={noop} />);
}

/** A `ProjectStatus` with only the fields this screen reads. */
function status(
  state: string,
  inventory?: { files: number; folders: number; scan_errors: number },
): unknown {
  return {
    project_dir: PROJECT_DIR,
    state,
    output_root: DESTINATION,
    source_roots: [{ absolute_path: SOURCE }],
    inventory: inventory ?? null,
  };
}

async function fillFoldersAndSubmit(): Promise<void> {
  const user = userEvent.setup();
  await user.type(
    screen.getByLabelText(/carpeta que quieres ordenar/i),
    SOURCE,
  );
  await user.type(
    screen.getByLabelText(/dónde guardar el resultado/i),
    DESTINATION,
  );
  await user.click(
    screen.getByRole("button", { name: /examinar mis archivos/i }),
  );
}

/** The happy path: a brand new project that inspects cleanly. */
function mockFreshReview(inventory: {
  files: number;
  folders: number;
  scan_errors: number;
}): void {
  vi.mocked(createProject).mockResolvedValue(status("CREATED") as never);
  vi.mocked(scanProject).mockResolvedValue({
    errors: inventory.scan_errors,
  } as never);
  vi.mocked(hashProject).mockResolvedValue({} as never);
  vi.mocked(analyzeProject).mockResolvedValue({ duplicate_sets: 89 } as never);
  vi.mocked(createPlan).mockResolvedValue({ operations: 1290 } as never);
  vi.mocked(projectStatus).mockResolvedValue(
    status("PLAN_READY", inventory) as never,
  );
}

describe("GuidedFlow", () => {
  beforeEach(() => {
    vi.resetAllMocks();
  });

  // Typecheck, lint and build all passed while this screen crashed on mount,
  // because getCurrentWebview throws synchronously outside Tauri. Only
  // rendering it catches that, so this test exists to keep it caught.
  test("renders even where drag and drop is unavailable", () => {
    renderFlow();
    expect(
      screen.getByRole("heading", { name: /ordenar mis documentos/i }),
    ).toBeDefined();
    expect(screen.getByLabelText(/carpeta que quieres ordenar/i)).toBeDefined();
  });

  test("states the origin is never modified, before anything is chosen", () => {
    renderFlow();
    expect(
      screen.getByText(/nunca modifica ni borra tus archivos originales/i),
    ).toBeDefined();
  });

  test("reports findings in plain language and never claims a deletion", async () => {
    mockFreshReview({ files: 1234, folders: 56, scan_errors: 0 });

    renderFlow();
    await fillFoldersAndSubmit();

    expect(
      await screen.findByRole("heading", { name: /esto he encontrado/i }),
    ).toBeDefined();
    // Numbers are shown grouped for readability, not as raw integers.
    expect(screen.getByText("1.234")).toBeDefined();
    expect(screen.getByText("89")).toBeDefined();
    // The duplicate policy is REPORT_ONLY, so the copy must never be described
    // as removing anything the user owns.
    expect(
      screen.getByText(/no se borra nada por nuestra cuenta/i),
    ).toBeDefined();
  });

  test("a failure surfaces as an alert and returns to the form", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "validation",
      message: "las carpetas se solapan",
    });
    vi.mocked(openProject).mockRejectedValue({
      code: "not_found",
      message: "no project here",
    });

    renderFlow();
    await fillFoldersAndSubmit();

    const alert = await screen.findByRole("alert");
    // The creation failure is the one that explains the problem; the failed
    // attempt to reopen must not mask it.
    expect(alert.textContent).toContain("las carpetas se solapan");
    // The user keeps their input and can correct it.
    expect(screen.getByLabelText(/carpeta que quieres ordenar/i)).toBeDefined();
  });

  test("announces the verified result without overstating it", async () => {
    mockFreshReview({ files: 2, folders: 1, scan_errors: 0 });
    vi.mocked(createPlan).mockResolvedValue({ operations: 3 } as never);
    vi.mocked(approvePlan).mockResolvedValue({} as never);
    vi.mocked(executePlan).mockResolvedValue({
      completed: 3,
      bytes_copied: 2048,
    } as never);
    vi.mocked(verifyProject).mockResolvedValue({
      verdict: "COMPLETED_WITH_WARNINGS",
      problems: 0,
      warnings: 2,
    } as never);

    renderFlow();
    await fillFoldersAndSubmit();
    await screen.findByRole("heading", { name: /esto he encontrado/i });
    await userEvent.click(
      screen.getByRole("button", { name: /hacer la copia/i }),
    );

    // A run with warnings must not be announced as a clean success.
    expect(
      await screen.findByRole("heading", {
        name: /copia terminada, con avisos/i,
      }),
    ).toBeDefined();
    expect(screen.getByText(/tus originales siguen intactos/i)).toBeDefined();
  });

  // --- Resuming an interrupted run ---------------------------------------

  test("an interrupted run continues instead of re-hashing everything", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "the project directory already exists",
    });
    vi.mocked(openProject).mockResolvedValue(status("HASHED") as never);
    vi.mocked(analyzeProject).mockResolvedValue({ duplicate_sets: 4 } as never);
    vi.mocked(createPlan).mockResolvedValue({ operations: 12 } as never);
    vi.mocked(projectStatus).mockResolvedValue(
      status("PLAN_READY", { files: 40, folders: 3, scan_errors: 0 }) as never,
    );

    renderFlow();
    await fillFoldersAndSubmit();

    await screen.findByRole("heading", { name: /esto he encontrado/i });
    expect(vi.mocked(scanProject)).not.toHaveBeenCalled();
    expect(vi.mocked(hashProject)).not.toHaveBeenCalled();
    expect(vi.mocked(analyzeProject)).toHaveBeenCalledOnce();
    expect(screen.getByText(/lo hemos\s+continuado/i)).toBeDefined();
  });

  // A project directory is derived from the destination, so reusing a
  // destination with a different origin would otherwise silently continue
  // somebody else's work against the wrong source.
  test("refuses to resume a project that describes different folders", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "the project directory already exists",
    });
    vi.mocked(openProject).mockResolvedValue({
      project_dir: PROJECT_DIR,
      state: "HASHED",
      output_root: DESTINATION,
      source_roots: [{ absolute_path: "E:\\otra-cosa" }],
      inventory: null,
    } as never);

    renderFlow();
    await fillFoldersAndSubmit();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("E:\\otra-cosa");
    expect(vi.mocked(hashProject)).not.toHaveBeenCalled();
    expect(vi.mocked(analyzeProject)).not.toHaveBeenCalled();
  });

  test("a plan left from an earlier session is re-validated, not trusted", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "x",
    });
    vi.mocked(openProject).mockResolvedValue(status("PLAN_READY") as never);
    vi.mocked(validatePlan).mockResolvedValue({
      ok: true,
      operations: 77,
      problems: [],
    } as never);
    vi.mocked(projectStatus).mockResolvedValue(
      status("PLAN_READY", { files: 80, folders: 5, scan_errors: 0 }) as never,
    );

    renderFlow();
    await fillFoldersAndSubmit();

    await screen.findByRole("heading", { name: /esto he encontrado/i });
    expect(vi.mocked(createPlan)).not.toHaveBeenCalled();
    // The count comes from the stored plan, never from a placeholder.
    expect(screen.getByText("77")).toBeDefined();
    // Nothing was analysed in this run, so no duplicate count is claimed.
    expect(screen.queryByText(/grupos de copias repetidas/i)).toBeNull();
  });

  test("a stale plan is refused rather than offered for approval", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "x",
    });
    vi.mocked(openProject).mockResolvedValue(status("PLAN_READY") as never);
    vi.mocked(validatePlan).mockResolvedValue({
      ok: false,
      operations: 77,
      problems: ["destination collision"],
    } as never);

    renderFlow();
    await fillFoldersAndSubmit();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("destination collision");
    expect(vi.mocked(approvePlan)).not.toHaveBeenCalled();
  });

  // The approved manifest already records the user's consent (§26.4). Asking
  // again would be asking twice for one decision.
  test("a half-done copy resumes without asking for approval twice", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "x",
    });
    vi.mocked(openProject).mockResolvedValue(
      status("EXECUTION_PAUSED") as never,
    );
    vi.mocked(executePlan).mockResolvedValue({
      completed: 5,
      bytes_copied: 1024,
    } as never);
    vi.mocked(verifyProject).mockResolvedValue({
      verdict: "COMPLETED",
      problems: 0,
      warnings: 0,
    } as never);

    renderFlow();
    await fillFoldersAndSubmit();

    await screen.findByRole("heading", { name: /copia a medias/i });
    await userEvent.click(
      screen.getByRole("button", { name: /continuar la copia/i }),
    );

    expect(
      await screen.findByRole("heading", { name: /copia está verificada/i }),
    ).toBeDefined();
    expect(vi.mocked(approvePlan)).not.toHaveBeenCalled();
    expect(vi.mocked(executePlan)).toHaveBeenCalledOnce();
  });

  test("a finished destination is reported, not copied over", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "x",
    });
    vi.mocked(openProject).mockResolvedValue(status("COMPLETED") as never);

    renderFlow();
    await fillFoldersAndSubmit();

    await screen.findByRole("heading", { name: /ya está hecha/i });
    expect(vi.mocked(executePlan)).not.toHaveBeenCalled();
    expect(vi.mocked(approvePlan)).not.toHaveBeenCalled();
    expect(screen.getByText(/no tocaremos esta/i)).toBeDefined();
  });

  test("a state the engine cannot continue is handed over, not guessed at", async () => {
    vi.mocked(createProject).mockRejectedValue({
      code: "conflict",
      message: "x",
    });
    vi.mocked(openProject).mockResolvedValue(status("VERIFYING") as never);

    renderFlow();
    await fillFoldersAndSubmit();

    await screen.findByRole("heading", { name: /trabajo interrumpido/i });
    expect(screen.getByText("VERIFYING")).toBeDefined();
    expect(vi.mocked(executePlan)).not.toHaveBeenCalled();
  });
});
