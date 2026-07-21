import type {
  MediaRelationView,
  ProjectStatus,
  SimilarityRelationView,
} from "../types";
import { ContentIntelligenceView } from "./ContentIntelligenceView";

const COUNT_FORMAT = new Intl.NumberFormat("es-ES");
const SIZE_FORMAT = new Intl.NumberFormat("es-ES", {
  maximumFractionDigits: 1,
});
const SIZE_UNITS = ["bytes", "KB", "MB", "GB", "TB", "PB"] as const;

/** Readable labels for the engine's closed relation enums (presentation only). */
const SIMILARITY_KIND_LABELS: Record<SimilarityRelationView["kind"], string> = {
  LIKELY_VERSION: "Probable versión",
  TRUNCATED_VARIANT: "Variante recortada",
  RECOMPOSED_CONTENT: "Contenido recompuesto",
  SIMILAR_CONTENT: "Contenido similar",
};

const MEDIA_RELATION_LABELS: Record<MediaRelationView["relation"], string> = {
  IMAGE_PERCEPTUAL_MATCH: "Imágenes que coinciden",
  AUDIO_ACOUSTIC_MATCH: "Audios que coinciden",
  VIDEO_PERCEPTUAL_MATCH: "Vídeos que coinciden",
};

function formatCount(value: number): string {
  return COUNT_FORMAT.format(value);
}

function formatBytes(value: number): string {
  let scaled = value;
  let unit = 0;
  while (scaled >= 1000 && unit < SIZE_UNITS.length - 1) {
    scaled /= 1000;
    unit += 1;
  }
  return `${SIZE_FORMAT.format(scaled)} ${SIZE_UNITS[unit]}`;
}

type NextAction = "refresh" | "similarity" | "media" | null;

interface NextStep {
  message: string;
  action: NextAction;
}

/**
 * Derives the suggested next step from the same DTO conditions that the
 * sections below already present. It only points at actions the facade
 * offers; the shell never decides on its own.
 */
function deriveNextStep(status: ProjectStatus): NextStep {
  if (status.integrity !== null && status.integrity.problems.length > 0) {
    return {
      message:
        "La comprobación de integridad ha encontrado problemas. Revísalos más arriba antes de continuar.",
      action: null,
    };
  }
  if (status.latest_snapshot_id === null) {
    return {
      message:
        "Todavía no hay un análisis completado de tus carpetas. Actualiza el estado para comprobar si hay novedades.",
      action: "refresh",
    };
  }
  const diagnostics = status.structural_diagnostics;
  if (diagnostics === null || !diagnostics.analysis_complete) {
    return {
      message:
        "El análisis estructural está pendiente o en marcha. Actualiza el estado para ver si ya ha terminado.",
      action: "refresh",
    };
  }
  if (status.similarity === null) {
    return {
      message:
        "El análisis estructural ha terminado. Ahora puedes buscar versiones y contenidos parecidos entre tus archivos.",
      action: "similarity",
    };
  }
  if (status.media === null) {
    return {
      message:
        "Ya tienes la evidencia de similitud. Ahora puedes analizar fotos, audio y vídeo.",
      action: "media",
    };
  }
  return {
    message:
      "Los análisis principales están completos. Revisa la evidencia de cada sección o busca dentro del contenido más abajo.",
    action: null,
  };
}

interface StatusViewProps {
  status: ProjectStatus;
  busy: boolean;
  onRefresh: () => void;
  onAnalyzeSimilarity?: () => void;
  onAnalyzeMedia?: () => void;
  onBack: () => void;
}

