# DataForge

> Motor open source de reconstrucción documental. Ordena, reconstruye y
> migra archivos **sin tocar el origen**.

DataForge analiza discos, carpetas compartidas y copias de seguridad
desordenadas; reconstruye relaciones entre archivos; propone una organización
justificable; y produce una copia verificada criptográficamente, con
trazabilidad de cada decisión. El documento fundacional es
[RFC-0001](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

**Estado actual: 1.0.0 — Milestone 1.0 "Stable Reconstruction Platform".**
El pipeline completo llega de la carpeta caótica a una copia verificada —
probado hasta **1.000.000 de archivos** con verificación independiente
`COMPLETED` — e incluye inteligencia documental y multimedia, plugins WASM,
IA asistida BYOK, snapshots incrementales y NAS endurecido. Los contratos
públicos (schemas, algoritmos, ABI, 19 migraciones) están congelados bajo
test de regresión; el mapa garantía→evidencia de la release vive en
[docs/release/m1.0-acceptance.md](docs/release/m1.0-acceptance.md).

La 1.0 es **Windows-first y de alcance declarado** — no una promesa de
producción universal. Lo que hay es una **copia segura, explicable y
verificable**: inventaría un origen sin tocarlo,
detecta relaciones estructurales y versiones binarias acotadas, extrae e
indexa contenido documental, propone un plan conservador y audita cada
decisión. No reconstruye automáticamente expedientes por significado ni deja
que una búsqueda/consulta cambie el origen o el plan.

Qué debes saber antes de apuntarlo a datos reales:

- **Windows es la única plataforma con la seguridad implementada.** En Linux y
  macOS la ejecución **se bloquea** en lugar de fingir garantías que no
  tenemos.
- **NAS/UNC está endurecido pero degradado por naturaleza**: la validación
  clasifica el filesystem real (red, FAT, desconocido) y el executor rechaza
  destinos sin identidad física salvo reconocimiento explícito
  (`--allow-degraded-destination`); sin identidad física, la detección de
  sustituciones baja a *degradada* y DataForge lo dice, no lo oculta.
- **Las garantías dependen de lo que ofrezca el filesystem.** DataForge lo dice
  explícitamente en cada fingerprint en vez de suponerlo.
- Frente a quien pueda editar la base del proyecto, la garantía es de
  **detección**, no de prevención.

Detalle completo en
[`docs/threat-model/filesystem-hardening.md`](docs/threat-model/filesystem-hardening.md).

Qué existe hoy (real, con pruebas):

- Monorepo Rust (crates de motor, sidecars aislados, CLI, shell desktop y
  herramientas de corpus) + pnpm.
- **Frontera segura del sistema de archivos** (`df-fs-safety`): ninguna
  escritura sale del output root a través de junctions, symlinks o reparse
  points; el finalize no sobrescribe por semántica de plataforma, no por una
  comprobación previa; el verificador nunca sigue enlaces.
- **Manifiesto de ejecución inmutable**: lo aprobado es exactamente lo
  ejecutado, y manipularlo se detecta criptográficamente.
- Dominio: IDs tipados, `Project`, `SourceRoot`, `Snapshot`, `AuditEvent`,
  inventario (`PathOccurrence`, `ContentObject`, fingerprints) y la máquina
  de estados completa de RFC-0001 §11.
- SQLite como única fuente de verdad: migraciones `0001`–`0019` con checksum,
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
- **Similitud y versionado M0.3** (`df-similarity`): FastCDC streaming,
  chunks BLAKE3 normalizados, MinHash/LSH acotado y fallback de chunks raros.
  Cada candidato se reevalúa con similitud exacta ponderada por bytes; los
  runs son configurables, reanudables, auditados y sellados. SHA-256 sigue
  siendo la única identidad y una relación nunca crea una operación.
- **Inteligencia documental M0.4** (`df-extract`): TXT/HTML/DOCX/EML/ZIP
  normalizados y segmentados con límites absolutos; adjuntos y rutas de ZIP
  permanecen virtuales; correo conserva metadata y construye hilos básicos
  deterministas. PDF superior, adjunto o contenido en ZIP se procesa solo en
  `df-extract-worker`, bajo memoria/deadline de sistema operativo; sin sidecar
  queda `LIMITED`, nunca cae a un parser dentro del proceso principal.
- **Búsqueda y consulta reconstruibles**: Tantivy indexa texto, ruta y contexto;
  Parquet expone metadata acotada a DataFusion. Los artefactos tienen schema y
  digest versionados, se fijan mediante leases contra sustitución y se pueden
  regenerar desde SQLite. SQL de clientes se ejecuta solo en
  `df-query-worker`, sin DDL/DML/spill y con límites de filas, celdas, memoria,
  bytes y tiempo.
- **Inteligencia multimedia M0.5** (`df-media`): metadata acotada de imagen/
  audio/vídeo en worker aislado, relaciones solo-revisión selladas por run;
  nunca una operación automática (ADR-0032).
- **Plugins WASM M0.6** (`df-plugin`): registro persistido con hash y firma
  re-verificados en cada carga, ABI WIT versionada, Wasmtime sin WASI ni
  filesystem por defecto, límites de memoria/tiempo por capacidad y runs
  sellados sobre evidencia (ADR-0033).
- **IA asistida BYOK M0.7** (`df-ai`): explicaciones opcionales sobre
  evidencia con la clave del usuario (Windows Credential Manager, nunca en SQLite ni
  en el ledger), consentimiento por digest en cada invocación, transportes
  Anthropic/OpenAI solo en el borde de la fachada y auditoría de cada
  asistencia; la IA no decide ni modifica planes (ADR-0034).
- **Escala y evolución M0.8**: pipeline completo probado con 1.000.000 de
  archivos (1.093.705 operaciones ejecutadas al primer intento, verificación
  independiente `COMPLETED`, memoria acotada); snapshots incrementales
  opt-in que solo reusan identidad física probada byte-idéntica
  (ADR-0035); NAS endurecido con clasificación real del filesystem y
  destino degradado solo con reconocimiento explícito (ADR-0036).
- **Estabilización M0.9**: contratos congelados con test de regresión
  (ADR-0037), fuzzing de los parsers de entrada no confiable, SBOM CycloneDX
  reproducible, linkado reproducible verificado en CI (ADR-0038), firma de
  release keyless con Sigstore (ADR-0039), manual de usuario y threat model
  consolidado.
- Planificación (`df-planner`): plan con cobertura completa de cada aparición,
  política explícita de duplicados, guía de reglas/revisión, razones por
  operación, validación y aprobación que congela un manifiesto bajo SHA-256.
- Recuperación de fases: `ANALYZING`, `PLANNING` y `PLAN_REVIEW` se pueden
  reanudar sin repetir la transición inicial, crear otra versión del mismo
  plan ni duplicar el manifiesto/evento de aprobación ya persistido.
- Ejecución segura (`df-executor`): copia por archivo con fingerprint
  pre/post, archivo parcial, doble hash en streaming, comparación, rename
  atómico; sin sobrescritura, con colisiones resueltas de forma
  determinista, errores tipados y reanudación tras interrupción. Los parciales
  usan `.dataforge-partial-<operation-id>-<lease-token>` (sin repetir nombres
  que pueden ocupar 255 unidades UTF-16). La propiedad se reclama solo tras
  `create_new`, con la identidad física capturada desde ese mismo handle; un
  nombre/token o `RUNNING` sin identidad nunca autorizan borrar. Los sufijos
  de colisión recortan solo el stem cuando necesitan reservar espacio.
- Verificación independiente (`df-verifier`): re-hash de cada copia,
  cobertura, plan no manipulado, parciales, archivos no registrados y
  origen intacto; veredicto `COMPLETED`, `COMPLETED_WITH_WARNINGS` o
  `FAILED`.
- Ledger de auditoría append-only con encadenamiento SHA-256, verificación
  y eventos de todo el pipeline.
- CLI `dataforge`: `project create/status`, `scan`, `hash [--incremental]`,
  `analyze`, `similarity`, `content extract/fail/build/search/query`,
  `media`, `plugin register/run/list`, `ai key/explain/audits`,
  `plan create/validate/approve`, `review list/decide`,
  `execute [--allow-degraded-destination]`, `verify`, informes de
  duplicados/árboles/contextos/anomalías/similitudes y `audit verify`
  (con `--json` y códigos de salida documentados).
- App de escritorio (Tauri 2 + React + TypeScript strict): crear/abrir proyecto
  y ver estado, inventario, integridad, diagnóstico estructural M0.2,
  relaciones de versiones M0.3 y diagnóstico multimedia M0.5; M0.4 permite
  extraer/reanudar/cerrar, construir artefactos, buscar y consultar SQL con
  estados accesibles, usando la misma `df-facade` que la CLI.

Qué **no** existe todavía (y no está simulado): relaciones documentales por
significado, reconstrucción automática de expedientes, daemon en segundo
plano, firma Authenticode/SmartScreen (los artefactos se firman con Sigstore
keyless, ADR-0039, pero eso no es una firma de código de Windows) e informes
finales exportables. La extracción produce evidencia derivada y búsquedas; la
IA explica sobre evidencia con consentimiento — ninguna de las dos es
comprensión semántica ni autoriza consolidar contenido o árboles
automáticamente.
Ver el [roadmap](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md#45-roadmap-maestro)
y el [mapa de aceptación M1.0](docs/release/m1.0-acceptance.md).

## Inicio rápido (Windows)

> Guía completa de uso (todos los comandos, capacidades y garantías) en el
> [Manual de usuario](docs/manual/README.md).

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
cargo run -p dataforge-cli -- similarity --path D:\proyectos\demo
cargo run -p dataforge-cli -- report similarities --path D:\proyectos\demo
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
crates/df-*      motor: error, domain, fs-safety, process-safety, ledger,
                 db, scan, hash, similarity, extract, search, query, media,
                 plugin, ai, planner, executor, verifier, facade
tools/           df-corpus (corpus sintético + escala,
                 docs/testing/corpus-and-scale.md) y los sidecars aislados
                 df-extract-worker, df-query-worker, df-media-worker
fuzz/            dianas cargo-fuzz de los parsers de entrada no confiable
docs/            RFCs, ADRs, arquitectura, threat model, manual, release
scripts/         bootstrap del entorno (PowerShell) y generador de SBOM
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
7. Similitud no es identidad: MinHash solo genera candidatos, el porcentaje
   publicado se recalcula desde chunks y ninguna relación modifica un plan.

## Contribuir

Lee [CONTRIBUTING.md](CONTRIBUTING.md) (DCO, puerta de calidad, ADR/RFC) y
[GOVERNANCE.md](GOVERNANCE.md). Seguridad: [SECURITY.md](SECURITY.md).

## Licencia

Doble licencia [MIT](LICENSE-MIT) o [Apache-2.0](LICENSE-APACHE), a elección
del usuario.
