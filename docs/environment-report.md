# Informe de entorno — máquina de referencia

**Fecha:** 2026-07-13
**Generado durante:** bootstrap del Milestone 0.0
**Repositorio:** `C:\Users\luisc\OneDrive\Escritorio\appppppppppp` (carpeta sincronizada por OneDrive)

## Sistema

| Campo | Valor |
| --- | --- |
| Sistema operativo | Microsoft Windows 11 Home (build 10.0.26200) |
| Arquitectura | x64 |
| Shell principal | Windows PowerShell 5.1 (también Git Bash) |
| Privilegios | usuario estándar, **sin** administrador |
| Espacio libre en C: | ~351 GB |
| WebView2 Runtime | 150.0.4078.65 (preinstalado) |

## Herramientas preexistentes (no instaladas por el bootstrap)

| Herramienta | Versión | Ubicación |
| --- | --- | --- |
| Git | 2.55.0.windows.2 | `C:\Program Files\Git` |
| Node.js | v24.18.0 (LTS) | `C:\Program Files\nodejs` |
| npm | 11.6.x | con Node |
| Corepack | 0.35.0 | con Node (no usado; ver ADR-0012) |
| winget | 1.29.280 | App Installer |

## Herramientas instaladas durante el bootstrap

| Herramienta | Versión | Fuente | Comando |
| --- | --- | --- | --- |
| rustup | 1.29.0 | winget `Rustlang.Rustup` (manifiesto oficial; winget verifica el SHA-256 del instalador) | `winget install --id Rustlang.Rustup --silent ...` |
| Rust stable (GNU) | rustc/cargo 1.97.0, host `x86_64-pc-windows-gnu` | rustup (canales firmados) | `rustup toolchain install stable-x86_64-pc-windows-gnu --profile default; rustup set default-host x86_64-pc-windows-gnu` |
| rustfmt / clippy | incluidos (perfil default) | rustup | — |
| MinGW-w64 GCC (WinLibs) | 16.1.0 (msvcrt-posix-seh, r3) | winget `BrechtSanders.WinLibs.POSIX.MSVCRT` (portable, espacio de usuario) | `winget install --id BrechtSanders.WinLibs.POSIX.MSVCRT --silent ...` |
| pnpm | 10.34.5 | registro npm oficial, prefix de usuario `%APPDATA%\npm` | `npm install -g pnpm@10` |

Licencias de lo instalado: rustup/Rust (MIT OR Apache-2.0), GCC/MinGW-w64
(GPL con runtime exception — solo herramienta de build, no se distribuye),
pnpm (MIT). Compatibles con el proyecto.

## Plugins de desarrollo

- `@tauri-apps/cli` 2.9.x, `vite` 7.3.x, `typescript` 5.9.x,
  `@vitejs/plugin-react` — devDependencies del workspace pnpm (lockfile).
- `cargo-audit` / `cargo-deny`: **no instalados localmente** todavía
  (compilación larga); corren en CI y pueden instalarse con
  `scripts/install-dev-plugins.ps1`. Decisión en ADR-0013.
- Servidores MCP: ninguno (sin necesidad concreta en este hito, §0.1.3).

## Skills

- Del entorno del agente: las estándar de Claude Code (sin cambios).
- Creadas para el repositorio (`.codex/skills/`): `bootstrap-environment`,
  `rust-quality-gate`, `sqlite-migrations`, `dataforge-invariants`
  (política en ADR-0014).

## Variables de entorno relevantes

| Variable | Valor | Motivo |
| --- | --- | --- |
| `CARGO_TARGET_DIR` | `%LOCALAPPDATA%\dataforge-target` (recomendada, no persistida) | el repo vive en OneDrive; el target fuera evita el churn del sincronizador |
| PATH (usuario) | + `%USERPROFILE%\.cargo\bin`, `%APPDATA%\npm`, WinLibs `mingw64\bin` | añadidos por los instaladores |

## Verificaciones ejecutadas

- `scripts/check-environment.ps1` → exit 0 (todo lo obligatorio presente).
- `scripts/verify-toolchain.ps1` → exit 0 (compila y ejecuta un binario
  Rust real; node y pnpm ejecutan).
- `cargo build` / `cargo test` / `cargo clippy` / `cargo fmt --check` → OK.
- `pnpm install` / `typecheck` / `vite build` → OK.
- CLI probada de extremo a extremo (`project create` + `project status`,
  humano y `--json`), con verificación de que el origen queda intacto.

