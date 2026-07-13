# ADR-0014 — Skills del repositorio (`.codex/skills/`)

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 §0.1.7

## Contexto

RFC-0001 exige una capa de skills propia del repositorio para estandarizar
tareas repetitivas y controles de calidad ejecutados por agentes de
codificación (Codex, Claude Code u otros), con límites explícitos.

## Decisión

Las skills viven en `.codex/skills/<nombre>/SKILL.md` con un formato fijo
(nombre, objetivo, cuándo usarla, entradas, salidas, herramientas
permitidas, límites, comandos, criterios de éxito, fallos esperados).

Skills creadas en Milestone 0.0:

- `bootstrap-environment` — preparar/verificar el entorno con los scripts.
- `rust-quality-gate` — puerta de calidad: fmt, clippy, tests, build.
- `sqlite-migrations` — reglas para crear una migración nueva.
- `dataforge-invariants` — lista de invariantes que ningún cambio puede
  violar (origen inmutable, sin borrado, sin sobrescritura, ledger
  append-only, clientes solo vía `df-facade`).

Límites globales (aplican a toda skill):

- nunca modificar archivos dentro de un origen de un proyecto DataForge;
- nunca saltarse tests ni ocultar errores;
- nunca ejecutar acciones destructivas ni `curl | sh` u equivalentes;
- nunca declarar una tarea terminada sin evidencia de ejecución.

## Consecuencias

- Los agentes disponen de procedimientos verificables y acotados.
- Cada skill nueva se añade al índice de
  `docs/contributor-guide/plugins-and-skills.md`.
