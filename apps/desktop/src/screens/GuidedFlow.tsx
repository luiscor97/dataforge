import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

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
import { ErrorAlert } from "../components/ErrorAlert";
import { type ErrorDto, isErrorDto } from "../types";

/**
 * The guided flow: three decisions instead of seven commands.
 *
 * The engine's pipeline is scan -> hash -> analyze -> plan -> approve ->
 * execute -> verify. Someone who just wants their documents in order should
 * never meet those words, so this screen chains them and speaks about what
 * they mean: looking at your files, copying them, checking the copy.
 *
 * What it must never do is invent reassurance. Progress here is deliberately
 * indeterminate: the engine reports no percentage, and a bar that crept
 * forward on a timer would be a lie told exactly when the user is most
 * anxious. Instead each stage says what is happening and warns when it may
 * take a while.
 */

type Stage =
  | { kind: "setup" }
  | { kind: "working"; label: string; detail: string }
  | { kind: "review"; findings: Findings }
  | { kind: "done"; result: Result };

interface Findings {
  files: number;
  folders: number;
  duplicateSets: number;
  operations: number;
  scanErrors: number;
  outputRoot: string;
}

interface Result {
  copied: number;
  bytes: number;
  verdict: string;
  problems: number;
  warnings: number;
  outputRoot: string;
}

/**
 * Spanish digit grouping, done explicitly rather than through
 * `toLocaleString`: that depends on the runtime's locale data, so the same
 * build can show "1.234" in one environment and "1234" in another. A count
 * the user is asked to trust should not change shape by accident.
 */
function humanCount(n: number): string {
  const [whole, fraction] = Math.abs(n).toFixed(0).split(".");
  const grouped = (whole ?? "0").replace(/\B(?=(\d{3})+(?!\d))/g, ".");
  const sign = n < 0 ? "-" : "";
  return fraction === undefined ? `${sign}${grouped}` : `${sign}${grouped},${fraction}`;
}

/** Human-readable size; the engine speaks bytes, people do not. */
function humanBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = Math.round(value * 10) / 10;
  const whole = Math.trunc(rounded);
  const decimal = Math.round((rounded - whole) * 10);
  const shown = decimal === 0 ? humanCount(whole) : `${humanCount(whole)},${decimal}`;
  return `${shown} ${units[unit]}`;
}

