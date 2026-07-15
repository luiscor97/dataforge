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
| [ADR-0019](ADR-0019-file-fingerprint-v2.md) | Fingerprint físico v2 (v0.1.1) | Aceptada |
| [ADR-0020](ADR-0020-raw-path-representation.md) | Representación raw de rutas (v0.1.1) | Aceptada |
| [ADR-0021](ADR-0021-platform-no-replace-finalization.md) | Finalize no-replace por plataforma y durabilidad (v0.1.1) | Aceptada |

| [ADR-0022](ADR-0022-atomic-project-initialization.md) | Creación atómica de proyectos y marker endurecido (v0.1.1) | Aceptada |

Los números 0001–0010 corresponden a las decisiones arquitectónicas de
RFC-0001 §6; 0011+ a decisiones de entorno y desarrollo (RFC-0001 §0.1.11).
Nuevas ADR se crean a partir de [TEMPLATE.md](TEMPLATE.md).
