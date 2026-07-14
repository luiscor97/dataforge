import type { ProjectStatus } from "../types";

interface StatusViewProps {
  status: ProjectStatus;
  busy: boolean;
  onRefresh: () => void;
  onBack: () => void;
}

export function StatusView({ status, busy, onRefresh, onBack }: StatusViewProps): React.JSX.Element {
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
