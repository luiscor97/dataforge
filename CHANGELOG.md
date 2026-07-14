# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/es/1.1.0/).
Versionado: [SemVer](https://semver.org/lang/es/).

## [Unreleased] — Milestone 0.1 "Safe Inventory Core" (en curso)

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
