import { useState } from "react";

import {
  buildContentArtifacts,
  extractContent,
  failContentExtraction,
  queryContent,
  searchContent,
} from "../api";
import { ErrorAlert } from "../components/ErrorAlert";
import {
  type ContentArtifactBuildOutcome,
  type ContentExtractionOutcome,
  type ContentQueryOutcome,
  type ContentSearchOutcome,
  type ErrorDto,
  isErrorDto,
} from "../types";

const COUNT_FORMAT = new Intl.NumberFormat("es-ES");
const SEARCH_PAGE_SIZE = 25;
const SEARCH_SNIPPET_CHARS = 320;
const DEFAULT_SQL =
  "SELECT file_name, relative_path, subject_kind, mime, size_bytes FROM content LIMIT 25";

type ContentAction = "extract" | "fail" | "build" | "search" | "query";

interface ContentIntelligenceViewProps {
  projectDir: string;
}

function optionalRunId(value: string): string | null {
  const normalized = value.trim();
  return normalized === "" ? null : normalized;
}

function toErrorDto(failure: unknown): ErrorDto {
  return isErrorDto(failure)
    ? failure
    : {
        code: "unknown",
        message: failure instanceof Error ? failure.message : String(failure),
      };
}

function occurrenceKeys(values: readonly string[]): string[] {
  const occurrences = new Map<string, number>();
  return values.map((value) => {
    const occurrence = occurrences.get(value) ?? 0;
    occurrences.set(value, occurrence + 1);
    return `${value}:${occurrence}`;
  });
}

