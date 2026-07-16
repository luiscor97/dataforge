# DataForge

> Motor open source de reconstrucción documental. Ordena, reconstruye y
> migra archivos **sin tocar el origen**.

DataForge analiza discos, carpetas compartidas y copias de seguridad
desordenadas; reconstruye relaciones entre archivos; propone una organización
justificable; y produce una copia verificada criptográficamente, con
trazabilidad de cada decisión. El documento fundacional es
[RFC-0001](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

**Estado actual: Milestone 0.2 — Structural Intelligence (cierre, objetivo
`0.2.0`), sobre la base endurecida por Filesystem Safety Hardening.** El
pipeline completo llega de la carpeta caótica a una copia verificada, y M0.2
añade diagnóstico estructural, perfiles, reglas seguras y revisión auditable.

DataForge **no está listo para producción general**. Lo que hay hoy es una
**copia segura, explicable y verificable**: inventaría un origen sin tocarlo,
detecta relaciones estructurales acotadas, propone un plan conservador y audita
cada decisión. Todavía no interpreta el significado de los documentos ni
reconstruye automáticamente expedientes a partir de su contenido.

Qué debes saber antes de apuntarlo a datos reales:

- **Windows es la única plataforma con la seguridad implementada.** En Linux y
  macOS la ejecución **se bloquea** en lugar de fingir garantías que no
  tenemos.
- **NAS/UNC es experimental** y no está validado: sin identidad física del
  filesystem, la detección de sustituciones baja a *degradada*.
- **Las garantías dependen de lo que ofrezca el filesystem.** DataForge lo dice
  explícitamente en cada fingerprint en vez de suponerlo.
- Frente a quien pueda editar la base del proyecto, la garantía es de
  **detección**, no de prevención.

Detalle completo en
[`docs/threat-model/filesystem-hardening.md`](docs/threat-model/filesystem-hardening.md).

Qué existe hoy (real, con pruebas):

- Monorepo Rust (workspace de 11 crates) + pnpm.
- **Frontera segura del sistema de archivos** (`df-fs-safety`): ninguna
  escritura sale del output root a través de junctions, symlinks o reparse
  points; el finalize no sobrescribe por semántica de plataforma, no por una
  comprobación previa; el verificador nunca sigue enlaces.
- **Manifiesto de ejecución inmutable**: lo aprobado es exactamente lo
  ejecutado, y manipularlo se detecta criptográficamente.
- Dominio: IDs tipados, `Project`, `SourceRoot`, `Snapshot`, `AuditEvent`,
  inventario (`PathOccurrence`, `ContentObject`, fingerprints) y la máquina
  de estados completa de RFC-0001 §11.
- SQLite como única fuente de verdad: migraciones `0001`–`0010` con checksum,
  repositorios transaccionales y comprobación de integridad
  (`integrity_check`, FK, migraciones, ledger).
- Escaneo seguro (`df-scan`): valida orígenes, inventaría archivos y
  carpetas en snapshots inmutables, registra reparse points sin seguirlos,
  soporta rutas largas de Windows y persiste errores parciales.
- Hashing (`df-hash`): BLAKE3 + SHA-256 en una pasada, fingerprint físico
  con invalidación pre/post (`SOURCE_CHANGED`) y cola reanudable — matar el
  proceso no pierde trabajo.
- Duplicados exactos (mismo tamaño + SHA-256) materializados como evidencia,
  cada conjunto con un **representante lógico** explicado. `REPORT_ONLY` es la
  política por defecto; tres políticas opt-in pueden representar copias
  demostradas, pero conservan siempre contextos desconocidos y fronteras
  protegidas. El origen nunca se borra.
- **Inteligencia estructural M0.2**: firmas Merkle de carpeta y clones exactos;
  relaciones `PARTIAL_TREE_CLONE` y `TREE_EMBEDDED` acotadas por cardinalidad,
  con recuentos del contenido exclusivo de ambos lados. Las relaciones son
  evidencia para conservación/revisión y nunca autorizan omitir una rama.
- **Perfiles declarativos embebidos**: `generic` clasifica contenedores de bajo
  valor y `legal` añade fronteras protegidas por nombre para expedientes,
  procedimientos, asuntos, clientes y personas. Los ids desconocidos se
  rechazan; no hay fallback silencioso que retire protección.
- **Reglas, anomalías y revisión**: reglas versionadas de nombre de archivo
  solo pueden elegir operaciones de copia seguras; se persisten anomalías de
  nombres/contenidos, rutas, lectura y estructura; la cola de revisión y sus
  decisiones con justificación son append-only. Una revisión pendiente se
  copia como `COPY_REVIEW`, no se descarta.
- **Frontera de completitud del análisis**: un marcador append-only por
  snapshot distingue un informe vacío válido de una caída entre etapas. Los
  informes fallan cerrados hasta que el marcador y un estado estable confirman
  el final del análisis.
- Planificación (`df-planner`): plan con cobertura completa de cada aparición,
  política explícita de duplicados, guía de reglas/revisión, razones por
  operación, validación y aprobación que congela un manifiesto bajo SHA-256.
- Recuperación de fases: `ANALYZING`, `PLANNING` y `PLAN_REVIEW` se pueden
  reanudar sin repetir la transición inicial, crear otra versión del mismo
  plan ni duplicar el manifiesto/evento de aprobación ya persistido.
- Ejecución segura (`df-executor`): copia por archivo con fingerprint
  pre/post, archivo parcial, doble hash en streaming, comparación, rename
  atómico; sin sobrescritura, con colisiones resueltas de forma
  determinista, errores tipados y reanudación tras interrupción.
- Verificación independiente (`df-verifier`): re-hash de cada copia,
  cobertura, plan no manipulado, parciales, archivos no registrados y
  origen intacto; veredicto `COMPLETED`, `COMPLETED_WITH_WARNINGS` o
  `FAILED`.
- Ledger de auditoría append-only con encadenamiento SHA-256, verificación
  y eventos de todo el pipeline.
- CLI `dataforge`: `project create/status`, `scan`, `hash`, `analyze`,
  `plan create/validate/approve`, `review list/decide`, `execute`, `verify`,
  `report duplicates/tree-clones/tree-relations/contexts/anomalies` y
  `audit verify` (con `--json` y códigos de salida documentados).
- App de escritorio (Tauri 2 + React + TypeScript strict): crear/abrir proyecto
  y ver estado, inventario, integridad y diagnóstico estructural M0.2, usando
  la misma `df-facade` que la CLI.

Qué **no** existe todavía (y no está simulado): extracción de contenido,
relaciones documentales por significado, reconstrucción automática de
expedientes, búsqueda, informes exportables, perfiles de usuario en runtime,
plugins o IA. Las relaciones M0.2 comparan identidades exactas de contenido y
nombres declarados; no son una comprensión semántica. Tampoco se consolidan
automáticamente árboles completos o parciales.
Ver el [roadmap](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md#45-roadmap-maestro).

## Inicio rápido (Windows)

```powershell
# 1. Preparar el entorno (idempotente; detecta lo ya instalado)
powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1

# 2. Compilar y probar el motor + CLI
cargo build
cargo test

# 3. CLI: el pipeline completo hasta la copia verificada
cargo run -p dataforge-cli -- project create `
  --name "Mi proyecto" `
  --path  D:\proyectos\demo `
  --output-root D:\salidas\demo `
  --profile legal `
  --source D:\datos\origen
cargo run -p dataforge-cli -- scan --path D:\proyectos\demo
cargo run -p dataforge-cli -- hash --path D:\proyectos\demo
cargo run -p dataforge-cli -- analyze --path D:\proyectos\demo
cargo run -p dataforge-cli -- report duplicates --path D:\proyectos\demo
cargo run -p dataforge-cli -- report tree-clones --path D:\proyectos\demo
cargo run -p dataforge-cli -- report tree-relations --path D:\proyectos\demo
cargo run -p dataforge-cli -- report contexts --path D:\proyectos\demo
cargo run -p dataforge-cli -- report anomalies --path D:\proyectos\demo
cargo run -p dataforge-cli -- review list --path D:\proyectos\demo
# Opcional: review decide --item <id> --decision COPY_ACTIVE --reason "..."
cargo run -p dataforge-cli -- plan create --path D:\proyectos\demo `
  --duplicate-policy REPORT_ONLY
cargo run -p dataforge-cli -- plan approve --path D:\proyectos\demo
cargo run -p dataforge-cli -- execute --path D:\proyectos\demo
cargo run -p dataforge-cli -- verify --path D:\proyectos\demo
cargo run -p dataforge-cli -- audit verify --path D:\proyectos\demo
cargo run -p dataforge-cli -- project status --path D:\proyectos\demo

# 4. App de escritorio (requiere toolchain MSVC; ver nota)
pnpm install
pnpm --filter dataforge-desktop tauri dev
```

> **Nota Windows/MSVC:** compilar el shell Tauri requiere las Visual Studio
> Build Tools. Sin ellas, el motor y la CLI funcionan igualmente con el
> fallback GNU documentado en
> [ADR-0011](docs/adr/ADR-0011-windows-user-space-toolchain.md), y el
> frontend se valida con `pnpm --filter dataforge-desktop build`.

## Estructura

```text
apps/cli/        CLI `dataforge`
apps/desktop/    Tauri 2 + React + TS strict (cliente de df-facade)
crates/df-*      motor: error, domain, fs-safety, ledger, db, scan, hash,
                 planner, executor, verifier, facade
docs/            RFCs, ADRs, arquitectura, threat model, guías
scripts/         bootstrap reproducible del entorno (PowerShell)
.codex/skills/   skills del repositorio para agentes de codificación
```

## Garantías de diseño

1. El origen es inmutable; no hay código de borrado ni sobrescritura.
2. SQLite es la única fuente de verdad; los informes son exportaciones.
3. Todo cambio de estado pasa por la máquina de estados y queda registrado
   en un ledger hash-chained verificable.
4. La interfaz no contiene lógica crítica: CLI y escritorio usan `df-facade`.
5. Un hallazgo automático es evidencia; solo una política explícita o una
   decisión humana puede cambiar la operación propuesta, y ambas permanecen
   dentro del conjunto de copias seguras.
6. Ningún informe estructural se publica antes del marcador final de su
   snapshot.

## Contribuir

Lee [CONTRIBUTING.md](CONTRIBUTING.md) (DCO, puerta de calidad, ADR/RFC) y
[GOVERNANCE.md](GOVERNANCE.md). Seguridad: [SECURITY.md](SECURITY.md).

## Licencia

Doble licencia [MIT](LICENSE-MIT) o [Apache-2.0](LICENSE-APACHE), a elección
del usuario.
