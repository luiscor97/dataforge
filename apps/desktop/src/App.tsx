import { useCallback, useEffect, useState } from "react";

import { createProject, engineVersion, openProject, projectStatus } from "./api";
import { StatusView } from "./screens/StatusView";
import { type ErrorDto, type ProjectStatus, isErrorDto } from "./types";

type Screen = "home" | "create" | "open" | "status";

interface CreateFormState {
  name: string;
  projectDir: string;
  outputRoot: string;
  sources: string;
}

const EMPTY_FORM: CreateFormState = {
  name: "",
  projectDir: "",
  outputRoot: "",
  sources: "",
};

export default function App(): React.JSX.Element {
  const [screen, setScreen] = useState<Screen>("home");
  const [form, setForm] = useState<CreateFormState>(EMPTY_FORM);
  const [openPath, setOpenPath] = useState<string>("");
  const [status, setStatus] = useState<ProjectStatus | null>(null);
  const [error, setError] = useState<ErrorDto | null>(null);
  const [busy, setBusy] = useState<boolean>(false);
  const [version, setVersion] = useState<string>("");

  useEffect(() => {
    engineVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
  }, []);

  const handleFailure = useCallback((failure: unknown) => {
    setError(isErrorDto(failure) ? failure : { code: "unknown", message: String(failure) });
  }, []);

  const submitCreate = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const created = await createProject({
        name: form.name,
        project_dir: form.projectDir,
        output_root: form.outputRoot,
        source_roots: form.sources
          .split("\n")
          .map((line) => line.trim())
          .filter((line) => line.length > 0),
      });
      setStatus(created);
      setScreen("status");
    } catch (failure) {
      handleFailure(failure);
    } finally {
      setBusy(false);
    }
  }, [form, handleFailure]);

  const submitOpen = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const opened = await openProject(openPath.trim());
      setStatus(opened);
      setScreen("status");
    } catch (failure) {
      handleFailure(failure);
    } finally {
      setBusy(false);
    }
  }, [openPath, handleFailure]);

  const refreshStatus = useCallback(async () => {
    if (status === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      // Full status includes the integrity pass.
      setStatus(await projectStatus(status.project_dir));
    } catch (failure) {
      handleFailure(failure);
    } finally {
      setBusy(false);
    }
  }, [status, handleFailure]);

  const goHome = useCallback(() => {
    setError(null);
    setScreen("home");
  }, []);

  return (
    <main className="shell">
      <header className="topbar">
        <h1 onClick={goHome}>DataForge</h1>
        <span className="version">{version !== "" ? `engine ${version}` : ""}</span>
      </header>

      {error !== null && (
        <div className="error" role="alert">
          <strong>{error.code}</strong>: {error.message}
        </div>
      )}

      {screen === "home" && (
        <section className="panel">
          <h2>Proyectos</h2>
          <p className="hint">
            Un proyecto analiza orígenes sin modificarlos y guarda todo su estado en SQLite.
          </p>
          <div className="actions">
            <button type="button" onClick={() => setScreen("create")}>
              Crear proyecto
            </button>
            <button type="button" onClick={() => setScreen("open")}>
              Abrir proyecto
            </button>
          </div>
        </section>
      )}

      {screen === "create" && (
        <section className="panel">
          <h2>Crear proyecto</h2>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void submitCreate();
            }}
          >
            <label>
              Nombre
              <input
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                placeholder="Migración archivo histórico"
                required
              />
            </label>
            <label>
              Carpeta del proyecto (nueva o vacía)
              <input
                value={form.projectDir}
                onChange={(e) => setForm({ ...form, projectDir: e.target.value })}
                placeholder="D:\proyectos\migracion-2026"
                required
              />
            </label>
            <label>
              Carpeta de salida (para fases futuras; no puede solaparse)
              <input
                value={form.outputRoot}
                onChange={(e) => setForm({ ...form, outputRoot: e.target.value })}
                placeholder="D:\salidas\migracion-2026"
                required
              />
            </label>
            <label>
              Orígenes (uno por línea, solo lectura; opcional)
              <textarea
                value={form.sources}
                onChange={(e) => setForm({ ...form, sources: e.target.value })}
                placeholder={"D:\\datos\\servidor-antiguo"}
                rows={3}
              />
            </label>
            <div className="actions">
              <button type="submit" disabled={busy}>
                {busy ? "Creando…" : "Crear"}
              </button>
              <button type="button" onClick={goHome} disabled={busy}>
                Cancelar
              </button>
            </div>
          </form>
        </section>
      )}

      {screen === "open" && (
        <section className="panel">
          <h2>Abrir proyecto</h2>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void submitOpen();
            }}
          >
            <label>
              Carpeta del proyecto
              <input
                value={openPath}
                onChange={(e) => setOpenPath(e.target.value)}
                placeholder="D:\proyectos\migracion-2026"
                required
              />
            </label>
            <div className="actions">
              <button type="submit" disabled={busy}>
                {busy ? "Abriendo…" : "Abrir"}
              </button>
              <button type="button" onClick={goHome} disabled={busy}>
                Cancelar
              </button>
            </div>
          </form>
        </section>
      )}

      {screen === "status" && status !== null && (
        <StatusView status={status} busy={busy} onRefresh={() => void refreshStatus()} onBack={goHome} />
      )}
    </main>
  );
}