## Incidencias

1. `corepack enable` requiere escribir en `C:\Program Files\nodejs` →
   descartado; pnpm instalado vía npm en espacio de usuario (ADR-0012).
2. `package.json#packageManager` inicial (10.18.3) no coincidía con el pnpm
   instalado (10.34.5) y el auto-switch de pnpm fallaba → alineado a
   10.34.5.
3. Los `.ps1` con acentos necesitan UTF-8 **con BOM** para PowerShell 5.1 →
   re-codificados.
4. `rust-toolchain.toml` (canal `stable`) resolvía al triple MSVC por
   defecto → `rustup set default-host x86_64-pc-windows-gnu`.
5. El `crate-type = ["staticlib", "cdylib", "rlib"]` de la plantilla Tauri
   rompe el enlace GNU ("export ordinal too large") → `["rlib"]`, ya que
   los tipos extra solo sirven para móvil (fuera de alcance). Con ello el
   escritorio compila, enlaza y abre también con GNU (queda un aviso benigno
   del linker por el manifest embebido; MSVC/CI no lo tiene).

## Bloqueos reales (acción manual pendiente)

| Bloqueo | Causa | Alternativa aplicada | Acción pendiente |
| --- | --- | --- | --- |
| Visual Studio Build Tools (MSVC) | instalador machine-wide con elevación UAC; sesión sin administrador (RFC §0.1.6) | toolchain GNU + WinLibs (ADR-0011); CI compila con MSVC | `winget install Microsoft.VisualStudio.2022.BuildTools` (workload C++) con permisos, luego `rustup set default-host x86_64-pc-windows-msvc` |
| GitHub CLI | MSI machine-wide (elevación); además no hay remoto GitHub aún | no necesario para el hito | instalar al publicar el repositorio |

## Notas de reproducibilidad

Todo el bootstrap está codificado en `scripts/bootstrap-windows.ps1`
(idempotente). En una máquina Windows limpia con winget y Node LTS, el
script reproduce este entorno sin intervención manual salvo los bloqueos
documentados arriba.

---

# Anexo — máquina 2 (bootstrap del 2026-07-14)

**Repositorio:** `C:\Users\Usuario\Desktop\dataforge-main` (sin `.git`;
ver "Incidencias").

## Sistema

| Campo | Valor |
| --- | --- |
| Sistema operativo | Microsoft Windows 11 Home (build 10.0.26200) |
| Arquitectura | x64 |
| Shell principal | PowerShell 7 (pwsh) |
| Privilegios | usuario estándar |

## Preexistente

| Herramienta | Versión |
| --- | --- |
| Git | 2.54.0.windows.1 |
| Node.js | v26.3.0 |
| winget | 1.29.280 |

## Instalado (mismos comandos y fuentes que la máquina 1)

| Herramienta | Versión | Fuente |
| --- | --- | --- |
| rustup | 1.29.0 | winget `Rustlang.Rustup` (hash verificado por winget) |
| Rust stable (GNU) | rustc/cargo 1.97.0, host `x86_64-pc-windows-gnu` | rustup |
| MinGW-w64 GCC (WinLibs) | 16.1.0 (msvcrt-posix-seh, r3) | winget `BrechtSanders.WinLibs.POSIX.MSVCRT` |
| pnpm | 10.x | `npm install -g pnpm@10` |

## Verificaciones ejecutadas (2026-07-14)

- `cargo fmt --check` / `cargo clippy -D warnings` / `cargo test` → OK
  (Milestone 0.0 validado en esta máquina antes de continuar).
- `pnpm install --frozen-lockfile` / `pnpm --filter dataforge-desktop
  build` → OK.
- Pipeline M0.1 probado de extremo a extremo con el binario real
  (`project create` → `scan` → `hash` → `report duplicates` →
  `audit verify` → `project status`).

## Incidencias

1. La carpeta de trabajo llegó **sin historial git** (descarga tipo ZIP,
   nombre `dataforge-main`). Pendiente: re-vincular con el repositorio o
   `git init` + remoto antes de seguir acumulando cambios.
2. Mismos bloqueos que la máquina 1 (MSVC Build Tools y GitHub CLI
   requieren elevación); mismo fallback GNU de ADR-0011.