export function ContentIntelligenceView({
  projectDir,
}: ContentIntelligenceViewProps): React.JSX.Element {
  const [activeAction, setActiveAction] = useState<ContentAction | null>(null);
  const [error, setError] = useState<ErrorDto | null>(null);
  const [announcement, setAnnouncement] = useState<string>("");
  const [runId, setRunId] = useState<string>("");
  const [failureReason, setFailureReason] = useState<string>("");
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [sql, setSql] = useState<string>(DEFAULT_SQL);
  const [extraction, setExtraction] = useState<ContentExtractionOutcome | null>(
    null,
  );
  const [artifacts, setArtifacts] =
    useState<ContentArtifactBuildOutcome | null>(null);
  const [searchResult, setSearchResult] = useState<ContentSearchOutcome | null>(
    null,
  );
  const [queryResult, setQueryResult] = useState<ContentQueryOutcome | null>(
    null,
  );

  const busy = activeAction !== null;

  async function perform<T>(
    action: ContentAction,
    operation: () => Promise<T>,
    onSuccess: (result: T) => void,
    successMessage: string,
  ): Promise<void> {
    setActiveAction(action);
    setError(null);
    setAnnouncement("");
    try {
      const result = await operation();
      onSuccess(result);
      setAnnouncement(successMessage);
    } catch (failure) {
      setError(toErrorDto(failure));
    } finally {
      setActiveAction(null);
    }
  }

  async function handleExtract(): Promise<void> {
    await perform(
      "extract",
      () => extractContent(projectDir),
      (outcome) => {
        setExtraction(outcome);
        setRunId(outcome.run_id);
      },
      "La extracción de contenido ha terminado.",
    );
  }

  async function handleFail(): Promise<void> {
    const selectedRun = runId.trim();
    const reason = failureReason.trim();
    if (selectedRun === "" || reason === "") {
      setError({
        code: "validation",
        message: "Indica el run y el motivo del cierre.",
      });
      return;
    }
    await perform(
      "fail",
      () => failContentExtraction(projectDir, selectedRun, reason),
      (outcome) => {
        setExtraction(outcome);
        setFailureReason("");
      },
      "El run de extracción se ha cerrado como fallido.",
    );
  }

  async function handleBuild(): Promise<void> {
    await perform(
      "build",
      () => buildContentArtifacts(projectDir, optionalRunId(runId)),
      (outcome) => {
        setArtifacts(outcome);
        setRunId(outcome.run_id);
      },
      "La búsqueda y el análisis están preparados.",
    );
  }

  async function handleSearch(): Promise<void> {
    await perform(
      "search",
      () =>
        searchContent(projectDir, optionalRunId(runId), {
          query: searchQuery,
          limit: SEARCH_PAGE_SIZE,
          offset: 0,
          snippet_chars: SEARCH_SNIPPET_CHARS,
        }),
      (outcome) => {
        setSearchResult(outcome);
        setRunId(outcome.run_id);
      },
      "La búsqueda ha terminado.",
    );
  }

  async function handleQuery(): Promise<void> {
    await perform(
      "query",
      () => queryContent(projectDir, optionalRunId(runId), sql),
      (outcome) => {
        setQueryResult(outcome);
        setRunId(outcome.run_id);
      },
      "La consulta SQL de solo lectura ha terminado.",
    );
  }

  const extractionMetrics =
    extraction === null
      ? []
      : ([
          ["Contenidos", extraction.counters.contents_total],
          ["Extraídos", extraction.counters.extracted],
          ["No compatibles", extraction.counters.unsupported],
          ["Limitados", extraction.counters.limited],
          ["Fallidos", extraction.counters.failed],
          ["Sujetos de texto", extraction.counters.text_subjects],
          ["Segmentos", extraction.counters.text_segments],
          ["Mensajes", extraction.counters.mail_messages],
          ["Hilos", extraction.counters.mail_threads],
          ["Adjuntos", extraction.counters.mail_attachments],
          ["Entradas de archivo", extraction.counters.archive_entries],
        ] satisfies Array<readonly [string, number]>);
  const columnKeys =
    queryResult === null
      ? []
      : occurrenceKeys(
          queryResult.result.columns.map(
            (column) => `${column.name}:${column.data_type}`,
          ),
        );
  const rowKeys =
    queryResult === null
      ? []
      : occurrenceKeys(
          queryResult.result.rows.map((row) => JSON.stringify(row)),
        );

  return (
    <section
      className="diagnostics content-intelligence"
      aria-labelledby="content-heading"
    >
      <div className="section-heading">
        <div>
          <h3 id="content-heading">
            Contenido y búsqueda <span className="milestone">M0.4</span>
          </h3>
          <p>
            Extrae el texto de tus archivos y busca dentro de ellos. Nada ocurre
            automáticamente: cada paso se lanza con su propio botón.
          </p>
        </div>
        {extraction !== null && (
          <span className="diagnostic-badge diagnostic-complete" role="status">
            {extraction.status}
          </span>
        )}
      </div>

      {error !== null && <ErrorAlert error={error} />}
      <p className="visually-hidden" role="status" aria-live="polite">
        {activeAction === "extract"
          ? "Extrayendo contenido."
          : activeAction === "fail"
            ? "Cerrando el run."
            : activeAction === "build"
              ? "Preparando la búsqueda."
              : activeAction === "search"
                ? "Buscando contenido."
                : activeAction === "query"
                  ? "Ejecutando la consulta SQL."
                  : announcement}
      </p>

      <div className="content-run-control">
        <label htmlFor="content-run-id">Run de extracción</label>
        <input
          id="content-run-id"
          value={runId}
          onChange={(event) => setRunId(event.target.value)}
          placeholder="Vacío para usar el último run completado"
          disabled={busy}
          aria-describedby="content-run-help"
        />
        <p id="content-run-help" className="field-help">
          La extracción completa este valor automáticamente. Déjalo vacío para
          construir, buscar o consultar el último run completado del snapshot
          actual.
        </p>
      </div>

      <section
        className="content-step"
        aria-labelledby="content-extract-heading"
      >
        <div className="content-step-heading">
          <div>
            <h4 id="content-extract-heading">1. Extraer</h4>
            <p>
              Lee el texto de los archivos ya verificados para poder buscar en
              ellos. No modifica los originales.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void handleExtract()}
            disabled={busy}
          >
            {activeAction === "extract" ? "Extrayendo…" : "Extraer contenido"}
          </button>
        </div>

        {extraction === null ? (
          <p className="diagnostic-status diagnostic-pending" role="status">
            No se ha ejecutado una extracción en esta sesión. Empieza por aquí
            para poder buscar en tus documentos.
          </p>
        ) : (
          <div className="content-outcome">
            <dl className="metric-list content-metrics">
              <dt>Run</dt>
              <dd>
                <code>{extraction.run_id}</code>
              </dd>
              <dt>Snapshot</dt>
              <dd>
                <code>{extraction.snapshot_id}</code>
              </dd>
              <dt>Procesados ahora</dt>
              <dd>
                {COUNT_FORMAT.format(extraction.processed_this_invocation)}
              </dd>
              <dt>Reutilizados ahora</dt>
              <dd>{COUNT_FORMAT.format(extraction.reused_this_invocation)}</dd>
              <dt>Hilos construidos ahora</dt>
              <dd>
                {COUNT_FORMAT.format(extraction.threads_built_this_invocation)}
              </dd>
              {extractionMetrics.map(([label, value]) => (
                <div className="metric-pair" key={label}>
                  <dt>{label}</dt>
                  <dd>{COUNT_FORMAT.format(value)}</dd>
                </div>
              ))}
            </dl>
            {extraction.error !== null && (
              <p className="metric-alert">{extraction.error}</p>
            )}
          </div>
        )}

        <details className="failure-control">
          <summary>Cerrar un run irrecuperable (avanzado)</summary>
          <p>
            Úsalo solo si el origen no puede restaurarse. Un error normal debe
            conservar el run abierto para poder reanudarlo.
          </p>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void handleFail();
            }}
          >
            <label htmlFor="content-failure-reason">Motivo del cierre</label>
            <textarea
              id="content-failure-reason"
              value={failureReason}
              onChange={(event) => setFailureReason(event.target.value)}
              rows={2}
              maxLength={4096}
              required
              disabled={busy}
            />
            <button
              type="submit"
              disabled={
                busy || runId.trim() === "" || failureReason.trim() === ""
              }
            >
              {activeAction === "fail"
                ? "Cerrando…"
                : "Marcar run como fallido"}
            </button>
          </form>
        </details>
      </section>

      <section className="content-step" aria-labelledby="content-build-heading">
        <div className="content-step-heading">
          <div>
            <h4 id="content-build-heading">2. Preparar la búsqueda</h4>
            <p>
              Crea el índice de búsqueda y la tabla de análisis a partir de la
              extracción. Puedes regenerarlos cuando quieras.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void handleBuild()}
            disabled={busy}
          >
            {activeAction === "build" ? "Preparando…" : "Preparar búsqueda"}
          </button>
        </div>
        {artifacts !== null && (
          <dl
            className="metric-list content-metrics"
            aria-label="Artefactos construidos"
          >
            <dt>Documentos indexados</dt>
            <dd>{COUNT_FORMAT.format(artifacts.search_index.documents)}</dd>
            <dt>Filas analíticas</dt>
            <dd>{COUNT_FORMAT.format(artifacts.analytical_snapshot.rows)}</dd>
            <dt>Esquema de búsqueda</dt>
            <dd>
              <code>{artifacts.search_index.schema_version}</code>
            </dd>
            <dt>Esquema analítico</dt>
            <dd>
              <code>{artifacts.analytical_snapshot.schema_version}</code>
            </dd>
          </dl>
        )}
      </section>

      <section
        className="content-step"
        aria-labelledby="content-search-heading"
      >
        <h4 id="content-search-heading">3. Buscar</h4>
        <p className="step-description">
          Busca palabras o frases dentro del contenido extraído de tus archivos.
        </p>
        <form
          className="inline-form"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSearch();
          }}
        >
          <label htmlFor="content-search-query">Consulta de texto</label>
          <div className="input-action">
            <input
              id="content-search-query"
              type="search"
              value={searchQuery}
              onChange={(event) => setSearchQuery(event.target.value)}
              required
              disabled={busy}
            />
            <button type="submit" disabled={busy || searchQuery.trim() === ""}>
              {activeAction === "search" ? "Buscando…" : "Buscar"}
            </button>
          </div>
        </form>

        {searchResult !== null && (
          <div className="content-results" aria-live="polite">
            <p>
              {COUNT_FORMAT.format(searchResult.hits.length)} resultado(s) para
              «{searchResult.query}».
            </p>
            {searchResult.hits.length === 0 ? (
              <p>No hay coincidencias.</p>
            ) : (
              <ol className="search-results">
                {searchResult.hits.map((hit) => (
                  <li key={hit.subject_id}>
                    <h5>{hit.file_name}</h5>
                    <p>
                      <strong>Ruta:</strong> <code>{hit.relative_path}</code>
                    </p>
                    {hit.virtual_path !== null && (
                      <p>
                        <strong>Ruta virtual:</strong>{" "}
                        <code>{hit.virtual_path}</code>
                      </p>
                    )}
                    <p>
                      <strong>Contexto:</strong> {hit.context}
                    </p>
                    <p className="search-snippet">{hit.snippet}</p>
                    <small>
                      {hit.subject} · {hit.mime} · puntuación{" "}
                      {hit.score.toFixed(3)}
                    </small>
                  </li>
                ))}
              </ol>
            )}
          </div>
        )}
      </section>

      <section className="content-step" aria-labelledby="content-query-heading">
        <h4 id="content-query-heading">4. Consultar con SQL (avanzado)</h4>
        <p className="step-description">
          Para usuarios avanzados: consultas de solo lectura sobre la evidencia
          derivada.
        </p>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void handleQuery();
          }}
        >
          <label htmlFor="content-sql">
            SQL de solo lectura sobre la tabla content
          </label>
          <textarea
            id="content-sql"
            className="sql-input"
            value={sql}
            onChange={(event) => setSql(event.target.value)}
            rows={4}
            spellCheck={false}
            required
            disabled={busy}
          />
          <button type="submit" disabled={busy || sql.trim() === ""}>
            {activeAction === "query"
              ? "Consultando…"
              : "Ejecutar consulta acotada"}
          </button>
        </form>

        {queryResult !== null && (
          <div
            className="table-scroll"
            tabIndex={0}
            aria-label="Resultado SQL desplazable"
          >
            <table>
              <caption>
                {COUNT_FORMAT.format(queryResult.result.rows.length)} fila(s)
                devueltas
              </caption>
              <thead>
                <tr>
                  {queryResult.result.columns.map((column, columnIndex) => (
                    <th key={columnKeys[columnIndex]} scope="col">
                      {column.name}
                      <small>{column.data_type}</small>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {queryResult.result.rows.map((row, rowIndex) => (
                  <tr key={rowKeys[rowIndex]}>
                    {row.map((cell, columnIndex) => (
                      <td key={columnKeys[columnIndex]}>
                        {cell ?? <span aria-label="nulo">NULL</span>}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </section>
  );
}
