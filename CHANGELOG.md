# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/es/1.1.0/).
Versionado: [SemVer](https://semver.org/lang/es/).

## [Unreleased]

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
