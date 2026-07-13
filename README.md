# DataForge

> Motor open source de reconstrucción documental. Ordena, reconstruye y
> migra archivos **sin tocar el origen**.

DataForge analiza discos, carpetas compartidas y copias de seguridad
desordenadas; reconstruye relaciones entre archivos; propone una organización
justificable; y produce una copia verificada criptográficamente, con
trazabilidad de cada decisión. El documento fundacional es
[RFC-0001](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

**Estado actual: Milestone 0.0 — Repository Foundation (`v0.0.1-dev`).**

Qué existe hoy (real, con pruebas):

- Monorepo Rust (workspace de 7 crates) + pnpm.
- Dominio: IDs tipados, `Project`, `SourceRoot`, `Snapshot`, `AuditEvent` y
  la máquina de estados completa de RFC-0001 §11.
- SQLite como única fuente de verdad: migración `0001_foundation`,
  migraciones con checksum, repositorios transaccionales y comprobación de
  integridad (`integrity_check`, FK, migraciones, ledger).
- Ledger de auditoría append-only con encadenamiento SHA-256 y verificación.
- CLI `dataforge`: `project create` y `project status` (con `--json` y
  códigos de salida documentados).
- App de escritorio (Tauri 2 + React + TypeScript strict): crear proyecto,
  abrir proyecto y ver estado/integridad, usando los mismos comandos de
  `df-facade` que la CLI.

Qué **no** existe todavía (y no está simulado): escaneo, hashing,
duplicados, planes, copia, verificación de copias, búsqueda, plugins, IA.
Ver el [roadmap](docs/rfcs/RFC-0001-dataforge-foundation-and-roadmap.md#45-roadmap-maestro).

## Inicio rápido (Windows)

```powershell
# 1. Preparar el entorno (idempotente; detecta lo ya instalado)
powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1

# 2. Compilar y probar el motor + CLI
cargo build
cargo test

# 3. CLI
cargo run -p dataforge-cli -- project create `
  --name "Mi proyecto" `
  --path  D:\proyectos\demo `
  --output-root D:\salidas\demo
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
crates/df-*      motor: error, domain, ledger, db, facade
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
