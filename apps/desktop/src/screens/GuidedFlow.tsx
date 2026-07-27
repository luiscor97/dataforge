import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { invoke } from "@tauri-apps/api/core";

import {
  analyzeProject,
  approvePlan,
  createPlan,
  createProject,
  destinationGuarantees,
  executePlan,
  executePlanOnDegradedDestination,
  hashProject,
  openProject,
  projectStatus,
  scanProject,
  scanProgress,
  validatePlan,
  verifyProject,
} from "../api";
import { ErrorAlert } from "../components/ErrorAlert";
import { type ErrorDto, type ProjectStatus, isErrorDto } from "../types";
import { type Resume, resumeFrom } from "./resume";

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
 * take a while. For the same reason, a number this screen did not obtain from
 * the engine in this run is not shown at all rather than shown as zero.
 */

type Stage =
  | { kind: "setup" }
  | { kind: "working"; label: string; detail: string }
  | { kind: "review"; findings: Findings }
  | {
      kind: "resumeCopy";
      resume: Resume & { kind: "copy" };
      outputRoot: string;
    }
  | {
      kind: "outOfSpace";
      copied: number;
      bytes: number;
      pending: number;
      outputRoot: string;
      /** Carried so continuing does not ask for the ADR-0036 grant twice. */
      degraded: boolean;
    }
  | {
      kind: "degraded";
      filesystem: string;
      resume: Resume & { kind: "copy" };
      outputRoot: string;
    }
  | { kind: "finished"; state: string }
  | { kind: "manual"; state: string }
  | { kind: "done"; result: Result };

interface Findings {
  files: number;
  folders: number;
  /** `null` when this run reused an analysis it did not perform itself. */
  duplicateSets: number | null;
  operations: number;
  scanErrors: number;
  outputRoot: string;
  /** True when the flow continued work left over from an earlier session. */
  resumed: boolean;
}

interface Result {
  /** `null` when the copy itself finished in an earlier session. */
  copied: number | null;
  bytes: number | null;
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
  return fraction === undefined
    ? `${sign}${grouped}`
    : `${sign}${grouped},${fraction}`;
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
  const shown =
    decimal === 0 ? humanCount(whole) : `${humanCount(whole)},${decimal}`;
  return `${shown} ${units[unit]}`;
}

