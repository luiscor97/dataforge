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
  scanProject,
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
  scanProject: vi.fn(),
  verifyProject: vi.fn(),
}));

// The webview module is only resolvable inside Tauri. Leaving it unmocked is
// deliberate in the first test below, which pins that the screen survives it.
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => {
    throw new Error("no Tauri runtime");
  },
}));

const noop = (): void => {};

function renderFlow(): void {
  render(<GuidedFlow onOpenAdvanced={noop} onExit={noop} />);
}

async function fillFoldersAndSubmit(): Promise<void> {
  const user = userEvent.setup();
  await user.type(
    screen.getByLabelText(/carpeta que quieres ordenar/i),
    "D:\\origen",
  );
  await user.type(
    screen.getByLabelText(/dónde guardar el resultado/i),
    "D:\\salida",
  );
  await user.click(screen.getByRole("button", { name: /examinar mis archivos/i }));
}

describe("GuidedFlow", () => {
  beforeEach(() => {
    vi.mocked(createProject).mockReset();
    vi.mocked(scanProject).mockReset();
    vi.mocked(hashProject).mockReset();
    vi.mocked(analyzeProject).mockReset();
    vi.mocked(createPlan).mockReset();
    vi.mocked(approvePlan).mockReset();
    vi.mocked(executePlan).mockReset();
    vi.mocked(verifyProject).mockReset();
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
    vi.mocked(createProject).mockResolvedValue({
      project_dir: "D:\\salida-dataforge",
    } as never);
    vi.mocked(scanProject).mockResolvedValue({
      files: 1234,
      folders: 56,
      errors: 0,
    } as never);
    vi.mocked(hashProject).mockResolvedValue({} as never);
    vi.mocked(analyzeProject).mockResolvedValue({ duplicate_sets: 89 } as never);
    vi.mocked(createPlan).mockResolvedValue({ operations: 1290 } as never);

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

    renderFlow();
    await fillFoldersAndSubmit();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("las carpetas se solapan");
    // The user keeps their input and can correct it.
    expect(screen.getByLabelText(/carpeta que quieres ordenar/i)).toBeDefined();
  });

  test("announces the verified result without overstating it", async () => {
    vi.mocked(createProject).mockResolvedValue({
      project_dir: "D:\\salida-dataforge",
    } as never);
    vi.mocked(scanProject).mockResolvedValue({
      files: 2,
      folders: 1,
      errors: 0,
    } as never);
    vi.mocked(hashProject).mockResolvedValue({} as never);
    vi.mocked(analyzeProject).mockResolvedValue({ duplicate_sets: 0 } as never);
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
    await userEvent.click(screen.getByRole("button", { name: /hacer la copia/i }));

    // A run with warnings must not be announced as a clean success.
    expect(
      await screen.findByRole("heading", { name: /copia terminada, con avisos/i }),
    ).toBeDefined();
    expect(screen.getByText(/tus originales siguen intactos/i)).toBeDefined();
  });
});
