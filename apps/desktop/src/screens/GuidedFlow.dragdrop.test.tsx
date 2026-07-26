import { render, screen } from "@testing-library/react";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

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

type DragPayload =
  | { type: "enter" | "over"; position: { x: number; y: number } }
  | { type: "drop"; paths: string[]; position: { x: number; y: number } }
  | { type: "leave" };

let handler: ((event: { payload: DragPayload }) => void) | null = null;

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (fn: (event: { payload: DragPayload }) => void) => {
      handler = fn;
      return Promise.resolve(() => {
        handler = null;
      });
    },
  }),
}));

/**
 * jsdom lays nothing out, so every rect is zero. These are the two drop boxes
 * placed one above the other, which is how the screen actually renders them.
 */
const BOXES: Record<string, DOMRect> = {
  "Carpeta que quieres ordenar": { top: 100, bottom: 200, left: 0, right: 400 },
  "Dónde guardar el resultado": { top: 220, bottom: 320, left: 0, right: 400 },
} as unknown as Record<string, DOMRect>;

const noop = (): void => {};

beforeEach(() => {
  handler = null;
  vi.spyOn(
    HTMLLabelElement.prototype,
    "getBoundingClientRect",
  ).mockImplementation(function (this: HTMLLabelElement): DOMRect {
    const label = this.textContent ?? "";
    const match = Object.keys(BOXES).find((key) => label.startsWith(key));
    return (
      (match === undefined ? undefined : BOXES[match]) ??
      ({ top: 0, bottom: 0, left: 0, right: 0 } as DOMRect)
    );
  });
  // A plain, unscaled display: physical pixels equal CSS pixels.
  Object.defineProperty(window, "devicePixelRatio", {
    value: 1,
    configurable: true,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

async function drop(path: string, at: { x: number; y: number }): Promise<void> {
  await act(async () => {
    handler?.({ payload: { type: "drop", paths: [path], position: at } });
  });
}

function field(name: RegExp): HTMLInputElement {
  return screen.getByLabelText(name) as HTMLInputElement;
}

describe("GuidedFlow drag and drop", () => {
  beforeEach(async () => {
    render(<GuidedFlow onOpenAdvanced={noop} onExit={noop} />);
    // The listener is registered from a promise inside an effect.
    await act(async () => {});
  });

  // The drop used to fill whichever field was last focused, ignoring where the
  // pointer was. Dropping a folder onto the second box and watching it land in
  // the first is the kind of thing a user reads as the app being broken.
  test("a folder lands in the box it was dropped on", async () => {
    await drop("D:\\origen", { x: 200, y: 150 });
    expect(field(/carpeta que quieres ordenar/i).value).toBe("D:\\origen");
    expect(field(/dónde guardar el resultado/i).value).toBe("");

    await drop("D:\\salida", { x: 200, y: 260 });
    expect(field(/dónde guardar el resultado/i).value).toBe("D:\\salida");
    // The first box keeps what it had.
    expect(field(/carpeta que quieres ordenar/i).value).toBe("D:\\origen");
  });

  test("the highlight follows the pointer, not the focus", async () => {
    await act(async () => {
      handler?.({ payload: { type: "over", position: { x: 200, y: 260 } } });
    });
    const destination = field(/dónde guardar el resultado/i).closest("label");
    expect(destination?.className).toContain("dropfield-active");

    await act(async () => {
      handler?.({ payload: { type: "leave" } });
    });
    expect(destination?.className).not.toContain("dropfield-active");
  });

  // Dropping on the surrounding panel is still useful: it fills the field the
  // user was working in rather than doing nothing.
  test("a drop outside both boxes falls back to the focused field", async () => {
    field(/dónde guardar el resultado/i).focus();
    await drop("D:\\en-algun-sitio", { x: 200, y: 800 });
    expect(field(/dónde guardar el resultado/i).value).toBe(
      "D:\\en-algun-sitio",
    );
    expect(field(/carpeta que quieres ordenar/i).value).toBe("");
  });

  // On a 150% display the drag position arrives at 1.5x the CSS coordinates.
  test("a scaled display does not shift where the drop lands", async () => {
    Object.defineProperty(window, "devicePixelRatio", {
      value: 1.5,
      configurable: true,
    });
    await drop("D:\\salida", { x: 300, y: 390 });
    expect(field(/dónde guardar el resultado/i).value).toBe("D:\\salida");
    expect(field(/carpeta que quieres ordenar/i).value).toBe("");
  });
});