export function StatusView({
  status,
  busy,
  onRefresh,
  onAnalyzeSimilarity,
  onAnalyzeMedia,
  onBack,
}: StatusViewProps): React.JSX.Element {
  const diagnostics = status.structural_diagnostics ?? null;
  const similarity = status.similarity ?? null;
  const media = status.media ?? null;
  const nextStep = deriveNextStep(status);

  return (
    <section className="panel status-view">
      <div className="status-header">
        <h2>
          {status.name}{" "}
          <span className={`state state-${status.state.toLowerCase()}`}>
            {status.state}
          </span>
        </h2>
      </div>

      {status.inventory !== null && (
        <dl className="stats" aria-label="Resumen del último inventario">
          <div className="stat">
            <dt>Archivos</dt>
            <dd>{formatCount(status.inventory.files)}</dd>
          </div>
          <div className="stat">
            <dt>Carpetas</dt>
            <dd>{formatCount(status.inventory.folders)}</dd>
          </div>
          <div className="stat">
            <dt>Tamaño total</dt>
            <dd>{formatBytes(status.inventory.bytes)}</dd>
          </div>
          {status.inventory.hash_pending > 0 && (
            <div className="stat">
              <dt>Huellas pendientes</dt>
              <dd className="metric-warning">
                {formatCount(status.inventory.hash_pending)}
              </dd>
            </div>
          )}
        </dl>
      )}

      {status.integrity === null ? (
        <p className="integrity integrity-unknown">
          Integridad del proyecto: sin comprobar todavía. Se comprueba cada vez
          que actualizas el estado.
        </p>
      ) : status.integrity.problems.length === 0 ? (
        <p className="integrity integrity-ok">
          <span className="ok">Integridad correcta</span>: base de datos, claves
          y registro de eventos verificados.
        </p>
      ) : (
        <div className="integrity integrity-problems">
          <p>
            <strong>Problemas de integridad detectados:</strong>
          </p>
          <ul className="problems">
            {status.integrity.problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="next-step">
        <h3>Siguiente paso</h3>
        <p>{nextStep.message}</p>
        {nextStep.action === "refresh" && (
          <button
            type="button"
            className="primary"
            onClick={onRefresh}
            disabled={busy}
          >
            {busy ? "Comprobando…" : "Actualizar estado"}
          </button>
        )}
        {nextStep.action === "similarity" && (
          <button
            type="button"
            className="primary"
            onClick={onAnalyzeSimilarity}
            disabled={busy || onAnalyzeSimilarity === undefined}
          >
            {busy ? "Analizando similitud…" : "Analizar similitud y versiones"}
          </button>
        )}
        {nextStep.action === "media" && (
          <button
            type="button"
            className="primary"
            onClick={onAnalyzeMedia}
            disabled={busy || onAnalyzeMedia === undefined}
          >
            {busy ? "Analizando medios…" : "Analizar imagen, audio y vídeo"}
          </button>
        )}
      </div>

      <section
        className="diagnostics"
        aria-labelledby="structural-diagnostics-heading"
      >
        <div className="section-heading">
          <div>
            <h3 id="structural-diagnostics-heading">
              Diagnóstico estructural <span className="milestone">M0.2</span>
            </h3>
            <p>
              Qué carpetas se repiten, cuáles están protegidas y qué reglas se
              aplicaron en el último análisis.
            </p>
          </div>
          {diagnostics?.analysis_complete === true && (
            <span
              className="diagnostic-badge diagnostic-complete"
              role="status"
            >
              Análisis completo
            </span>
          )}
        </div>

        {diagnostics === null ? (
          <p className="diagnostic-status diagnostic-pending" role="status">
            {status.latest_snapshot_id === null
              ? "Pendiente: primero hay que completar el escaneo, el hashing y el análisis."
              : "Pendiente: este snapshot todavía no tiene diagnóstico estructural."}
          </p>
        ) : diagnostics.analysis_complete === false ? (
          <p className="diagnostic-status diagnostic-pending" role="status">
            Análisis estructural pendiente o incompleto. Los recuentos parciales
            no se presentan como resultados finales; actualiza el estado cuando
            termine.
          </p>
        ) : (
          <>
            <div className="diagnostics-grid">
              <section
                className="diagnostic-group"
                aria-labelledby="diagnostic-contexts-heading"
              >
                <h4 id="diagnostic-contexts-heading">Contextos y fronteras</h4>
                <p>
                  Ubicaciones genéricas y límites que la deduplicación no debe
                  disolver.
                </p>
                <dl className="metric-list">
                  <dt>Carpetas genéricas</dt>
                  <dd>{formatCount(diagnostics.generic_folders)}</dd>
                  <dt>Fronteras protegidas</dt>
                  <dd>{formatCount(diagnostics.protected_boundaries)}</dd>
                </dl>
              </section>

              <section
                className="diagnostic-group"
                aria-labelledby="diagnostic-trees-heading"
              >
                <h4 id="diagnostic-trees-heading">Estructura de árboles</h4>
                <p>Firmas completas y relaciones detectadas entre carpetas.</p>
                <dl className="metric-list">
                  <dt>Firmas de carpeta</dt>
                  <dd>{formatCount(diagnostics.folder_signatures)}</dd>
                  <dt>Clones exactos</dt>
                  <dd>{formatCount(diagnostics.exact_tree_clone_sets)}</dd>
                  <dt>Clones parciales</dt>
                  <dd>{formatCount(diagnostics.partial_tree_clones)}</dd>
                  <dt>Árboles embebidos</dt>
                  <dd>{formatCount(diagnostics.embedded_trees)}</dd>
                  <dt>Componentes repetidos</dt>
                  <dd>{formatCount(diagnostics.repeated_components)}</dd>
                  <dt>Límite de candidatos</dt>
                  <dd
                    className={
                      diagnostics.candidate_cap_reached
                        ? "metric-warning"
                        : undefined
                    }
                  >
                    {diagnostics.candidate_cap_reached
                      ? "Alcanzado — resultados no exhaustivos"
                      : "No alcanzado"}
                  </dd>
                </dl>
              </section>

              <section
                className="diagnostic-group"
                aria-labelledby="diagnostic-review-heading"
              >
                <h4 id="diagnostic-review-heading">
                  Reglas, anomalías y revisión
                </h4>
                <p>
                  Decisiones explicables que requieren atención antes de
                  planificar.
                </p>
                <dl className="metric-list">
                  <dt>Coincidencias de reglas</dt>
                  <dd>{formatCount(diagnostics.rule_matches)}</dd>
                  <dt>Anomalías</dt>
                  <dd>{formatCount(diagnostics.anomalies)}</dd>
                  <dt>Anomalías de prioridad alta</dt>
                  <dd
                    className={
                      diagnostics.high_anomalies > 0
                        ? "metric-alert"
                        : undefined
                    }
                  >
                    {formatCount(diagnostics.high_anomalies)}
                  </dd>
                  <dt>Pendientes de revisión</dt>
                  <dd
                    className={
                      diagnostics.pending_review > 0
                        ? "metric-warning"
                        : undefined
                    }
                  >
                    {formatCount(diagnostics.pending_review)}
                  </dd>
                </dl>
              </section>
            </div>
            <p className="diagnostic-note">
              Estos datos son evidencia diagnóstica: una coincidencia o anomalía
              no autoriza por sí sola a eliminar ni consolidar contenido.
            </p>
          </>
        )}
      </section>

      <section className="diagnostics" aria-labelledby="similarity-heading">
        <div className="section-heading">
          <div>
            <h3 id="similarity-heading">
              Similitud y versiones <span className="milestone">M0.3</span>
            </h3>
            <p>
              Archivos que comparten gran parte de su contenido: posibles
              versiones o copias parciales.
            </p>
          </div>
          {similarity !== null && (
            <span
              className="diagnostic-badge diagnostic-complete"
              role="status"
            >
              Evidencia sellada
            </span>
          )}
        </div>

        {similarity === null ? (
          <div className="diagnostic-status diagnostic-pending" role="status">
            <p>
              {diagnostics?.analysis_complete === true
                ? "Aún no se ha ejecutado. Pulsa el botón cuando quieras: no modifica tus archivos."
                : "Pendiente: primero debe terminar el análisis estructural."}
            </p>
            {diagnostics?.analysis_complete === true && (
              <button
                type="button"
                onClick={onAnalyzeSimilarity}
                disabled={busy || onAnalyzeSimilarity === undefined}
              >
                {busy
                  ? "Analizando similitud…"
                  : "Analizar similitud y versiones"}
              </button>
            )}
          </div>
        ) : (
          <>
            <div className="diagnostics-grid">
              <section
                className="diagnostic-group"
                aria-labelledby="similarity-summary-heading"
              >
                <h4 id="similarity-summary-heading">Resumen del corpus</h4>
                <dl className="metric-list">
                  <dt>Contenidos</dt>
                  <dd>{formatCount(similarity.counters.contents_total)}</dd>
                  <dt>Contenidos fragmentados</dt>
                  <dd>{formatCount(similarity.counters.contents_chunked)}</dd>
                  <dt>Chunks</dt>
                  <dd>{formatCount(similarity.counters.chunks_total)}</dd>
                  <dt>Candidatos evaluados</dt>
                  <dd>{formatCount(similarity.counters.candidates_total)}</dd>
                  <dt>Relaciones</dt>
                  <dd>{formatCount(similarity.counters.relations_total)}</dd>
                  <dt>Umbral exacto</dt>
                  <dd>
                    {(similarity.config.options.threshold * 100).toFixed(1)}%
                  </dd>
                  <dt>Evidencia mínima</dt>
                  <dd>
                    {formatCount(similarity.config.options.min_shared_chunks)}{" "}
                    chunks /{" "}
                    {formatCount(similarity.config.options.min_shared_bytes)}{" "}
                    bytes
                  </dd>
                  <dt>Máximo de candidatos</dt>
                  <dd>
                    {formatCount(similarity.config.options.max_candidates)}
                  </dd>
                  <dt>Límite de candidatos</dt>
                  <dd
                    className={
                      similarity.candidate_cap_reached
                        ? "metric-warning"
                        : undefined
                    }
                  >
                    {similarity.candidate_cap_reached
                      ? "Alcanzado — resultados no exhaustivos"
                      : "No alcanzado"}
                  </dd>
                </dl>
              </section>
              <section
                className="diagnostic-group similarity-relations"
                aria-labelledby="similarity-relations-heading"
              >
                <h4 id="similarity-relations-heading">
                  Relaciones principales
                </h4>
                {similarity.relationships.length === 0 ? (
                  <p>Ningún par superó los umbrales configurados.</p>
                ) : (
                  <ol>
                    {similarity.relationships.map((relation) => (
                      <li key={relation.id}>
                        <strong>{SIMILARITY_KIND_LABELS[relation.kind]}</strong>{" "}
                        <code className="relation-code">{relation.kind}</code> —{" "}
                        {(relation.similarity * 100).toFixed(1)}%
                        <br />
                        <code>{relation.path_a}</code>
                        <br />
                        <span aria-hidden="true">↔</span>{" "}
                        <code>{relation.path_b}</code>
                        <br />
                        <small>
                          {formatCount(relation.shared_chunks)} chunks y{" "}
                          {formatCount(relation.shared_bytes)} bytes compartidos
                        </small>
                      </li>
                    ))}
                  </ol>
                )}
                {similarity.relationships_truncated && (
                  <p className="metric-warning">
                    La vista muestra solo las primeras{" "}
                    {formatCount(similarity.relationships.length)} relaciones.
                  </p>
                )}
              </section>
            </div>
            <p className="diagnostic-note">
              Evidencia de revisión: la similitud no equivale a identidad y
              nunca autoriza por sí sola una eliminación, consolidación ni
              operación del plan.
            </p>
          </>
        )}
      </section>

      <section className="diagnostics" aria-labelledby="media-heading">
        <div className="section-heading">
          <div>
            <h3 id="media-heading">
              Inteligencia multimedia <span className="milestone">M0.5</span>
            </h3>
            <p>
              Fotos, audios y vídeos que parecen ser el mismo material aunque el
              archivo sea distinto.
            </p>
          </div>
          {media !== null && (
            <span
              className="diagnostic-badge diagnostic-complete"
              role="status"
            >
              Evidencia sellada
            </span>
          )}
        </div>

        {media === null ? (
          <div className="diagnostic-status diagnostic-pending" role="status">
            <p>
              {diagnostics?.analysis_complete === true
                ? "Aún no se ha ejecutado. Pulsa el botón cuando quieras: no modifica tus archivos."
                : "Pendiente: primero debe terminar el análisis estructural."}
            </p>
            {diagnostics?.analysis_complete === true && (
              <button
                type="button"
                onClick={onAnalyzeMedia}
                disabled={busy || onAnalyzeMedia === undefined}
              >
                {busy ? "Analizando medios…" : "Analizar imagen, audio y vídeo"}
              </button>
            )}
          </div>
        ) : (
          <>
            <div className="diagnostics-grid">
              <section
                className="diagnostic-group"
                aria-labelledby="media-summary-heading"
              >
                <h4 id="media-summary-heading">Resumen del corpus</h4>
                <dl className="metric-list">
                  <dt>Contenidos multimedia</dt>
                  <dd>{formatCount(media.counters.contents_total)}</dd>
                  <dt>Analizados</dt>
                  <dd>{formatCount(media.counters.contents_analyzed)}</dd>
                  <dt>Limitados</dt>
                  <dd>{formatCount(media.counters.contents_limited)}</dd>
                  <dt>Fallidos</dt>
                  <dd
                    className={
                      media.counters.contents_failed > 0
                        ? "metric-warning"
                        : undefined
                    }
                  >
                    {formatCount(media.counters.contents_failed)}
                  </dd>
                  <dt>Pares comparados</dt>
                  <dd>{formatCount(media.counters.pairs_compared)}</dd>
                  <dt>Relaciones</dt>
                  <dd>{formatCount(media.counters.relations_total)}</dd>
                  <dt>Límite de pares</dt>
                  <dd
                    className={
                      media.pair_cap_reached ? "metric-warning" : undefined
                    }
                  >
                    {media.pair_cap_reached
                      ? "Alcanzado — resultados no exhaustivos"
                      : "No alcanzado"}
                  </dd>
                </dl>
              </section>
              <section
                className="diagnostic-group similarity-relations"
                aria-labelledby="media-relations-heading"
              >
                <h4 id="media-relations-heading">Relaciones de revisión</h4>
                {media.relations.length === 0 ? (
                  <p>Ningún par superó los umbrales perceptuales del motor.</p>
                ) : (
                  <ol>
                    {media.relations.map((relation) => (
                      <li key={`${relation.content_a}-${relation.content_b}`}>
                        <strong>
                          {MEDIA_RELATION_LABELS[relation.relation]}
                        </strong>{" "}
                        <code className="relation-code">
                          {relation.relation}
                        </code>{" "}
                        — {(relation.score_millionths / 10_000).toFixed(1)}%
                        <br />
                        <code>{relation.path_a ?? relation.content_a}</code>
                        <br />
                        <span aria-hidden="true">↔</span>{" "}
                        <code>{relation.path_b ?? relation.content_b}</code>
                      </li>
                    ))}
                  </ol>
                )}
                {media.relations_truncated && (
                  <p className="metric-warning">
                    La vista muestra solo las primeras{" "}
                    {formatCount(media.relations.length)} relaciones.
                  </p>
                )}
              </section>
            </div>
            <p className="diagnostic-note">
              Evidencia de revisión: una coincidencia perceptual señala posibles
              rediciones del mismo material y nunca autoriza por sí sola una
              eliminación, consolidación ni operación del plan.
            </p>
          </>
        )}
      </section>

      <ContentIntelligenceView projectDir={status.project_dir} />

      <details className="tech-details">
        <summary>Detalles técnicos del proyecto</summary>
        <dl className="facts">
          <dt>Identificador</dt>
          <dd>
            <code>{status.project_id}</code>
          </dd>
          <dt>Perfil</dt>
          <dd>{status.profile}</dd>
          <dt>Versión de la app</dt>
          <dd>{status.app_version}</dd>
          <dt>Creado</dt>
          <dd>{status.created_at}</dd>
          <dt>Actualizado</dt>
          <dd>{status.updated_at}</dd>
          <dt>Carpeta del proyecto</dt>
          <dd>
            <code>{status.project_dir}</code>
          </dd>
          <dt>Carpeta de resultados</dt>
          <dd>
            <code>{status.output_root}</code>
          </dd>
          <dt>Carpeta de auditoría</dt>
          <dd>
            <code>{status.audit_root}</code>
          </dd>
          <dt>Orígenes</dt>
          <dd>
            {status.source_roots.length === 0 ? (
              "(ninguno registrado)"
            ) : (
              <ul>
                {status.source_roots.map((root) => (
                  <li key={root.id}>
                    <code>{root.absolute_path}</code> — {root.filesystem}, solo
                    lectura
                  </li>
                ))}
              </ul>
            )}
          </dd>
          {status.latest_snapshot_id !== null && status.inventory !== null && (
            <>
              <dt>Snapshot</dt>
              <dd>
                <code>{status.latest_snapshot_id}</code>
              </dd>
              <dt>Inventario</dt>
              <dd>
                {status.inventory.files} archivo(s), {status.inventory.folders}{" "}
                carpeta(s), {status.inventory.bytes} byte(s),{" "}
                {status.inventory.scan_errors} error(es),{" "}
                {status.inventory.reparse_points} enlace(s) no seguido(s)
              </dd>
              <dt>Hashing</dt>
              <dd>
                {status.inventory.hash_done} completado(s),{" "}
                {status.inventory.hash_pending} pendiente(s),{" "}
                {status.inventory.hash_failed} fallido(s),{" "}
                {status.inventory.hash_source_changed} con origen modificado
              </dd>
            </>
          )}
          <dt>Registro de eventos</dt>
          <dd>
            {status.event_count} evento(s)
            {status.last_event !== null && (
              <>
                {" — último "}
                <code>{status.last_event.event_type}</code> (
                {status.last_event.timestamp}, {status.last_event.actor})
              </>
            )}
          </dd>
        </dl>
      </details>

      <div className="actions">
        <button type="button" onClick={onRefresh} disabled={busy}>
          {busy ? "Comprobando…" : "Actualizar estado"}
        </button>
        <button type="button" onClick={onBack} disabled={busy}>
          Volver
        </button>
      </div>
    </section>
  );
}
