# Entorno de desarrollo reproducible

Este documento resume la configuración con la que se validó el bootstrap de
DataForge. No describe el estado vivo de una máquina concreta. Por privacidad
no incluye nombres de usuario, rutas absolutas, identificadores de hardware,
espacio libre ni versiones de parche del sistema operativo.

Las fuentes autoritativas para versiones son `rust-toolchain.toml`,
`package.json`, `pnpm-lock.yaml` y `Cargo.lock`. El estado de una instalación
se comprueba ejecutando los scripts del repositorio; este documento no debe
usarse como inventario de software instalado.

## Plataforma validada

| Campo | Configuración |
| --- | --- |
| Sistema operativo | Windows 11 x64 |
| Shell | Windows PowerShell 5.1 o PowerShell 7 |
| Privilegios para motor y CLI | Usuario estándar, sin administrador |
| WebView2 | Runtime evergreen de Windows, para la app de escritorio |
| Ubicación del repositorio | Cualquier directorio local con permisos de escritura |

Una carpeta sincronizada por OneDrive es compatible, aunque puede generar
churn y bloqueos transitorios. En ese caso conviene mantener el directorio de
compilación fuera de la carpeta sincronizada:

```powershell
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\dataforge-target"
```

## Toolchain

| Herramienta | Política | Fuente |
| --- | --- | --- |
| Git | Versión vigente de Git for Windows | Distribución oficial |
| Node.js | `>=20`, según `package.json` | Distribución oficial |
| pnpm | Versión fijada en `package.json#packageManager` | Registro npm oficial |
| Rust | Canal fijado en `rust-toolchain.toml`; MSRV en `Cargo.toml` | rustup |
| rustfmt / clippy | Componentes del toolchain Rust | rustup |
| MinGW-w64 | Fallback GNU en espacio de usuario | WinLibs vía winget |
| MSVC + Windows SDK | Recomendado para el shell Tauri | Visual Studio Build Tools |

El motor y la CLI pueden desarrollarse con el toolchain GNU documentado en
[ADR-0011](adr/ADR-0011-windows-user-space-toolchain.md). La CI comprueba el
shell Tauri con MSVC.

## Bootstrap

El bootstrap es idempotente y está codificado en:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1
```

Después se valida el entorno real de la máquina actual:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/check-environment.ps1
powershell -ExecutionPolicy Bypass -File scripts/verify-toolchain.ps1
```

Las herramientas opcionales de seguridad se instalan mediante
`scripts/install-dev-plugins.ps1`. GitHub CLI no es necesario para compilar,
probar ni ejecutar DataForge.

## Puerta de calidad

La comprobación completa se deriva de la CI y de la skill
`.codex/skills/rust-quality-gate/SKILL.md`:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
cargo build -p dataforge-cli
cargo check -p dataforge-desktop
pnpm install --frozen-lockfile
pnpm --filter dataforge-desktop typecheck
pnpm --filter dataforge-desktop build
cargo audit
cargo deny check
```

La disponibilidad de `cargo-audit`, `cargo-deny`, MSVC o GitHub CLI debe
consultarse en el momento de ejecutar la puerta; no se infiere de este
documento.

## Consideraciones reproducibles

- `corepack enable` puede requerir escribir en la instalación global de
  Node.js. El bootstrap usa pnpm en espacio de usuario, conforme a ADR-0012.
- Los scripts de Windows con texto no ASCII deben conservar una codificación
  compatible con Windows PowerShell 5.1.
- Si rustup resuelve por defecto al triple MSVC y ese toolchain no está
  disponible, ADR-0011 documenta el fallback GNU + WinLibs.
- El shell Tauri mantiene `crate-type = ["rlib"]`; los tipos de biblioteca
  móvil quedan fuera del alcance actual.
- Los resultados históricos de una máquina no sustituyen una ejecución nueva
  de los scripts y de la puerta de calidad.
