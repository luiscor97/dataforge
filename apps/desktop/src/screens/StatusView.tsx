import type { ProjectStatus } from "../types";

const COUNT_FORMAT = new Intl.NumberFormat("es-ES");

function formatCount(value: number): string {
  return COUNT_FORMAT.format(value);
}

interface StatusViewProps {
  status: ProjectStatus;
  busy: boolean;
  onRefresh: () => void;
  onBack: () => void;
}

export function StatusView({ status, busy, onRefresh, onBack }: StatusViewProps): React.JSX.Element {
  const diagnostics = status.structural_diagnostics ?? null;

  return (
    <section className="panel">
      <h2>
        {status.name} <span className={`state state-${status.state.toLowerCase()}`}>{status.state}</span>
      </h2>
      <dl className="facts">
        <dt>Identificador</dt>
        <dd>
          <code>{status.project_id}</code>
        </dd>
        <dt>Perfil</dt>
        <dd>{status.profile}</dd>
        <dt>Creado</dt>
        <dd>{status.created_at}</dd>
        <dt>Actualizado</dt>
        <dd>{status.updated_at}</dd>
        <dt>Carpeta</dt>
        <dd>
          <code>{status.project_dir}</code>
        </dd>
        <dt>Salida</dt>
        <dd>
          <code>{status.output_root}</code>
        </dd>
        <dt>Auditoría</dt>
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
                  <code>{root.absolute_path}</code> — {root.filesystem}, solo lectura
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
              {status.inventory.files} archivo(s), {status.inventory.folders} carpeta(s),{" "}
              {status.inventory.bytes} byte(s), {status.inventory.scan_errors} error(es),{" "}
              {status.inventory.reparse_points} enlace(s) no seguido(s)
            </dd>
            <dt>Hashing</dt>
            <dd>
              {status.inventory.hash_done} completado(s), {status.inventory.hash_pending}{" "}
              pendiente(s), {status.inventory.hash_failed} fallido(s),{" "}
              {status.inventory.hash_source_changed} con origen modificado
            </dd>
          </>
        )}
        <dt>Ledger</dt>
        <dd>
          {status.event_count} evento(s)
          {status.last_event !== null && (
            <>
              {" — último "}
              <code>{status.last_event.event_type}</code> ({status.last_event.timestamp},{" "}
              {status.last_event.actor})
            </>
          )}
        </dd>
        <dt>Integridad</dt>
        <dd>
          {status.integrity === null ? (
            "sin comprobar (usa Actualizar para ejecutar la verificación)"
          ) : status.integrity.problems.length === 0 ? (
            <span className="ok">correcta: base de datos, claves foráneas, migraciones y ledger</span>
          ) : (
            <ul className="problems">
              {status.integrity.problems.map((problem) => (
                <li key={problem}>{problem}</li>
              ))}
            </ul>
          )}
        </dd>
      </dl>
      <section className="diagnostics" aria-labelledby="structural-diagnostics-heading">
        <div className="section-heading">
          <div>
            <h3 id="structural-diagnostics-heading">Diagnóstico estructural M0.2</h3>
            <p>
              Evidencia sobre contextos, árboles repetidos y reglas aplicadas al último snapshot.
            </p>
          </div>
          {diagnostics?.analysis_complete === true && (
            <span className="diagnostic-badge diagnostic-complete">Análisis completo</span>
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
            Análisis estructural pendiente o incompleto. Los recuentos parciales no se presentan
            como resultados finales; actualiza el estado cuando termine.
          </p>
        ) : (
          <>
            <div className="diagnostics-grid">
              <section className="diagnostic-group" aria-labelledby="diagnostic-contexts-heading">
                <h4 id="diagnostic-contexts-heading">Contextos y fronteras</h4>
                <p>Ubicaciones genéricas y límites que la deduplicación no debe disolver.</p>
                <dl className="metric-list">
                  <dt>Carpetas genéricas</dt>
                  <dd>{formatCount(diagnostics.generic_folders)}</dd>
                  <dt>Fronteras protegidas</dt>
                  <dd>{formatCount(diagnostics.protected_boundaries)}</dd>
                </dl>
              </section>

              <section className="diagnostic-group" aria-labelledby="diagnostic-trees-heading">
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
                </dl>
              </section>

              <section className="diagnostic-group" aria-labelledby="diagnostic-review-heading">
                <h4 id="diagnostic-review-heading">Reglas, anomalías y revisión</h4>
                <p>Decisiones explicables que requieren atención antes de planificar.</p>
                <dl className="metric-list">
                  <dt>Coincidencias de reglas</dt>
                  <dd>{formatCount(diagnostics.rule_matches)}</dd>
                  <dt>Anomalías</dt>
                  <dd>{formatCount(diagnostics.anomalies)}</dd>
                  <dt>Anomalías de prioridad alta</dt>
                  <dd className={diagnostics.high_anomalies > 0 ? "metric-alert" : undefined}>
                    {formatCount(diagnostics.high_anomalies)}
                  </dd>
                  <dt>Pendientes de revisión</dt>
                  <dd className={diagnostics.pending_review > 0 ? "metric-warning" : undefined}>
                    {formatCount(diagnostics.pending_review)}
                  </dd>
                </dl>
              </section>
            </div>
            <p className="diagnostic-note">
              Estos datos son evidencia diagnóstica: una coincidencia o anomalía no autoriza por
              sí sola a eliminar ni consolidar contenido.
            </p>
          </>
        )}
      </section>
      <div className="actions">
        <button type="button" onClick={onRefresh} disabled={busy}>
          {busy ? "Comprobando…" : "Actualizar estado e integridad"}
        </button>
        <button type="button" onClick={onBack} disabled={busy}>
          Volver
        </button>
      </div>
    </section>
  );
}
