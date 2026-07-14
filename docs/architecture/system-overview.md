# DataForge — visión general del sistema

Estado del documento: refleja lo **implementado** en Milestone 0.0. La
arquitectura objetivo completa está en
[RFC-0001 §7](../rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

## Capas

```text
┌────────────────────────────┐   ┌──────────────────────────┐
│     DataForge Desktop      │   │      dataforge (CLI)     │
│ Tauri 2 + React + TS strict│   │       clap, --json       │
└─────────────┬──────────────┘   └────────────┬─────────────┘
              │  commands (IPC)               │  llamadas directas
┌─────────────▼───────────────────────────────▼─────────────┐
│                        df-facade                          │
│   create_project / open_project / project_status          │
│   validación de rutas, marker JSON, DTOs serializables    │
└─────────────┬─────────────────────────────────────────────┘
              │
┌─────────────▼─────────────┐      ┌───────────────────────┐
│          df-db            │──────│       df-ledger       │
│ SQLite, migraciones con   │ usa  │ eventos hash-chained  │
│ checksum, repositorios,   │      │ SHA-256, JSON canónico│
│ integridad                │      └───────────┬───────────┘
└─────────────┬─────────────┘                  │
┌─────────────▼─────────────────────────────── ▼────────────┐
│                       df-domain                           │
│ IDs tipados, Project, SourceRoot, Snapshot, AuditEvent,   │
│ máquina de estados (RFC §11)                              │
└─────────────┬─────────────────────────────────────────────┘
┌─────────────▼─────────────┐
│         df-error          │  errores tipados + exit codes │
└───────────────────────────┘
```

## Reglas de dependencia

- `df-domain` es puro: sin I/O, sin SQL.
- Solo `df-db` emite SQL; solo `df-facade` toca el sistema de archivos del
  proyecto (marker, directorios).
- Los clientes (CLI, desktop) dependen **exclusivamente** de `df-facade`
  (+ `df-domain` para el tipo `Actor` y `df-error` para códigos de salida).
- La interfaz no contiene lógica crítica: los comandos Tauri son adaptadores
  de una llamada a la fachada.

## Datos en disco (por proyecto)

```text
<proyecto>/
├── project.dataforge.json      # marcador versionado (no es fuente de verdad)
└── state/
    └── dataforge.sqlite        # única fuente de verdad transaccional
```

## Flujo implementado en 0.0

1. `project create` valida nombre y rutas (proyecto/salida/orígenes
   disjuntos, orígenes existentes y de solo lectura por política), crea la
   carpeta, aplica la migración 0001 y persiste `Project` + `SourceRoot`s +
   evento `PROJECT_CREATED` en una única transacción.
2. `project status` abre el marker, valida que el id del marker y el de la
   base coinciden, y ejecuta la pasada de integridad: `integrity_check`,
   `foreign_key_check`, checksums de migraciones y verificación
   criptográfica de la cadena de eventos.

Las fases de escaneo, hash, análisis, plan, ejecución y verificación están
definidas en la máquina de estados pero **no** implementadas: pertenecen a
Milestone 0.1+ y hoy no existe ninguna ruta de código que las simule.
