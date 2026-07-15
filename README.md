# DataForge

> Motor open source de reconstrucción documental. Ordena, reconstruye y
> migra archivos **sin tocar el origen**.

DataForge analiza discos, carpetas compartidas y copias de seguridad
desordenadas; reconstruye relaciones entre archivos; propone una organización
justificable; y produce una copia verificada criptográficamente, con
trazabilidad de cada decisión. El documento fundacional es
[RFC-0001](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

**Estado actual: Milestone 0.2 — Structural Intelligence (en curso).**
Milestone 0.1 (Safe Inventory Core) está completo: el pipeline llega de la
carpeta caótica a la copia verificada con informe.

Qué existe hoy (real, con pruebas):

- Monorepo Rust (workspace de 12 crates) + pnpm.
- Dominio: IDs tipados, `Project`, `SourceRoot`, `Snapshot`, `AuditEvent`,
  inventario (`PathOccurrence`, `ContentObject`, fingerprints) y la máquina
  de estados completa de RFC-0001 §11.
- SQLite como única fuente de verdad: migraciones `0001_foundation` y
  `0002_inventory` con checksum, repositorios transaccionales y comprobación
  de integridad (`integrity_check`, FK, migraciones, ledger).
- Escaneo seguro (`df-scan`): valida orígenes, inventaría archivos y
  carpetas en snapshots inmutables, registra reparse points sin seguirlos,
  soporta rutas largas de Windows y persiste errores parciales.
- Hashing (`df-hash`): BLAKE3 + SHA-256 en una pasada, fingerprint físico
  con invalidación pre/post (`SOURCE_CHANGED`) y cola reanudable — matar el
  proceso no pierde trabajo.
- Duplicados exactos (mismo tamaño + SHA-256) como informe de evidencia y
  conjuntos materializados en el análisis, cada uno con su **representante
  lógico** (§15.5): la mejor ubicación canónica, elegida por penalización de
  ubicación, limpieza del nombre y profundidad, con la razón explicada — sin
  que ello implique borrar las demás copias.
- **Inteligencia estructural (M0.2)**: firmas Merkle de carpeta (BLAKE3,
  §19.2) y detección de **clones exactos de árbol** (carpetas injertadas con
  subárboles byte a byte idénticos); y **clasificación de contexto** que marca
  contenedores genéricos de bajo valor (Descargas, Escritorio, Backup,
  Recuperado, Copia, Temporales) con la penalización del §18.3. Todo es
  evidencia: nada se propone borrar.
- Planificación (`df-planner`): plan con cobertura completa de cada
  aparición, razones por operación, validación (destinos, colisiones,
  cobertura) y aprobación que congela el plan bajo un SHA-256 canónico.
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
  `plan create/validate/approve`, `execute`, `verify`,
  `report duplicates/tree-clones/contexts`, `audit verify` (con `--json` y
  códigos de salida documentados).
- App de escritorio (Tauri 2 + React + TypeScript strict): crear proyecto,
  abrir proyecto y ver estado, inventario e integridad, usando los mismos
  comandos de `df-facade` que la CLI.

Qué **no** existe todavía (y no está simulado): clones parciales/embebidos de
árbol, perfiles con fronteras protegidas, grafo de entidades, reglas
declarativas, anomalías, consolidación de duplicados, similitud, búsqueda,
plugins, IA.
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
  --source D:\datos\origen
cargo run -p dataforge-cli -- scan --path D:\proyectos\demo
cargo run -p dataforge-cli -- hash --path D:\proyectos\demo
cargo run -p dataforge-cli -- analyze --path D:\proyectos\demo
cargo run -p dataforge-cli -- report duplicates --path D:\proyectos\demo
cargo run -p dataforge-cli -- report tree-clones --path D:\proyectos\demo
cargo run -p dataforge-cli -- report contexts --path D:\proyectos\demo
cargo run -p dataforge-cli -- plan create --path D:\proyectos\demo
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
crates/df-*      motor: error, domain, ledger, db, scan, hash,
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

## Contribuir

Lee [CONTRIBUTING.md](CONTRIBUTING.md) (DCO, puerta de calidad, ADR/RFC) y
[GOVERNANCE.md](GOVERNANCE.md). Seguridad: [SECURITY.md](SECURITY.md).

## Licencia

Doble licencia [MIT](LICENSE-MIT) o [Apache-2.0](LICENSE-APACHE), a elección
del usuario.
