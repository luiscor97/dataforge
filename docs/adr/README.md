# Architecture Decision Records

| ADR | Título | Estado |
| --- | ------ | ------ |
| [ADR-0001](ADR-0001-rust-core.md) | Rust como núcleo del motor | Aceptada |
| [ADR-0002](ADR-0002-sqlite-source-of-truth.md) | SQLite como única fuente de verdad transaccional | Aceptada |
| [ADR-0003](ADR-0003-origin-immutable.md) | El origen es inmutable | Aceptada |
| [ADR-0011](ADR-0011-windows-user-space-toolchain.md) | Toolchain Windows en espacio de usuario (GNU + WinLibs) | Aceptada |
| [ADR-0012](ADR-0012-node-and-pnpm-policy.md) | Política de Node.js y pnpm | Aceptada |
| [ADR-0013](ADR-0013-development-plugins.md) | Plugins y herramientas de desarrollo | Aceptada |
| [ADR-0014](ADR-0014-codex-skills-policy.md) | Skills del repositorio (`.codex/skills/`) | Aceptada |
| [ADR-0015](ADR-0015-inventory-increment-scan-hash.md) | Decisiones del incremento de inventario (M0.1): escaneo y hashing | Aceptada |
| [ADR-0016](ADR-0016-plan-execute-verify-increment.md) | Decisiones del incremento de planificación, ejecución y verificación (M0.1) | Aceptada |
| [ADR-0017](ADR-0017-secure-filesystem-boundary.md) | Frontera segura del sistema de archivos (`df-fs-safety`) (v0.1.1) | Aceptada |
| [ADR-0018](ADR-0018-immutable-execution-manifest.md) | Manifiesto de ejecución inmutable (v0.1.1) | Aceptada |
| [ADR-0019](ADR-0019-file-fingerprint-v2.md) | Fingerprint físico v2 (v0.1.1) | Aceptada |
| [ADR-0020](ADR-0020-raw-path-representation.md) | Representación raw de rutas (v0.1.1) | Aceptada |
| [ADR-0021](ADR-0021-platform-no-replace-finalization.md) | Finalize no-replace por plataforma y durabilidad (v0.1.1) | Aceptada |
| [ADR-0022](ADR-0022-atomic-project-initialization.md) | Creación atómica de proyectos y marker endurecido (v0.1.1) | Aceptada |
| [ADR-0023](ADR-0023-folder-merkle-signatures.md) | Firmas Merkle de carpeta y detección de clones exactos de árbol (M0.2) | Aceptada |
| [ADR-0024](ADR-0024-folder-context-classification.md) | Clasificación de contexto de carpetas por marcadores de perfil (M0.2) | Aceptada |
| [ADR-0025](ADR-0025-duplicate-logical-representative.md) | Representante lógico de un conjunto de duplicados (M0.2) | Aceptada |
| [ADR-0026](ADR-0026-declarative-profiles.md) | Perfiles declarativos y fronteras protegidas (M0.2) | Aceptada |

Los números 0001–0010 corresponden a las decisiones arquitectónicas de
RFC-0001 §6; 0011+ a decisiones de entorno y desarrollo (RFC-0001 §0.1.11).
Nuevas ADR se crean a partir de [TEMPLATE.md](TEMPLATE.md).

Los números son únicos e irrepetibles: 0017–0022 pertenecen al endurecimiento
`v0.1.1-dev` y 0023–0025 al Milestone 0.2. Cuando dos ramas de trabajo
proponen el mismo número, cede la que aún no está publicada (el tag manda).
