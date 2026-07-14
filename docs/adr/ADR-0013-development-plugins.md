# ADR-0013 — Plugins y herramientas de desarrollo

**Estado:** Aceptada
**Fecha:** 2026-07-13
**Relacionada con:** RFC-0001 §0.1.2, §0.1.3, §0.1.8

## Contexto

RFC-0001 distingue *plugins de desarrollo* (para construir DataForge) de
*plugins de DataForge* (parte del producto, Milestone 0.6). Solo procede
instalar herramientas que resuelvan una necesidad del hito actual, de fuente
identificable y sin ejecución remota opaca.

## Decisión

Herramientas de desarrollo adoptadas en Milestone 0.0:

| Herramienta | Fuente | Uso |
| --- | --- | --- |
| `rustfmt`, `clippy` | componentes rustup | formato y lint obligatorios |
| `@tauri-apps/cli` | npm (devDependency del workspace) | dev/build del escritorio |
| `vite`, `typescript`, `@vitejs/plugin-react` | npm (devDependencies) | frontend |
| `cargo-audit`, `cargo-deny` | `cargo install` (crates.io) | auditoría de dependencias; se instalan con `scripts/install-dev-plugins.ps1` y corren en CI |

Decisiones explícitas:

- **No** se instalan servidores MCP: ningún hito actual los necesita
  (criterio 1 de §0.1.3). Se reevaluará cuando exista una necesidad concreta.
- **No** se instala GitHub CLI todavía: el repositorio es local y no hay
  remoto configurado; el instalador MSI machine-wide pediría elevación.
  Alternativa cuando haga falta: `winget install GitHub.cli` con elevación,
  o la release portable oficial.
- La CLI de Tauri se consume como paquete npm versionado en el lockfile en
  lugar de `cargo install tauri-cli` (misma funcionalidad, instalación
  reproducible y sin compilación de 10+ minutos).
- Los *plugins de DataForge* (extractores, clasificadores, rule packs…)
  quedan prohibidos hasta Milestone 0.6, salvo stubs de interfaz
  estrictamente necesarios (ninguno todavía).

## Consecuencias

- `scripts/install-dev-plugins.ps1` es el único punto de entrada para añadir
  herramientas de desarrollo; cada adición futura se documenta ahí y, si no
  es trivial, en una ADR.
