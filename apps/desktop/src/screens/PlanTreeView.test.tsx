import { render, screen } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";

import type { PlanDestinationTree } from "../types";
import { PlanTreeView } from "./PlanTreeView";

function tree(
  overrides: Partial<PlanDestinationTree> = {},
): PlanDestinationTree {
  return {
    plan_id: "plan-1",
    version: 1,
    output_root: "D:\\salida",
    files: 3,
    directories: 1,
    bytes: 1024 * 1024,
    without_destination: 0,
    nodes: [
      {
        prefix: "Origen",
        depth: 1,
        files: 2,
        directories: 1,
        bytes: 1024 * 1024,
        by_operation: [["COPY_ACTIVE", 2]],
        sample: "Origen\\expediente\\demanda.pdf",
      },
      {
        prefix: "90_DataForge_Review",
        depth: 1,
        files: 1,
        directories: 0,
        bytes: 0,
        by_operation: [["COPY_REVIEW", 1]],
        sample: "90_DataForge_Review\\Origen\\copia (2).pdf",
      },
      {
        prefix: "Origen\\expediente",
        depth: 2,
        files: 2,
        directories: 0,
        bytes: 1024 * 1024,
        by_operation: [["COPY_ACTIVE", 2]],
        sample: null,
      },
    ],
    ...overrides,
  };
}

describe("PlanTreeView", () => {
  test("names the destination roots and where they will be written", async () => {
    render(<PlanTreeView load={() => Promise.resolve(tree())} />);

    await screen.findByText(/Qué se va a crear en el destino/i);
    // The literal folder name must appear: renaming it in the UI would
    // describe a directory the user will not find on disk.
    expect(screen.getByText("90_DataForge_Review\\")).toBeDefined();
    expect(screen.getByText("Origen\\")).toBeDefined();
    expect(screen.getByText(/D:\\salida/)).toBeDefined();
    // A sample path beats a count for judging a plan.
    expect(screen.getByText(/Origen\\expediente\\demanda\.pdf/)).toBeDefined();
  });

  test("shows only top-level roots, not every nested prefix", async () => {
    render(<PlanTreeView load={() => Promise.resolve(tree())} />);

    await screen.findByText("Origen\\");
    // `Origen\expediente` is depth 2: it belongs to the CLI's deeper view,
    // not to the pre-approval summary.
    expect(screen.queryByText("Origen\\expediente\\")).toBeNull();
  });

  test("raises copies that would land nowhere", async () => {
    render(
      <PlanTreeView
        load={() => Promise.resolve(tree({ without_destination: 7 }))}
      />,
    );

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("7");
    expect(alert.textContent).toMatch(/no apruebes/i);
  });

  test("reports a failure instead of rendering an empty tree", async () => {
    render(
      <PlanTreeView
        load={() =>
          Promise.reject({ code: "validation", message: "no hay plan" })
        }
      />,
    );

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no hay plan");
  });

  test("loads once for a stable loader", async () => {
    const load = vi.fn(() => Promise.resolve(tree()));
    const { rerender } = render(<PlanTreeView load={load} />);
    await screen.findByText("Origen\\");
    rerender(<PlanTreeView load={load} />);
    await screen.findByText("Origen\\");
    expect(load).toHaveBeenCalledTimes(1);
  });
});
