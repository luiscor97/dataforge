import { useCallback, useEffect, useState } from "react";

import type { ErrorDto, PlanDestinationTree } from "../types";
import { isErrorDto } from "../types";

const COUNT_FORMAT = new Intl.NumberFormat("es-ES");
const SIZE_FORMAT = new Intl.NumberFormat("es-ES", {
  maximumFractionDigits: 1,
});
const SIZE_UNITS = ["bytes", "KB", "MB", "GB", "TB", "PB"] as const;

function formatBytes(value: number): string {
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < SIZE_UNITS.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return unit === 0
    ? `${COUNT_FORMAT.format(value)} ${SIZE_UNITS[0]}`
    : `${SIZE_FORMAT.format(size)} ${SIZE_UNITS[unit]}`;
}

/**
 * Plain-language names for the engine's destination roots.
 *
 * Presentation only: the folder that gets created is the one the plan says,
 * shown verbatim next to the label. Renaming it here would be lying about
 * what the user will find on disk.
 */
const ROOT_LABELS: Record<string, string> = {
  "90_DataForge_Review": "Necesita que lo revises",
  "95_DataForge_Separated": "Apartado por seguridad",
  "98_DataForge_Temporary": "Archivos temporales del sistema",
};

interface PlanTreeViewProps {
  /** Loads the tree; injected so the screen stays free of transport code. */
  load: () => Promise<PlanDestinationTree>;
}

/**
 * The output tree the plan would produce, shown before it is approved.
 *
 * Approving freezes an immutable manifest, so this is the last moment at
 * which the shape of the result can still be judged. Counts alone cannot
 * answer "where does my data end up".
 */
export function PlanTreeView({ load }: PlanTreeViewProps): React.JSX.Element {
  const [tree, setTree] = useState<PlanDestinationTree | null>(null);
  const [error, setError] = useState<ErrorDto | null>(null);
  const [loading, setLoading] = useState(true);

  const run = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setTree(await load());
    } catch (failure) {
      setError(
        isErrorDto(failure)
          ? failure
          : { code: "unknown", message: String(failure) },
      );
    } finally {
      setLoading(false);
    }
  }, [load]);

  useEffect(() => {
    void run();
  }, [run]);

  if (loading) {
    return (
      <p className="plan-tree-loading">Calculando dónde iría cada archivo…</p>
    );
  }
  if (error !== null) {
    return (
      <p className="plan-tree-error" role="alert">
        No se pudo calcular el destino: {error.message}
      </p>
    );
  }
  if (tree === null) {
    return <></>;
  }

  const roots = tree.nodes.filter((node) => node.depth === 1);
  return (
    <section className="plan-tree" aria-labelledby="plan-tree-heading">
      <h4 id="plan-tree-heading">Qué se va a crear en el destino</h4>
      <p className="plan-tree-root">
        Todo esto se escribirá dentro de <code>{tree.output_root}</code>. Tus
        originales no se tocan.
      </p>
      <p className="plan-tree-total">
        <strong>{COUNT_FORMAT.format(tree.files)}</strong> archivo(s) y{" "}
        <strong>{COUNT_FORMAT.format(tree.directories)}</strong> carpeta(s),{" "}
        <strong>{formatBytes(tree.bytes)}</strong> en total.
      </p>

      {tree.without_destination > 0 && (
        <p className="plan-tree-warning" role="alert">
          Atención: {COUNT_FORMAT.format(tree.without_destination)} copia(s) no
          tienen destino registrado. No apruebes hasta revisarlo.
        </p>
      )}

      <ul className="plan-tree-roots">
        {roots.map((node) => (
          <li key={node.prefix}>
            <div className="plan-tree-name">
              <code>{node.prefix}\</code>
              {ROOT_LABELS[node.prefix] !== undefined && (
                <span className="plan-tree-label">
                  {ROOT_LABELS[node.prefix]}
                </span>
              )}
            </div>
            <div className="plan-tree-figures">
              {COUNT_FORMAT.format(node.files)} archivo(s) ·{" "}
              {formatBytes(node.bytes)}
            </div>
            {node.sample !== null && (
              <div className="plan-tree-sample">
                Por ejemplo: <code>{node.sample}</code>
              </div>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