/** Last path segment, for showing a folder by name instead of full path. */
function folderName(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

interface Props {
  onOpenAdvanced: (projectDir: string) => void;
  onExit: () => void;
}

export function GuidedFlow({ onOpenAdvanced, onExit }: Props): React.JSX.Element {
  const [stage, setStage] = useState<Stage>({ kind: "setup" });
  const [source, setSource] = useState("");
  const [destination, setDestination] = useState("");
  const [error, setError] = useState<ErrorDto | null>(null);
  const [dropTarget, setDropTarget] = useState<"source" | "destination" | null>(
    null,
  );
  const projectDir = useRef<string>("");
  // Which field a drop should fill. A ref because the webview listener is
  // registered once and would otherwise capture a stale value.
  const activeField = useRef<"source" | "destination">("source");

  // Dropping a folder onto the window beats typing a path from memory, and
  // Tauri gives us this without any extra dependency. Everything here is
  // wrapped because `getCurrentWebview` throws *synchronously* when no Tauri
  // runtime is present (a browser preview, a test harness): drag and drop is
  // a convenience, and losing it must never take the screen down with it.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") {
            setDropTarget(activeField.current);
            return;
          }
          setDropTarget(null);
          if (event.payload.type === "drop") {
            const dropped = event.payload.paths.at(0);
            if (dropped !== undefined) {
              if (activeField.current === "source") {
                setSource(dropped);
              } else {
                setDestination(dropped);
              }
            }
          }
        })
        .then((fn) => {
          unlisten = fn;
        })
        .catch(() => {
          // Typing the path still works.
        });
    } catch {
      // Same: no drag and drop, but the screen renders and works.
    }
    return () => unlisten?.();
  }, []);

  const fail = useCallback((failure: unknown) => {
    setError(
      isErrorDto(failure)
        ? failure
        : { code: "unknown", message: String(failure) },
    );
    setStage({ kind: "setup" });
  }, []);

  /** Stages 1-4: look at the files and work out what to copy. */
  const startReview = useCallback(async () => {
    setError(null);
    const trimmedSource = source.trim();
    const trimmedDestination = destination.trim();
    if (trimmedSource === "" || trimmedDestination === "") {
      setError({
        code: "validation",
        message: "Indica la carpeta que quieres ordenar y dónde guardar el resultado.",
      });
      return;
    }
    try {
      setStage({
        kind: "working",
        label: "Preparando…",
        detail: "Creando el proyecto donde guardar todo lo que descubramos.",
      });
      // The project lives beside the destination, so the user keeps one place
      // to look; the engine refuses overlapping roots and says so.
      const dir = `${trimmedDestination.replace(/[\\/]+$/, "")}-dataforge`;
      const created = await createProject({
        name: folderName(trimmedSource),
        project_dir: dir,
        output_root: trimmedDestination,
        profile: "generic",
        source_roots: [trimmedSource],
      });
      projectDir.current = created.project_dir;

      setStage({
        kind: "working",
        label: "Mirando qué archivos tienes…",
        detail: "Solo los leemos. Nada se modifica.",
      });
      const scan = await scanProject(projectDir.current);

      setStage({
        kind: "working",
        label: "Identificando cada archivo…",
        detail:
          "Calculamos una huella única de cada uno. Con muchos archivos esto puede tardar unos minutos.",
      });
      await hashProject(projectDir.current);

      setStage({
        kind: "working",
        label: "Buscando duplicados…",
        detail: "Comparamos las huellas para encontrar copias exactas.",
      });
      const analyzed = await analyzeProject(projectDir.current);

      setStage({
        kind: "working",
        label: "Preparando la propuesta…",
        detail: "Decidimos qué copiar y a dónde.",
      });
      const plan = await createPlan(projectDir.current);

      setStage({
        kind: "review",
        findings: {
          files: scan.files,
          folders: scan.folders,
          scanErrors: scan.errors,
          duplicateSets: analyzed.duplicate_sets,
          operations: plan.operations,
          outputRoot: trimmedDestination,
        },
      });
    } catch (failure) {
      fail(failure);
    }
  }, [source, destination, fail]);

  /** Stages 5-7: freeze the plan, copy, and check the copy independently. */
  const startCopy = useCallback(
    async (findings: Findings) => {
      setError(null);
      try {
        setStage({
          kind: "working",
          label: "Copiando tus archivos…",
          detail:
            "Cada archivo se copia y se comprueba al vuelo. Puedes cerrar y continuar más tarde: no se pierde el trabajo hecho.",
        });
        await approvePlan(projectDir.current);
        const executed = await executePlan(projectDir.current);

        setStage({
          kind: "working",
          label: "Comprobando la copia…",
          detail:
            "Volvemos a leer lo copiado y lo contrastamos con el original, sin fiarnos de lo que acabamos de hacer.",
        });
        const verified = await verifyProject(projectDir.current);

        setStage({
          kind: "done",
          result: {
            copied: executed.completed,
            bytes: executed.bytes_copied,
            verdict: verified.verdict,
            problems: verified.problems,
            warnings: verified.warnings,
            outputRoot: findings.outputRoot,
          },
        });
      } catch (failure) {
        fail(failure);
      }
    },
    [fail],
  );

  const busy = stage.kind === "working";

  return (
    <section className="panel guided" aria-busy={busy}>
      {error !== null && <ErrorAlert error={error} />}

      {stage.kind === "setup" && (
        <>
          <h2>Ordenar mis documentos</h2>
          <p className="hint">
            Elige la carpeta desordenada y dónde quieres el resultado. DataForge
            la examina y crea una copia ordenada y verificada.{" "}
            <strong>Nunca modifica ni borra tus archivos originales.</strong>
          </p>

          <form
            onSubmit={(event) => {
              event.preventDefault();
              void startReview();
            }}
          >
            <label
              className={`dropfield${dropTarget === "source" ? " dropfield-active" : ""}`}
            >
              Carpeta que quieres ordenar
              <input
                value={source}
                onChange={(e) => setSource(e.target.value)}
                onFocus={() => {
                  activeField.current = "source";
                }}
                placeholder="Arrastra la carpeta aquí, o escribe su ruta"
                required
              />
              <span className="field-help">
                Puedes arrastrar la carpeta desde el explorador hasta esta
                ventana. Solo la leeremos.
              </span>
            </label>

            <label
              className={`dropfield${dropTarget === "destination" ? " dropfield-active" : ""}`}
            >
              Dónde guardar el resultado
              <input
                value={destination}
                onChange={(e) => setDestination(e.target.value)}
                onFocus={() => {
                  activeField.current = "destination";
                }}
                placeholder="Arrastra una carpeta vacía, o escribe su ruta"
                required
              />
              <span className="field-help">
                Debe ser una carpeta distinta de la anterior. Ahí aparecerá tu
                copia ordenada.
              </span>
            </label>

            <div className="actions">
              <button type="submit" className="primary">
                Examinar mis archivos
              </button>
              <button type="button" onClick={onExit}>
                Volver
              </button>
            </div>
          </form>
        </>
      )}

      {stage.kind === "working" && (
        <div className="working" role="status" aria-live="polite">
          <h2>{stage.label}</h2>
          <div className="working-bar" aria-hidden="true">
            <span />
          </div>
          <p className="hint">{stage.detail}</p>
        </div>
      )}

      {stage.kind === "review" && (
        <>
          <h2>Esto he encontrado</h2>
          <div className="tiles">
            <div className="tile">
              <span className="tile-label">Archivos</span>
              <strong>{humanCount(stage.findings.files)}</strong>
            </div>
            <div className="tile">
              <span className="tile-label">Carpetas</span>
              <strong>{humanCount(stage.findings.folders)}</strong>
            </div>
            <div className="tile">
              <span className="tile-label">Grupos de copias repetidas</span>
              <strong>{humanCount(stage.findings.duplicateSets)}</strong>
            </div>
          </div>

          {stage.findings.scanErrors > 0 && (
            <p className="notice notice-warning">
              {humanCount(stage.findings.scanErrors)}{" "}
              {stage.findings.scanErrors === 1
                ? "archivo no se pudo leer"
                : "archivos no se pudieron leer"}{" "}
              (por permisos o porque estaban en uso). Quedan anotados y no se
              copiarán; el resto sí.
            </p>
          )}

          <h3>Esto voy a hacer</h3>
          <p>
            Copiar <strong>{humanCount(stage.findings.operations)}</strong>{" "}
            elementos a <code>{stage.findings.outputRoot}</code>, comprobando
            uno por uno que la copia es idéntica al original.
          </p>
          <p className="hint">
            Las copias repetidas se conservan todas y quedan señaladas en el
            informe: no se borra nada por nuestra cuenta. Tus archivos
            originales no se tocan en ningún momento.
          </p>

          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => void startCopy(stage.findings)}
            >
              Hacer la copia
            </button>
            <button type="button" onClick={onExit}>
              Ahora no
            </button>
          </div>
        </>
      )}

      {stage.kind === "done" && (
        <>
          <h2>
            {stage.result.verdict === "COMPLETED"
              ? "Listo. Tu copia está verificada."
              : stage.result.verdict === "COMPLETED_WITH_WARNINGS"
                ? "Copia terminada, con avisos."
                : "La comprobación ha encontrado problemas."}
          </h2>
          <div className="tiles">
            <div className="tile">
              <span className="tile-label">Elementos copiados</span>
              <strong>{humanCount(stage.result.copied)}</strong>
            </div>
            <div className="tile">
              <span className="tile-label">Tamaño</span>
              <strong>{humanBytes(stage.result.bytes)}</strong>
            </div>
          </div>

          {stage.result.verdict === "COMPLETED" && (
            <p className="notice notice-ok">
              Cada archivo copiado se ha vuelto a leer y coincide exactamente
              con el original. Tus archivos originales están intactos.
            </p>
          )}
          {stage.result.verdict === "COMPLETED_WITH_WARNINGS" && (
            <p className="notice notice-warning">
              La copia se completó, pero hay{" "}
              {humanCount(stage.result.warnings)}{" "}
              {stage.result.warnings === 1 ? "aviso" : "avisos"} que conviene
              revisar. Tus originales siguen intactos.
            </p>
          )}
          {stage.result.verdict === "FAILED" && (
            <p className="notice notice-danger">
              Se han detectado {humanCount(stage.result.problems)}{" "}
              {stage.result.problems === 1 ? "problema" : "problemas"}. Tus
              archivos originales están intactos: puedes revisar el detalle y
              volver a intentarlo.
            </p>
          )}

          <p>
            Tu copia está en <code>{stage.result.outputRoot}</code>.
          </p>

          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => onOpenAdvanced(projectDir.current)}
            >
              Ver el detalle
            </button>
            <button type="button" onClick={onExit}>
              Terminar
            </button>
          </div>
        </>
      )}
    </section>
  );
}
