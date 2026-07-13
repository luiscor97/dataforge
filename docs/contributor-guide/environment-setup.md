# Preparación del entorno (Windows)

DataForge se desarrolla inicialmente en Windows 10/11 x64. Todo el proceso
está automatizado e idempotente:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1
```

El script diagnostica, instala **solo lo que falta**, y verifica el
resultado compilando y ejecutando código real. Puede repetirse sin efectos.

## Qué necesita el proyecto

| Herramienta | Versión | Fuente oficial | Notas |
| --- | --- | --- | --- |
| Git | ≥ 2.40 | winget `Git.Git` | |
| Rust (rustup) | stable, componentes rustfmt+clippy | winget `Rustlang.Rustup` | canal pineado por `rust-toolchain.toml` |
| Toolchain C | MSVC (preferido) o MinGW-w64 | VS Build Tools / winget `BrechtSanders.WinLibs.POSIX.MSVCRT` | necesario para enlazar y para SQLite bundled |
| Node.js | ≥ 20 (LTS) | winget `OpenJS.NodeJS.LTS` | |
| pnpm | 10.x | `npm install -g pnpm@10` | versión exacta en `package.json#packageManager` |
| Tauri CLI | 2.x | devDependency npm | se instala con `pnpm install` |
| WebView2 Runtime | evergreen | preinstalado en Win 11 | solo para ejecutar la app |

Opcionales: `cargo-audit`, `cargo-deny`, `sqlite3`
(`scripts/install-dev-plugins.ps1`).

## Sin permisos de administrador

Es el caso soportado por defecto (ADR-0011): rustup + toolchain GNU +
WinLibs GCC funcionan íntegramente en espacio de usuario. La única pieza que
exige elevación son las **Visual Studio Build Tools** (MSVC), necesarias
para el build oficial del shell Tauri en Windows:

```powershell
# Requiere UAC; ejecutar cuando haya permisos:
winget install Microsoft.VisualStudio.2022.BuildTools
# workload: "Desktop development with C++" (MSVC v143 + Windows 11 SDK)
rustup set default-host x86_64-pc-windows-msvc
```

Sin MSVC puedes igualmente: compilar y testear todo el motor y la CLI,
tipar/compilar el frontend, y hacer `cargo check`/`cargo build` del crate de
escritorio con el toolchain GNU.

## Repositorio dentro de OneDrive

Si tu copia de trabajo está en una carpeta sincronizada por OneDrive:

- exporta un target dir fuera de OneDrive para acelerar builds y evitar
  bloqueos del sincronizador:
  `$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\dataforge-target"`;
- `.npmrc` ya fija `node-linker=hoisted` para que pnpm no use junctions.

## Verificación

```powershell
powershell -ExecutionPolicy Bypass -File scripts/verify-toolchain.ps1
cargo build
cargo test
pnpm install
pnpm --filter dataforge-desktop build
```

El estado real de la máquina de referencia está en
[docs/environment-report.md](../environment-report.md).
