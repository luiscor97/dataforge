# ADR-0002 — SQLite como única fuente de verdad transaccional

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 regla 5, §10, §42.3

## Contexto

El estado de un proyecto (entidades, transiciones, planes, resultados,
auditoría) debe sobrevivir a interrupciones, ser transaccional y poder
inspeccionarse con herramientas estándar. Los informes CSV/JSON/Markdown/PDF
son derivados, nunca estado.

## Decisión

Cada proyecto guarda todo su estado en una base SQLite
(`state/dataforge.sqlite` dentro de la carpeta del proyecto). Reglas:

- `PRAGMA foreign_keys = ON` en cada conexión;
- migraciones versionadas y con checksum SHA-256 (`schema_migrations`),
  verificadas en cada apertura para detectar deriva de esquema;
- tablas `STRICT`; cada tabla lleva `created_at`;
- toda mutación corre en una transacción que incluye su evento de auditoría,
  de modo que estado y ledger no pueden divergir;
- `audit_events` es append-only, reforzado con triggers que abortan
  `UPDATE`/`DELETE`;
- no se almacenan binarios ni texto masivo en tablas transaccionales;
- el marcador `project.dataforge.json` solo identifica la carpeta y apunta a
  la base de datos: no es fuente de verdad.

Solo `df-db` emite SQL. WAL se evaluará con benchmarks antes de activarse
(RFC-0001 §10.3); mientras tanto se usa el journal por defecto.

## Alternativas consideradas

- **Archivos JSON por entidad**: sin transacciones ni integridad referencial.
- **Base embebida alternativa (sled, redb)**: menos tooling de inspección,
  formatos menos documentados (contra §5.7).
- **Servidor externo (PostgreSQL)**: rompe local-first (§5.1).

## Consecuencias

- Una base por proyecto simplifica copia/backup y aislamiento, y la fachada
  impone la invariante "un proyecto por base".
- Las tablas de RFC-0001 §10.1 restantes se crearán en migraciones nuevas en
  el hito que implemente su funcionalidad; la migración 0001 solo contiene
  las entidades reales de este hito (sin esquema muerto).
