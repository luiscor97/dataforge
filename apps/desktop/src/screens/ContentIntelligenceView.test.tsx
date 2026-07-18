import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  buildContentArtifacts,
  extractContent,
  failContentExtraction,
  queryContent,
  searchContent,
} from "../api";
import type {
  ContentExtractionOutcome,
  ContentQueryOutcome,
  ContentSearchOutcome,
} from "../types";
import { ContentIntelligenceView } from "./ContentIntelligenceView";

vi.mock("../api", () => ({
  buildContentArtifacts: vi.fn(),
  extractContent: vi.fn(),
  failContentExtraction: vi.fn(),
  queryContent: vi.fn(),
  searchContent: vi.fn(),
}));

const extractionOutcome: ContentExtractionOutcome = {
  run_id: "run-content-1",
  snapshot_id: "snapshot-1",
  status: "COMPLETED",
  extractor_version: "m0.4-test",
  config_digest: "a".repeat(64),
  counters: {
    contents_total: 12345,
    extracted: 1200,
    unsupported: 10,
    limited: 20,
    failed: 4,
    text_subjects: 1500,
    text_segments: 1700,
    mail_messages: 30,
    mail_threads: 12,
    mail_attachments: 8,
    archive_entries: 5,
  },
  processed_this_invocation: 1200,
  reused_this_invocation: 34,
  threads_built_this_invocation: 12,
  error: null,
};

const searchOutcome: ContentSearchOutcome = {
  run_id: "run-content-1",
  index: {
    id: "index-1",
    run_id: "run-content-1",
    snapshot_id: "snapshot-1",
    schema_version: "search-v1",
    relative_path: "content/search/index-1",
    content_digest: "b".repeat(64),
    documents: 1,
    created_at: "2026-07-18T10:00:00Z",
  },
  query: "contrato",
  hits: [
    {
      subject_id: "subject-1",
      content_id: "content-1",
      score: 1.25,
      file_name: "contrato.txt",
      relative_path: "legal/contrato.txt",
      representative_path: "C:\\evidence\\legal\\contrato.txt",
      virtual_path: "adjuntos/anexo.txt",
      subject: "DOCUMENT",
      context: "expediente 2026-42",
      mime: "text/plain",
      snippet: "Texto seguro <img src=x onerror=alert(1)>",
    },
  ],
};

const queryOutcome: ContentQueryOutcome = {
  run_id: "run-content-1",
  snapshot: {
    id: "analytical-1",
    run_id: "run-content-1",
    snapshot_id: "snapshot-1",
    schema_version: "analytical-v1",
    relative_path: "content/analytical/snapshot.parquet",
    sha256: "c".repeat(64),
    rows: 1,
    created_at: "2026-07-18T10:00:00Z",
  },
  result: {
    columns: [
      { name: "file_name", data_type: "Utf8" },
      { name: "size_bytes", data_type: "UInt64" },
    ],
    rows: [["contrato.txt", "42"]],
  },
};

beforeEach(() => {
  vi.mocked(buildContentArtifacts).mockReset();
  vi.mocked(extractContent).mockReset();
  vi.mocked(failContentExtraction).mockReset();
  vi.mocked(queryContent).mockReset();
  vi.mocked(searchContent).mockReset();
});

describe("ContentIntelligenceView", () => {
  test("announces and disables actions while extraction is loading", async () => {
    let resolveExtraction!: (outcome: ContentExtractionOutcome) => void;
    vi.mocked(extractContent).mockReturnValue(
      new Promise((resolve) => {
        resolveExtraction = resolve;
      }),
    );
    const user = userEvent.setup();
    render(<ContentIntelligenceView projectDir="C:\\project" />);

    await user.click(screen.getByRole("button", { name: "Extraer contenido" }));

    expect(screen.getByRole("button", { name: "Extrayendo…" })).toBeDisabled();
    expect(screen.getByText("Extrayendo contenido.")).toBeInTheDocument();

    resolveExtraction(extractionOutcome);
    expect(await screen.findByText("run-content-1")).toBeInTheDocument();
  });

  test("renders a clear facade error", async () => {
    vi.mocked(extractContent).mockRejectedValue({
      code: "validation",
      message: "the project has no complete snapshot",
    });
    const user = userEvent.setup();
    render(<ContentIntelligenceView projectDir="C:\\project" />);

    await user.click(screen.getByRole("button", { name: "Extraer contenido" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("validation");
    expect(alert).toHaveTextContent("the project has no complete snapshot");
  });

  test("renders successful extraction counters and selects its run", async () => {
    vi.mocked(extractContent).mockResolvedValue(extractionOutcome);
    const user = userEvent.setup();
    render(<ContentIntelligenceView projectDir="C:\\project" />);

    await user.click(screen.getByRole("button", { name: "Extraer contenido" }));

    const section = screen
      .getByRole("heading", { name: "1. Extraer" })
      .closest("section");
    expect(section).not.toBeNull();
    expect(
      within(section as HTMLElement).getByText("12.345"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Run de extracción")).toHaveValue(
      "run-content-1",
    );
    expect(screen.getByText("COMPLETED")).toBeInTheDocument();
  });

  test("renders search evidence as plain text", async () => {
    vi.mocked(searchContent).mockResolvedValue(searchOutcome);
    const user = userEvent.setup();
    const { container } = render(
      <ContentIntelligenceView projectDir="C:\\project" />,
    );

    await user.type(
      screen.getByRole("searchbox", { name: "Consulta de texto" }),
      "contrato",
    );
    await user.click(screen.getByRole("button", { name: "Buscar" }));

    expect(await screen.findByText("legal/contrato.txt")).toBeInTheDocument();
    expect(screen.getByText("expediente 2026-42")).toBeInTheDocument();
    expect(
      screen.getByText("Texto seguro <img src=x onerror=alert(1)>"),
    ).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });

  test("renders the bounded SQL result as a semantic table", async () => {
    vi.mocked(queryContent).mockResolvedValue(queryOutcome);
    const user = userEvent.setup();
    render(<ContentIntelligenceView projectDir="C:\\project" />);

    await user.click(
      screen.getByRole("button", { name: "Ejecutar consulta acotada" }),
    );

    const table = await screen.findByRole("table");
    expect(
      within(table).getByRole("columnheader", { name: /file_name/ }),
    ).toBeInTheDocument();
    expect(
      within(table).getByRole("cell", { name: "contrato.txt" }),
    ).toBeInTheDocument();
    expect(within(table).getByRole("cell", { name: "42" })).toBeInTheDocument();
  });
});
