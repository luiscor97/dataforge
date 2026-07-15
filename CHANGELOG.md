# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/es/1.1.0/).
Versionado: [SemVer](https://semver.org/lang/es/).

## [Unreleased]

### Hardening de seguridad del sistema de archivos (v0.1.1-dev)

Endurece el núcleo para poder probarlo sobre colecciones reales supervisadas.
No añade funcionalidad de producto. Modelo de amenazas completo en
[`docs/threat-model/filesystem-hardening.md`](docs/threat-model/filesystem-hardening.md).

- **Frontera segura del sistema de archivos** (crate nuevo `df-fs-safety`,
  ADR-0017): toda escritura pasa por él. El output root se valida y se
  identifica **físicamente** (volume serial + file id) antes de escribir y se
  revalida durante la ejecución; los destinos se resuelven **componente a
  componente** rechazando cualquiera que sea reparse point (symlink, junction o
  mount point). Sustituye `create_dir_all` y `File::create` por equivalentes
  que comprueban cada nivel. Motivo: validar que una ruta es relativa y sin
  `..` es *texto*, y el texto no dice nada del disco — una junction preexistente
  dentro de la salida redirigía la escritura fuera de ella.
- **Finalize sin reemplazo real** (ADR-0021): `MoveFileExW` **sin**
  `MOVEFILE_REPLACE_EXISTING`. Corrige una ventana real de sobrescritura
  silenciosa: en Windows `std::fs::rename` **sí sobrescribe**, y el
  `destination.exists()` previo era una comprobación TOCTOU — el código
  afirmaba una garantía que en esta plataforma no tenía (regla 3).
- **El verificador nunca sigue enlaces** (§28.2): recorrido con
  `symlink_metadata`, reparse points reportados y jamás traspasados, ciclos
  cortados por identidad física, y errores de lectura convertidos en hallazgos
  (`OUTPUT_REPARSE_POINT`, `OUTPUT_SUBTREE_UNREADABLE`) en vez de un `continue`
  silencioso. Antes podía leer **fuera** del output root y aun así certificarlo.
- **Manifiesto de ejecución inmutable** (migración `0004_execution_manifest`,
  ADR-0018): la aprobación congela el contrato completo —qué se lee, qué
  contenido se espera, dónde se escribe y qué operación corre— y el SHA-256 lo
  cubre entero. El executor ejecuta **solo** el manifiesto; las tablas de
  inventario vuelven a ser evidencia. Inmutabilidad impuesta por triggers.
  Antes, editar `content_objects.sha256` tras aprobar cambiaba lo ejecutado
  **sin mover el hash del plan** (la regla 10 era medio verdad).
- **Fingerprint físico v2** (ADR-0019): enum versionado `V1`/`V2`; v2 añade
  identidad física, `ChangeTime` de NTFS y atributos. Detecta la sustitución de
  un archivo por otro **del mismo tamaño y mismo mtime**, que v1 no veía. La
  comparación es un veredicto explícito (`SamePhysical`/`SameDegraded`/
  `Changed`), no `PartialEq`: identidad degradada **no** es "sin cambios", y v1
  y v2 nunca se declaran equivalentes. Los tokens v1 existentes siguen
  leyéndose.
- **Rutas raw reversibles** (migración `0005_path_identity`, ADR-0020): se
  conservan las unidades UTF-16 exactas (BLOB LE; hex en el JSON del
  manifiesto). Display, comparación y raw son tres cosas distintas y solo la
  raw abre archivos. Antes, un nombre con un surrogate suelto —legal en
  Windows— podía quedar inabrible o, peor, abrir **otro** archivo.
- **Creación atómica de proyectos y marker endurecido** (ADR-0022): el proyecto
  se construye en `<dir>.init-<uuid>` y se finaliza con un rename atómico; el
  marker se escribe el último y solo tras el integrity check. Un fallo no deja
  medio proyecto y el reintento funciona; **nunca** se limpia una carpeta
  preexistente del usuario. El marker deja de ser autoritativo para la ruta de
  la base (en Windows `join` con ruta absoluta descartaba la base y permitía
  redirigir SQLite fuera del proyecto), y `schema_version` gobierna la apertura
  con política explícita para versión futura, antigua o manipulada.
- CI: jobs Windows específicos de hardening, tests de manipulación y
  compatibilidad de migraciones.
- `cargo deny` vuelve a estar verde: llevaba roto desde M0.0 sin detectarse
  porque la CI nunca había llegado a ejecutarse. Los wildcards se eliminan
  dando versión explícita a las dependencias internas (sin excepción de
  configuración); los cinco advisories `unmaintained` de `unic-*` —que llegan
  transitivamente desde Tauri— se ignoran **uno a uno**, documentados y con
  condición de retirada y fecha de revisión.

### Limitaciones de este incremento

- **Windows es la única plataforma con seguridad implementada.** En el resto,
  la ejecución se **bloquea** en lugar de fingir garantías (regla 19).
- NAS/UNC sigue **experimental**: sin `file_id` la identidad es *degradada* y no
  se puede descartar sustitución.
- La garantía frente a quien pueda editar la base es de **detección**, no de
  prevención.
- Sin durabilidad garantizada ante fallo físico del hardware.
- Queda ventana TOCTOU residual entre validación y escritura: se reduce a
  "falla, no pisa", no se elimina.

## [Anterior] — Milestone 0.1 "Safe Inventory Core"

### Añadido

- Migración `0002_inventory`: tablas `scan_runs`, `folders`,
  `path_occurrences`, `content_objects`, `occurrence_content` y `hash_jobs`
  (RFC-0001 §10.1), STRICT y con claves foráneas.
- `df-scan`: validación de orígenes (§12.1) y escáner seguro (§13) — cola
  iterativa, reparse points registrados y nunca seguidos, rutas largas
  Windows (`\\?\`), nombres no-Unicode marcados, errores parciales
  persistidos, batches transaccionales, cancelación segura.
- `df-hash`: fingerprint físico v1, BLAKE3 + SHA-256 en una sola pasada de
  lectura, invalidación pre/post (`SOURCE_CHANGED`, §14.5) y cola de
  trabajos reanudable (`hash_jobs`).
- Duplicados exactos (mismo tamaño + SHA-256, §15) como informe de
  evidencia, sin proponer acciones.
- Eventos de auditoría del pipeline: `SCAN_STARTED/COMPLETED/CANCELLED/
  FAILED`, `HASH_STARTED/COMPLETED/PAUSED`.
- `df-facade`: `scan_project`, `hash_project`, `duplicate_report`,
  `verify_audit`; `ProjectStatus` incluye snapshot e inventario.
- CLI: `dataforge scan`, `dataforge hash`, `dataforge report duplicates`,
  `dataforge audit verify` (con `--json` y códigos de salida §33, incluido
  `3 partial completion`).
- Desktop: la vista de estado muestra snapshot, inventario y progreso de
  hashing reales.
- ADR-0015 con las decisiones del incremento.
- Migración `0003_planning`: `duplicate_sets`, `plans`, `plan_operations`
  (congeladas por trigger al aprobar), `operation_results` (append-only),
  `verification_runs` y `verification_findings`.
- `df-planner`: análisis (materializa duplicate_sets, §15), generación de
  plan con cobertura completa (§26.2) bajo política `REPORT_ONLY` —
  `COPY_ACTIVE`, `CREATE_DIRECTORY`, `NO_ACTION`, `BLOCKED`,
  `COPY_WITH_SUFFIX` para colisiones —, validación §26.5 y aprobación con
  serialización canónica + SHA-256 (§26.4).
- `df-executor`: protocolo por archivo del §27.1 (fingerprint pre/post,
  parcial `.n.dataforge-partial-<op>`, copia en streaming con doble hash,
  flush, comparación, rename atómico), colisiones §27.3, errores tipados
  §27.5, reanudación §27.4 y cancelación segura.
- `df-verifier`: verificación independiente §28 — re-hash de cada destino,
  cobertura de ejecución, plan no manipulado (re-serialización canónica),
  parciales huérfanos, archivos no registrados y origen sin cambios;
  veredicto `COMPLETED` / `COMPLETED_WITH_WARNINGS` / `FAILED`.
- Eventos: `ANALYSIS_COMPLETED`, `PLAN_CREATED`, `PLAN_APPROVED`,
  `EXECUTION_COMPLETED/PAUSED`, `VERIFICATION_COMPLETED`.
- CLI: `dataforge analyze`, `plan create/validate/approve`, `execute`,
  `verify` — el pipeline completo del RFC §33 para 0.1.
- ADR-0016 con las decisiones del incremento de plan/ejecución/verificación.

### Seguridad

- El escáner y el hasher abren el origen exclusivamente en lectura; los
  tests verifican que el origen no cambia tras el pipeline completo.
- El executor nunca sobrescribe (rename que falla si el destino existe,
  `SKIP_REPRESENTED`/sufijo determinista en colisiones) y el único borrado
  del código son sus propios archivos parciales fallidos (ADR-0016).
- Un plan aprobado es inmutable por trigger SQL y su manipulación offline
  se detecta criptográficamente en la verificación (`PLAN_TAMPERED`).

## [0.0.1-dev] — 2026-07-13 — Milestone 0.0 "Repository Foundation"

### Añadido

- Monorepo: workspace Cargo (7 crates) + workspace pnpm.
- `df-error`: errores tipados y códigos de salida (RFC-0001 §33).
- `df-domain`: IDs tipados (UUIDv4), `Project`, `ProfileRef`, `SourceRoot`
  (solo lectura por construcción), `Snapshot`, `AuditEvent`, `Actor` y la
  máquina de estados completa de RFC-0001 §11 con sus invariantes.
- `df-ledger`: JSON canónico, timestamps canónicos, construcción y
  verificación de cadenas de eventos SHA-256 (genesis, secuencia contigua,
  envelope que cubre metadatos).
- `df-db`: SQLite (rusqlite bundled), migración `0001_foundation` (tablas
  STRICT `projects`, `source_roots`, `snapshots`, `audit_events` +
  `schema_migrations`), migraciones con checksum verificado en apertura,
  triggers append-only sobre `audit_events`, repositorios transaccionales
  (crear proyecto, transición de estado, eventos) y pasada de integridad.
- `df-facade`: `create_project`, `open_project`, `project_status`;
  validación de rutas disjuntas; marker `project.dataforge.json` versionado.
- CLI `dataforge`: `project create`, `project status`, `--json`, códigos de
  salida 0/1/2/4/5.
- Desktop `DataForge Desktop` (Tauri 2 + React 19 + TS strict): pantallas
  de inicio, crear proyecto, abrir proyecto y estado con integridad; sin
  lógica crítica en la UI.
- Documentación: RFC-0001 en `docs/rfcs/`, ADR-0001..0003 y ADR-0011..0014,
  system overview, threat model inicial, guías de contribución y entorno.
- Bootstrap reproducible: `scripts/*.ps1` idempotentes + informe de entorno.
- Skills del repositorio en `.codex/skills/`.
- CI (GitHub Actions, Windows): fmt, clippy `-D warnings`, tests, build CLI,
  typecheck/build frontend, `cargo audit` + `cargo deny`.
- Gobernanza: licencias MIT/Apache-2.0, README, CONTRIBUTING (DCO),
  SECURITY, GOVERNANCE, Código de Conducta, plantillas de issues y PR.

### Seguridad

- Sin rutas de código de borrado ni sobrescritura; orígenes de solo lectura
  por política, reforzado con `CHECK` en SQL.
- Ledger append-only con verificación criptográfica y tests de manipulación.