/** Last path segment, for showing a folder by name instead of full path. */
function folderName(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/**
 * Loose path equality, used only to check that a reopened project describes
 * the folders the user just typed.
 *
 * Separators and trailing slashes are noise, and Windows paths are
 * case-insensitive. This is deliberately not a security boundary — the engine
 * owns path containment (`df-fs-safety`); here a false mismatch merely asks the
 * user to pick another destination, which is the safe direction to err in.
 */
function samePath(a: string, b: string): boolean {
  const normalise = (p: string): string =>
    p
      .replace(/[\\/]+/g, "\\")
      .replace(/\\+$/, "")
      .toLowerCase();
  return normalise(a) === normalise(b);
}

interface Props {
  onOpenAdvanced: (projectDir: string) => void;
  onExit: () => void;
}

export function GuidedFlow({
  onOpenAdvanced,
  onExit,
}: Props): React.JSX.Element {
  const [stage, setStage] = useState<Stage>({ kind: "setup" });
  const [source, setSource] = useState("");
  const [destination, setDestination] = useState("");
  const [error, setError] = useState<ErrorDto | null>(null);
  const [dropTarget, setDropTarget] = useState<"source" | "destination" | null>(
    null,
  );
  const projectDir = useRef<string>("");
  // Which field a drop fills when the pointer is over neither box — the last
  // one the user touched. Refs because the webview listener is registered once
  // and would otherwise capture stale values.
  const activeField = useRef<"source" | "destination">("source");
  const sourceBox = useRef<HTMLLabelElement | null>(null);
  const destinationBox = useRef<HTMLLabelElement | null>(null);

  // A native folder picker, offered next to drag-and-drop and typing. Like the
  // drag listener below it is best-effort: `invoke` rejects when no Tauri
  // runtime is present (browser preview, tests), and a missing picker must
  // never take the screen down — the text field and drag still work.
  const browse = useCallback(async (field: "source" | "destination") => {
    activeField.current = field;
    try {
      const picked = await invoke<string | string[] | null>(
        "plugin:dialog|open",
        {
          options: {
            directory: true,
            multiple: false,
            title:
              field === "source"
                ? "Elige la carpeta que quieres ordenar"
                : "Elige dónde guardar el resultado",
          },
        },
      );
      if (typeof picked === "string") {
        if (field === "source") setSource(picked);
        else setDestination(picked);
      }
    } catch {
      // Picker unavailable; drag-and-drop and manual entry remain.
    }
  }, []);

  // Dropping a folder onto the window beats typing a path from memory, and
  // Tauri gives us this without any extra dependency. Everything here is
  // wrapped because `getCurrentWebview` throws *synchronously* when no Tauri
  // runtime is present (a browser preview, a test harness): drag and drop is
  // a convenience, and losing it must never take the screen down with it.
  useEffect(() => {
    // The drag events carry a position in physical pixels, while the DOM
    // measures in CSS pixels; on a scaled display those differ. Hit-testing
    // the two boxes is what makes the folder land where the user aimed it
    // rather than in whichever field happened to be focused.
    const fieldUnder = (position: {
      x: number;
      y: number;
    }): "source" | "destination" | null => {
      const ratio = window.devicePixelRatio || 1;
      const x = position.x / ratio;
      const y = position.y / ratio;
      const hit = (box: HTMLLabelElement | null): boolean => {
        if (box === null) return false;
        const rect = box.getBoundingClientRect();
        return (
          x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
        );
      };
      if (hit(sourceBox.current)) return "source";
      if (hit(destinationBox.current)) return "destination";
      return null;
    };

    let unlisten: (() => void) | undefined;
    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") {
            setDropTarget(
              fieldUnder(event.payload.position) ?? activeField.current,
            );
            return;
          }
          setDropTarget(null);
          if (event.payload.type === "drop") {
            const dropped = event.payload.paths.at(0);
            if (dropped !== undefined) {
              const field =
                fieldUnder(event.payload.position) ?? activeField.current;
              if (field === "source") {
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

  /**
   * Find the project for these two folders, creating it or picking up the one
   * an earlier session left behind.
   *
   * Reopening is only safe once we have checked that the existing project
   * describes the *same* folders: a project directory is derived from the
   * destination, so a user who reuses a destination with a different origin
   * would otherwise silently continue somebody else's work.
   */
  const resolveProject = useCallback(
    async (
      dir: string,
      src: string,
      out: string,
    ): Promise<{ status: ProjectStatus; resumed: boolean }> => {
      try {
        return {
          status: await createProject({
            name: folderName(src),
            project_dir: dir,
            output_root: out,
            profile: "generic",
            source_roots: [src],
          }),
          resumed: false,
        };
      } catch (createFailure) {
        let existing: ProjectStatus;
        try {
          existing = await openProject(dir);
        } catch {
          // Not a project directory at all: the original error is the honest
          // one to show, because it explains why creating failed.
          throw createFailure;
        }
        const roots = existing.source_roots.map((root) => root.absolute_path);
        if (
          !samePath(existing.output_root, out) ||
          roots.length !== 1 ||
          !samePath(roots[0] ?? "", src)
        ) {
          throw {
            code: "conflict",
            message:
              `La carpeta de destino ya guarda un trabajo anterior sobre otra ` +
              `carpeta de origen (${roots.join(", ") || "sin origen"}). ` +
              `Elige otra carpeta de destino para no mezclar los dos.`,
          } satisfies ErrorDto;
        }
        return { status: existing, resumed: true };
      }
    },
    [],
  );

  /**
   * Stages 1-4: look at the files and work out what to copy.
   *
   * Stages the project already passed are skipped. A run over a real archive
   * takes minutes to hours, so being interrupted is ordinary; restarting from
   * scratch — or refusing outright, which is what this used to do — would
   * throw away work the engine took care to make resumable.
   */
  // While the scan runs, show the engine's real file count climbing so the user
  // can tell work is happening — never a fabricated percentage. Best-effort: a
  // failed poll (a momentary DB lock, or no Tauri runtime) simply skips a tick.
  const pollScanProgress = useCallback((dir: string): (() => void) => {
    let stopped = false;
    const tick = async (): Promise<void> => {
      try {
        const { files } = await scanProgress(dir);
        if (!stopped && files > 0) {
          setStage({
            kind: "working",
            label: "Mirando qué archivos tienes…",
            detail: `Solo los leemos. Nada se modifica. · ${files.toLocaleString(
              "es-ES",
            )} archivos vistos`,
          });
        }
      } catch {
        // Skip this tick.
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 400);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, []);

  const startReview = useCallback(
    async (
      plan: Resume & { kind: "review" },
      resumed: boolean,
      out: string,
    ) => {
      const dir = projectDir.current;
      let scanErrors: number | null = null;
      let duplicateSets: number | null = null;
      // Set by both branches below: the plan is either generated here or read
      // back from disk, never assumed.
      let operations: number;

      if (plan.scan) {
        setStage({
          kind: "working",
          label: "Mirando qué archivos tienes…",
          detail: "Solo los leemos. Nada se modifica.",
        });
        const stopPoll = pollScanProgress(dir);
        try {
          scanErrors = (await scanProject(dir)).errors;
        } finally {
          stopPoll();
        }
      }

      if (plan.hash) {
        setStage({
          kind: "working",
          label: "Identificando cada archivo…",
          detail:
            "Calculamos una huella única de cada uno. Con muchos archivos " +
            "esto puede tardar; si cierras la ventana, al volver se continúa " +
            "donde lo dejaste.",
        });
        await hashProject(dir);
      }

      if (plan.analyze) {
        setStage({
          kind: "working",
          label: "Buscando duplicados…",
          detail: "Comparamos las huellas para encontrar copias exactas.",
        });
        duplicateSets = (await analyzeProject(dir)).duplicate_sets;
      }

      if (plan.plan) {
        setStage({
          kind: "working",
          label: "Preparando la propuesta…",
          detail: "Decidimos qué copiar y a dónde.",
        });
        operations = (await createPlan(dir)).operations;
      } else {
        // The plan was written in an earlier session. Re-check its invariants
        // rather than trusting that it was valid once, and take the count from
        // the plan on disk instead of inventing one.
        setStage({
          kind: "working",
          label: "Revisando la propuesta anterior…",
          detail: "Comprobamos que sigue siendo válida antes de proponértela.",
        });
        const report = await validatePlan(dir);
        if (!report.ok) {
          throw {
            code: "validation",
            message:
              `La propuesta guardada ya no es válida: ` +
              `${report.problems.join("; ")}. Empieza de nuevo con una ` +
              `carpeta de destino vacía.`,
          } satisfies ErrorDto;
        }
        operations = report.operations;
      }

      // Counts we did not obtain above come from the project's own record,
      // never from a placeholder.
      const status = await projectStatus(dir);
      setStage({
        kind: "review",
        findings: {
          files: status.inventory?.files ?? 0,
          folders: status.inventory?.folders ?? 0,
          scanErrors: scanErrors ?? status.inventory?.scan_errors ?? 0,
          duplicateSets,
          operations,
          outputRoot: out,
          resumed,
        },
      });
    },
    [],
  );

  /**
   * Stages 5-7: freeze the plan, copy, and check the copy independently.
   *
   * `degraded` carries the user's explicit acknowledgement that the
   * destination cannot offer physical identity (ADR-0036). It is a parameter
   * rather than state because the engine refuses without it, and a refusal
   * discovered after an hour of copying is a worse way to learn this than a
   * question asked beforehand.
   */
  const startCopy = useCallback(
    async (plan: Resume & { kind: "copy" }, out: string, degraded = false) => {
      setError(null);
      const dir = projectDir.current;
      try {
        // Ask before committing, not after. A USB stick formatted exFAT or a
        // folder on the office NAS is an ordinary choice, and finding out it
        // is refused only at the end would waste the whole run.
        if (plan.execute && !degraded) {
          const guarantees = await destinationGuarantees(dir);
          if (!guarantees.has_physical_identity) {
            setStage({
              kind: "degraded",
              filesystem: guarantees.filesystem,
              resume: plan,
              outputRoot: out,
            });
            return;
          }
        }

        setStage({
          kind: "working",
          label: "Copiando tus archivos…",
          detail:
            "Cada archivo se copia y se comprueba al vuelo. Puedes cerrar y " +
            "continuar más tarde: no se pierde el trabajo hecho.",
        });
        if (plan.approve) {
          await approvePlan(dir);
        }
        let copied: number | null = null;
        let bytes: number | null = null;
        if (plan.execute) {
          const executed = degraded
            ? await executePlanOnDegradedDestination(dir)
            : await executePlan(dir);
          copied = executed.completed;
          bytes = executed.bytes_copied;
          if (executed.out_of_space) {
            // Verifying now would fail with an engine message about the
            // project's state, which explains nothing. The one useful fact is
            // that the destination drive is full and the work is not lost.
            setStage({
              kind: "outOfSpace",
              copied: executed.completed,
              bytes: executed.bytes_copied,
              pending: executed.pending,
              outputRoot: out,
              degraded,
            });
            return;
          }
        }

        setStage({
          kind: "working",
          label: "Comprobando la copia…",
          detail:
            "Volvemos a leer lo copiado y lo contrastamos con el original, " +
            "sin fiarnos de lo que acabamos de hacer.",
        });
        const verified = await verifyProject(dir);

        setStage({
          kind: "done",
          result: {
            copied,
            bytes,
            verdict: verified.verdict,
            problems: verified.problems,
            warnings: verified.warnings,
            outputRoot: out,
          },
        });
      } catch (failure) {
        fail(failure);
      }
    },
    [fail],
  );

  /** The single entry point: decide where this pair of folders stands. */
  const start = useCallback(async () => {
    setError(null);
    const src = source.trim();
    const out = destination.trim();
    if (src === "" || out === "") {
      setError({
        code: "validation",
        message:
          "Indica la carpeta que quieres ordenar y dónde guardar el resultado.",
      });
      return;
    }
    try {
      setStage({
        kind: "working",
        label: "Preparando…",
        detail: "Comprobamos si ya hay trabajo hecho sobre estas carpetas.",
      });
      // The project sits beside the destination so the user has one place to
      // look; the engine refuses overlapping roots and says so.
      const dir = `${out.replace(/[\\/]+$/, "")}-dataforge`;
      const { status, resumed } = await resolveProject(dir, src, out);
      projectDir.current = status.project_dir;

      const plan = resumeFrom(status.state);
      switch (plan.kind) {
        case "review":
          await startReview(plan, resumed, out);
          return;
        case "copy":
          setStage({ kind: "resumeCopy", resume: plan, outputRoot: out });
          return;
        case "finished":
          setStage({ kind: "finished", state: plan.state });
          return;
        case "manual":
          setStage({ kind: "manual", state: plan.state });
          return;
      }
    } catch (failure) {
      fail(failure);
    }
  }, [source, destination, fail, resolveProject, startReview]);

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
              void start();
            }}
          >
            <label
              ref={sourceBox}
              className={`dropfield${dropTarget === "source" ? " dropfield-active" : ""}`}
            >
              Carpeta que quieres ordenar
              <div className="input-action">
                <input
                  value={source}
                  onChange={(e) => setSource(e.target.value)}
                  onFocus={() => {
                    activeField.current = "source";
                  }}
                  placeholder="Arrastra la carpeta aquí, o escribe su ruta"
                  required
                />
                <button type="button" onClick={() => void browse("source")}>
                  Explorar…
                </button>
              </div>
              <span className="field-help">
                Puedes arrastrar la carpeta desde el explorador hasta esta
                ventana. Solo la leeremos.
              </span>
            </label>

            <label
              ref={destinationBox}
              className={`dropfield${dropTarget === "destination" ? " dropfield-active" : ""}`}
            >
              Dónde guardar el resultado
              <div className="input-action">
                <input
                  value={destination}
                  onChange={(e) => setDestination(e.target.value)}
                  onFocus={() => {
                    activeField.current = "destination";
                  }}
                  placeholder="Arrastra una carpeta vacía, o escribe su ruta"
                  required
                />
                <button type="button" onClick={() => void browse("destination")}>
                  Explorar…
                </button>
              </div>
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
          {stage.findings.resumed && (
            <p className="notice notice-info">
              Había trabajo a medias sobre estas carpetas y lo hemos continuado
              en lugar de empezar de cero.
            </p>
          )}
          <div className="tiles">
            <div className="tile">
              <span className="tile-label">Archivos</span>
              <strong>{humanCount(stage.findings.files)}</strong>
            </div>
            <div className="tile">
              <span className="tile-label">Carpetas</span>
              <strong>{humanCount(stage.findings.folders)}</strong>
            </div>
            {stage.findings.duplicateSets !== null && (
              <div className="tile">
                <span className="tile-label">Grupos de copias repetidas</span>
                <strong>{humanCount(stage.findings.duplicateSets)}</strong>
              </div>
            )}
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
              onClick={() =>
                void startCopy(
                  { kind: "copy", approve: true, execute: true },
                  stage.findings.outputRoot,
                )
              }
            >
              Hacer la copia
            </button>
            <button type="button" onClick={onExit}>
              Ahora no
            </button>
          </div>
        </>
      )}

      {stage.kind === "resumeCopy" && (
        <>
          <h2>Tienes una copia a medias</h2>
          <p>
            Ya habías aceptado esta copia en <code>{stage.outputRoot}</code> y
            quedó sin terminar. Podemos continuarla desde donde se quedó: lo ya
            copiado y comprobado no se repite, y nada de lo que hay se
            sobrescribe.
          </p>
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => void startCopy(stage.resume, stage.outputRoot)}
            >
              Continuar la copia
            </button>
            <button
              type="button"
              onClick={() => onOpenAdvanced(projectDir.current)}
            >
              Ver el detalle
            </button>
            <button type="button" onClick={onExit}>
              Ahora no
            </button>
          </div>
        </>
      )}

      {stage.kind === "degraded" && (
        <>
          <h2>Ese destino da menos garantías</h2>
          <p className="notice notice-warning">
            <code>{stage.outputRoot}</code> está en{" "}
            {stage.filesystem === "NETWORK"
              ? "una carpeta de red"
              : `un disco con formato ${stage.filesystem}`}
            . Ahí el sistema no nos deja identificar cada archivo de forma
            inequívoca, así que no podemos detectar con la misma seguridad que
            algo se sustituya por debajo mientras copiamos.
          </p>
          <p>
            Lo que <strong>sí</strong> seguimos garantizando: cada archivo se
            copia y se comprueba por su contenido, no se sobrescribe nada que ya
            exista, y tus originales no se tocan. Lo que se debilita es la
            detección de sustituciones durante la copia.
          </p>
          <p className="hint">
            Lo más seguro es copiar primero a un disco local (NTFS) y mover el
            resultado después. Si prefieres seguir, quedará anotado en el
            registro del proyecto que lo aceptaste.
          </p>
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => setStage({ kind: "setup" })}
            >
              Elegir otro destino
            </button>
            <button
              type="button"
              onClick={() =>
                void startCopy(stage.resume, stage.outputRoot, true)
              }
            >
              Entiendo el riesgo, copiar igualmente
            </button>
          </div>
        </>
      )}

      {stage.kind === "outOfSpace" && (
        <>
          <h2>No queda espacio en el destino</h2>
          <p className="notice notice-warning">
            El disco donde está <code>{stage.outputRoot}</code> se ha llenado,
            así que la copia se ha detenido ahí mismo en lugar de seguir
            intentándolo archivo por archivo. Nada de lo ya copiado se pierde, y{" "}
            <strong>tus archivos originales siguen intactos</strong>.
          </p>
          <div className="tiles">
            <div className="tile">
              <span className="tile-label">Copiado hasta ahora</span>
              <strong>{humanCount(stage.copied)}</strong>
            </div>
            <div className="tile">
              <span className="tile-label">Tamaño</span>
              <strong>{humanBytes(stage.bytes)}</strong>
            </div>
            <div className="tile">
              <span className="tile-label">Queda por copiar</span>
              <strong>{humanCount(stage.pending)}</strong>
            </div>
          </div>
          <p>
            Libera espacio en ese disco (o vacía la papelera) y continúa: se
            retomará justo donde se quedó, sin repetir lo hecho.
          </p>
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() =>
                void startCopy(
                  { kind: "copy", approve: false, execute: true },
                  stage.outputRoot,
                  stage.degraded,
                )
              }
            >
              Ya he liberado espacio, continuar
            </button>
            <button
              type="button"
              onClick={() => onOpenAdvanced(projectDir.current)}
            >
              Ver el detalle
            </button>
            <button type="button" onClick={onExit}>
              Ahora no
            </button>
          </div>
        </>
      )}

      {stage.kind === "finished" && (
        <>
          <h2>Esta carpeta ya está hecha</h2>
          {stage.state === "FAILED" ? (
            <p className="notice notice-danger">
              El destino que has indicado guarda un trabajo anterior cuya
              comprobación encontró problemas. Tus originales están intactos:
              abre el detalle para ver qué pasó antes de repetirlo.
            </p>
          ) : (
            <p className="notice notice-ok">
              El destino que has indicado ya contiene una copia terminada y
              comprobada
              {stage.state === "COMPLETED_WITH_WARNINGS" ? ", con avisos" : ""}.
              No hay nada que volver a copiar. Si quieres empezar de nuevo,
              elige otra carpeta de destino: no tocaremos esta.
            </p>
          )}
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => onOpenAdvanced(projectDir.current)}
            >
              Ver el detalle
            </button>
            <button type="button" onClick={() => setStage({ kind: "setup" })}>
              Elegir otras carpetas
            </button>
          </div>
        </>
      )}

      {stage.kind === "manual" && (
        <>
          <h2>Hay un trabajo interrumpido</h2>
          <p className="notice notice-warning">
            Sobre estas carpetas quedó un trabajo parado en un punto (
            <code>{stage.state}</code>) que no puedo continuar por mi cuenta sin
            arriesgarme a hacer algo que no has pedido. Tus archivos originales
            están intactos.
          </p>
          <p className="hint">
            Abre el detalle para ver el estado real, o empieza de nuevo con una
            carpeta de destino vacía.
          </p>
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => onOpenAdvanced(projectDir.current)}
            >
              Ver el detalle
            </button>
            <button type="button" onClick={() => setStage({ kind: "setup" })}>
              Elegir otras carpetas
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
          {/* Only shown when this run did the copying. A resumed project
              whose copy finished earlier has no honest number to put here. */}
          {stage.result.copied !== null && stage.result.bytes !== null && (
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
          )}

          {stage.result.verdict === "COMPLETED" && (
            <p className="notice notice-ok">
              Cada archivo copiado se ha vuelto a leer y coincide exactamente
              con el original. Tus archivos originales están intactos.
            </p>
          )}
          {stage.result.verdict === "COMPLETED_WITH_WARNINGS" && (
            <p className="notice notice-warning">
              La copia se completó, pero hay {humanCount(stage.result.warnings)}{" "}
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
