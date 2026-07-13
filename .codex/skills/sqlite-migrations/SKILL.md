# Skill: sqlite-migrations

**Nombre:** sqlite-migrations
**Objetivo:** crear migraciones SQLite correctas, versionadas y verificables
para `df-db`.

## Cuándo usarla

- Cualquier cambio de esquema (tabla, columna, índice, trigger).

## Entradas

- La necesidad de esquema del hito actual (nunca esquema "por si acaso").

## Salidas

- `crates/df-db/migrations/NNNN_nombre.sql` nuevo.
- Entrada en `MIGRATIONS` (`crates/df-db/src/migrations.rs`).
- Tests que ejercitan el esquema nuevo.

## Herramientas permitidas

- Editor + cargo test. `sqlite3` CLI solo para inspección manual.

## Límites

- **Nunca editar una migración ya aplicada/commiteada**: el checksum SHA-256
  se verifica en cada apertura y una edición rompe todas las bases
  existentes. Los cambios van SIEMPRE en una migración nueva.
- Numeración consecutiva de 4 dígitos; sin huecos.
- Toda tabla nueva: `STRICT`, `created_at`, claves foráneas explícitas.
- Tablas de evidencia/auditoría: append-only con triggers.
- Prohibido almacenar binarios o texto masivo en tablas transaccionales.
- Prohibido `DROP TABLE`/`DELETE` de datos de usuario en migraciones del MVP.

## Comandos

```powershell
cargo test -p df-db
# inspección manual opcional:
sqlite3 <proyecto>/state/dataforge.sqlite ".schema"
```

## Criterios de éxito

- `cargo test -p df-db` verde, incluidos `migrations_are_idempotent` y la
  pasada de integridad.
- Abrir una base creada con la versión anterior aplica la migración nueva
  sin errores (probar con un test de reapertura).

## Fallos esperados

- "migration drifted": alguien editó una migración aplicada → revertir la
  edición y crear una migración nueva.
