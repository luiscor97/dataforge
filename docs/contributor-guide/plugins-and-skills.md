# Plugins y skills

## Dos conceptos distintos (RFC-0001 §0.1.8)

- **Plugins de desarrollo**: herramientas para construir DataForge
  (linters, auditoría, CLIs). Se gestionan con
  `scripts/install-dev-plugins.ps1` y se documentan en ADR-0013.
- **Plugins de DataForge**: extensiones del producto (extractores,
  clasificadores, rule packs…). Prohibidos hasta **Milestone 0.6**
  (Wasmtime/WASI); hoy no existe ninguno ni stubs.

## Plugins de desarrollo vigentes

| Herramienta | Para qué | Cómo se instala |
| --- | --- | --- |
| rustfmt / clippy | formato y lint | componentes rustup |
| cargo-audit | vulnerabilidades RustSec | `scripts/install-dev-plugins.ps1` |
| cargo-deny | licencias/fuentes/duplicados | `scripts/install-dev-plugins.ps1` |
| @tauri-apps/cli, vite, typescript | escritorio | `pnpm install` (lockfile) |
| sqlite3 CLI | inspección manual de bases | `scripts/install-dev-plugins.ps1` |

Criterios para añadir uno nuevo: RFC-0001 §0.1.3 (necesidad concreta del
hito, fuente confiable, sin ejecución remota opaca, licencia compatible,
documentado). Añadirlo al script + a esta tabla + ADR si no es trivial.

## Skills del repositorio (`.codex/skills/`)

Procedimientos acotados para agentes de codificación (política: ADR-0014).

| Skill | Uso |
| --- | --- |
| `bootstrap-environment` | preparar/verificar el entorno de desarrollo |
| `rust-quality-gate` | puerta de calidad: fmt, clippy, tests, builds |
| `sqlite-migrations` | crear migraciones nuevas sin romper checksums |
| `dataforge-invariants` | checklist de invariantes previo a merge |

Cada skill declara: nombre, objetivo, cuándo usarla, entradas, salidas,
herramientas permitidas, límites, comandos, criterios de éxito y fallos
esperados. Ninguna skill puede: modificar orígenes de proyectos DataForge,
saltarse tests, ocultar errores, ejecutar acciones destructivas ni
sustituir decisiones de seguridad.
