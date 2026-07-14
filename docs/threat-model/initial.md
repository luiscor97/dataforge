# Modelo de amenazas inicial

Ámbito: Milestone 0.0 (fundación). Se ampliará con cada hito; la lista
completa de amenazas del producto está en RFC-0001 §37.

## Activos

1. Los archivos de origen del usuario (valor probatorio/histórico).
2. El estado del proyecto (`state/dataforge.sqlite`).
3. El ledger de auditoría (evidencia de qué hizo el motor y cuándo).
4. La cadena de suministro del propio repositorio.

## Amenazas y mitigaciones vigentes

| Amenaza | Mitigación implementada |
| --- | --- |
| El motor escribe/borra en el origen | Sin rutas de código de borrado o sobrescritura; `read_only_policy` forzado por constructor y por `CHECK` SQL; validación de solapamiento de rutas en la fachada; test que verifica que crear un proyecto no escribe en el origen |
| Salida/proyecto anidados que se autoalimentan | `ensure_disjoint` rechaza proyecto⊂salida, salida⊂proyecto, origen⊂{proyecto, salida, auditoría} y viceversa |
| Manipulación del ledger | Cadena SHA-256 con envelope canónico (tipo, actor, secuencia, timestamp, payload); triggers SQL append-only; verificación en `project status`; tests de manipulación (payload, metadatos, borrado de eslabón, recomputación) |
| Deriva silenciosa de esquema | Checksums SHA-256 de migraciones verificados en cada apertura |
| Corrupción SQLite | `PRAGMA integrity_check` + `foreign_key_check` en la pasada de integridad |
| Path traversal en rutas de proyecto | Rutas absolutizadas; componentes `..` rechazados en la comparación de contención |
| UI con lógica privilegiada | La UI solo llama comandos de `df-facade`; capacidades Tauri mínimas (`core:default`); CSP restrictiva |
| Dependencias comprometidas | `deny.toml` (fuentes solo crates.io, licencias permitidas, wildcard prohibido); `cargo audit`/`cargo deny` en CI; lockfiles versionados |
| Ejecución remota opaca en bootstrap | Scripts usan winget/npm/rustup oficiales; prohibido `irm | iex` (RFC §0.1.5) |

## Amenazas aceptadas / pendientes (con hito responsable)

- Zip bombs, reparse loops, placeholders cloud, nombres ilegales: fase de
  validación y escaneo (0.1) y contenedores (0.4).
- Verificación criptográfica de copias: ejecutor (0.1).
- Sandboxing de plugins (Wasmtime/WASI): 0.6.
- Redacción de logs con datos sensibles: cuando exista logging persistente.
- Firma de releases y SBOM: 0.9.
- El marker `project.dataforge.json` no está firmado; una edición manual se
  detecta solo si el id no coincide con la base. Aceptado: el marker no es
  fuente de verdad.
