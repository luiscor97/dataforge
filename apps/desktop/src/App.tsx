import { useCallback, useEffect, useState } from "react";

import {
  analyzeMedia,
  analyzeSimilarity,
  createProject,
  engineVersion,
  openProject,
  projectStatus,
} from "./api";
import { ErrorAlert } from "./components/ErrorAlert";
import { StatusView } from "./screens/StatusView";
import { type ErrorDto, type ProjectStatus, isErrorDto } from "./types";

type Screen = "home" | "create" | "open" | "status";
type BuiltInProfile = "generic" | "legal";

interface CreateFormState {
  name: string;
  projectDir: string;
  outputRoot: string;
  sources: string;
  profile: BuiltInProfile;
}

const EMPTY_FORM: CreateFormState = {
  name: "",
  projectDir: "",
  outputRoot: "",
  sources: "",
  profile: "generic",
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
    setError(
      isErrorDto(failure)
        ? failure
        : { code: "unknown", message: String(failure) },
    );
  }, []);

  const submitCreate = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const created = await createProject({
        name: form.name,
        project_dir: form.projectDir,
        output_root: form.outputRoot,
        profile: form.profile,
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

  const runSimilarity = useCallback(async () => {
    if (status === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await analyzeSimilarity(status.project_dir);
      setStatus(await projectStatus(status.project_dir));
    } catch (failure) {
      handleFailure(failure);
    } finally {
      setBusy(false);
    }
  }, [status, handleFailure]);

  const runMedia = useCallback(async () => {
    if (status === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await analyzeMedia(status.project_dir);
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
    <main className="shell" aria-busy={busy}>
      <header className="topbar">
        <h1>
          <button type="button" className="brand-button" onClick={goHome}>
            DataForge
          </button>
        </h1>
        <span className="version">
          {version !== "" ? `motor ${version}` : ""}
        </span>
      </header>

      {error !== null && <ErrorAlert error={error} />}

      {screen === "home" && (
        <section className="panel home">
          <h2>Pon orden en tus documentos</h2>
          <p className="lead">
            DataForge examina las carpetas que tú elijas y encuentra duplicados,
            versiones y estructura repetida, siempre con evidencia verificable.
            Nunca modifica ni borra tus archivos: solo los lee.
          </p>
          <h3 className="steps-title">Cómo funciona</h3>
          <ol className="steps">
            <li>
              <span className="step-number" aria-hidden="true">
                1
              </span>
              <h4>Crea un proyecto</h4>
              <p>
                Dale un nombre y elige las carpetas donde están tus documentos.
              </p>
            </li>
            <li>
              <span className="step-number" aria-hidden="true">
                2
              </span>
              <h4>Deja que analice</h4>
              <p>
                DataForge examina los archivos en modo solo lectura y guarda
                cada evidencia en el proyecto.
              </p>
            </li>
            <li>
              <span className="step-number" aria-hidden="true">
                3
              </span>
              <h4>Revisa la evidencia</h4>
              <p>
                Consulta duplicados, versiones y anomalías, y busca dentro del
                contenido de tus archivos.
              </p>
            </li>
          </ol>
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => setScreen("create")}
            >
              Crear proyecto
            </button>
            <button type="button" onClick={() => setScreen("open")}>
              Abrir proyecto existente
            </button>
          </div>
        </section>
      )}

      {screen === "create" && (
        <section className="panel">
          <h2>Crear proyecto</h2>
          <p className="screen-intro">
            Solo necesitas un nombre y dos carpetas de trabajo. Tus documentos
            originales no se tocan: DataForge únicamente los lee.
          </p>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void submitCreate();
            }}
          >
            <fieldset>
              <legend>Tu proyecto</legend>
              <label>
                Nombre del proyecto
                <input
                  value={form.name}
                  onChange={(e) => setForm({ ...form, name: e.target.value })}
                  placeholder="Ej.: Archivo histórico 2026"
                  required
                  aria-describedby="name-help"
                />
                <span id="name-help" className="field-help">
                  Sirve solo para que tú lo reconozcas; puedes usar el nombre
                  que quieras.
                </span>
              </label>
              <label>
                Perfil de análisis
                <select
                  value={form.profile}
                  onChange={(event) =>
                    setForm({
                      ...form,
                      profile: event.target.value as BuiltInProfile,
                    })
                  }
                  aria-describedby="profile-help"
                >
                  <option value="generic">Genérico</option>
                  <option value="legal">Jurídico</option>
                </select>
                <span id="profile-help" className="field-help">
                  El perfil jurídico protege expedientes, procedimientos y otras
                  agrupaciones legales para que el análisis no las separe. Si
                  tienes dudas, «Genérico» es la opción segura.
                </span>
              </label>
            </fieldset>
            <fieldset>
              <legend>Carpetas</legend>
              <label>
                Carpeta del proyecto
                <input
                  value={form.projectDir}
                  onChange={(e) =>
                    setForm({ ...form, projectDir: e.target.value })
                  }
                  placeholder="D:\proyectos\migracion-2026"
                  required
                  aria-describedby="project-dir-help"
                />
                <span id="project-dir-help" className="field-help">
                  Una carpeta nueva o vacía donde DataForge guardará su base de
                  datos y su registro de trabajo.
                </span>
              </label>
              <label>
                Carpeta de resultados
                <input
                  value={form.outputRoot}
                  onChange={(e) =>
                    setForm({ ...form, outputRoot: e.target.value })
                  }
                  placeholder="D:\salidas\migracion-2026"
                  required
                  aria-describedby="output-root-help"
                />
                <span id="output-root-help" className="field-help">
                  Aquí escribirá DataForge los resultados en fases futuras;
                  nunca toca tus originales. No puede estar dentro de las otras
                  carpetas.
                </span>
              </label>
              <label>
                Carpetas con tus documentos (opcional)
                <textarea
                  value={form.sources}
                  onChange={(e) =>
                    setForm({ ...form, sources: e.target.value })
                  }
                  placeholder={"D:\\datos\\servidor-antiguo"}
                  rows={3}
                  aria-describedby="sources-help"
                />
                <span id="sources-help" className="field-help">
                  Escribe una ruta por línea. DataForge solo las leerá, nunca
                  las modifica. Puedes añadirlas ahora o más adelante.
                </span>
              </label>
            </fieldset>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                {busy ? "Creando…" : "Crear proyecto"}
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
          <p className="screen-intro">
            Vuelve a un proyecto que ya creaste y continúa donde lo dejaste.
          </p>
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
                aria-describedby="open-path-help"
              />
              <span id="open-path-help" className="field-help">
                Es la carpeta que elegiste como «Carpeta del proyecto» al
                crearlo; dentro está la base de datos de DataForge.
              </span>
            </label>
            <div className="actions">
              <button type="submit" className="primary" disabled={busy}>
                {busy ? "Abriendo…" : "Abrir proyecto"}
              </button>
              <button type="button" onClick={goHome} disabled={busy}>
                Cancelar
              </button>
            </div>
          </form>
        </section>
      )}

      {screen === "status" && status !== null && (
        <StatusView
          status={status}
          busy={busy}
          onRefresh={() => void refreshStatus()}
          onAnalyzeSimilarity={() => void runSimilarity()}
          onAnalyzeMedia={() => void runMedia()}
          onBack={goHome}
        />
      )}
    </main>
  );
}
