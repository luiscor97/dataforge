<#
.SYNOPSIS
    Instala (solo si faltan) las herramientas de desarrollo de DataForge.
.DESCRIPTION
    Idempotente: detecta instalaciones existentes y no las repite.
    Todo se instala en espacio de usuario, sin elevación:
      - rustup + toolchain estable (winget Rustlang.Rustup)
      - GCC MinGW-w64 de WinLibs si no hay MSVC ni gcc (winget, portable)
      - pnpm 10 vía npm con prefix de usuario
    Las Visual Studio Build Tools (MSVC) requieren elevación y NO se
    instalan aquí; ver docs/adr/ADR-0011-windows-user-space-toolchain.md.
    Prohibido por política: ejecutar scripts remotos sin inspección
    (RFC-0001 §0.1.5).
.NOTES
    Códigos de salida: 0 correcto; 2 falta un prerrequisito no instalable
    (Node/winget); 3 falló una instalación.
#>
[CmdletBinding()]
param(
    # Omite la instalación del toolchain C aunque falte (p. ej. si vas a
    # instalar MSVC manualmente).
    [switch] $SkipCToolchain
)

$ErrorActionPreference = 'Stop'

function Refresh-SessionPath {
    $userPaths = @("$env:USERPROFILE\.cargo\bin", "$env:APPDATA\npm", "$env:LOCALAPPDATA\Microsoft\WinGet\Links")
    $winlibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter 'BrechtSanders.WinLibs*' -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'mingw64\bin' } | Where-Object { Test-Path $_ }
    $env:PATH = (($userPaths + $winlibs) -join ';') + ';' + $env:PATH
}

function Test-Cmd { param([string] $Name) [bool](Get-Command $Name -ErrorAction SilentlyContinue) }

Refresh-SessionPath

# --- Prerrequisitos que este script no puede resolver solo -----------------
if (-not (Test-Cmd 'winget')) {
    Write-Host "winget no está disponible; instala 'App Installer' desde Microsoft Store." -ForegroundColor Red
    exit 2
}
if (-not (Test-Cmd 'node')) {
    Write-Host "Node.js no encontrado. Instálalo con: winget install OpenJS.NodeJS.LTS" -ForegroundColor Red
    exit 2
}

$wingetArgs = @('--silent', '--accept-package-agreements', '--accept-source-agreements', '--disable-interactivity')

# --- Rust -------------------------------------------------------------------
if (-not (Test-Cmd 'rustup')) {
    Write-Host "Instalando rustup (winget Rustlang.Rustup)..."
    winget install --id Rustlang.Rustup @wingetArgs
    if ($LASTEXITCODE -ne 0) { Write-Host "Fallo instalando rustup" -ForegroundColor Red; exit 3 }
    Refresh-SessionPath
} else {
    Write-Host "rustup ya presente: $(rustup --version 2>$null | Select-Object -First 1)"
}

# --- Toolchain C ------------------------------------------------------------
$hasMsvc = Test-Cmd 'cl'
$hasGcc = Test-Cmd 'gcc'
if (-not $SkipCToolchain -and -not $hasMsvc -and -not $hasGcc) {
    Write-Host "Sin MSVC ni GCC. Instalando WinLibs MinGW-w64 (portable, espacio de usuario)..."
    winget install --id BrechtSanders.WinLibs.POSIX.MSVCRT @wingetArgs
    if ($LASTEXITCODE -ne 0) { Write-Host "Fallo instalando WinLibs" -ForegroundColor Red; exit 3 }
    Refresh-SessionPath
}

# Con el fallback GNU, el host por defecto de rustup debe ser el triple GNU
# para que rust-toolchain.toml (canal "stable") resuelva a un toolchain capaz
# de enlazar sin MSVC.
if (-not $hasMsvc) {
    Write-Host "Configurando rustup para el triple GNU (sin MSVC disponible)..."
    rustup set default-host x86_64-pc-windows-gnu | Out-Null
    rustup toolchain install stable-x86_64-pc-windows-gnu --profile default | Out-Null
    rustup default stable-x86_64-pc-windows-gnu | Out-Null
} else {
    rustup toolchain install stable --profile default | Out-Null
}

# --- pnpm --------------------------------------------------------------------
if (-not (Test-Cmd 'pnpm')) {
    Write-Host "Instalando pnpm 10 (npm, prefix de usuario)..."
    npm install -g pnpm@10
    if ($LASTEXITCODE -ne 0) { Write-Host "Fallo instalando pnpm" -ForegroundColor Red; exit 3 }
    Refresh-SessionPath
} else {
    Write-Host "pnpm ya presente: $(pnpm --version 2>$null)"
}

Write-Host "`nHerramientas listas. Verifica con scripts/verify-toolchain.ps1." -ForegroundColor Green
if (-not $hasMsvc) {
    Write-Host "Recordatorio: MSVC sigue pendiente (necesita elevación)." -ForegroundColor Yellow
    Write-Host "  winget install Microsoft.VisualStudio.2022.BuildTools  (workload C++)"
}
exit 0
