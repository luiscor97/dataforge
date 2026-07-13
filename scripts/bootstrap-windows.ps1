<#
.SYNOPSIS
    Bootstrap completo del entorno de desarrollo DataForge en Windows.
.DESCRIPTION
    Orquesta, en orden e idempotentemente:
      1. scripts/check-environment.ps1   (diagnóstico)
      2. scripts/install-dev-tools.ps1   (instala solo lo que falte)
      3. scripts/install-dev-plugins.ps1 (auditoría y utilidades; opcional)
      4. scripts/verify-toolchain.ps1    (pruebas reales de funcionamiento)
    Seguro de repetir: nada se reinstala si ya está presente.
    No requiere administrador; lo que exigiría elevación (MSVC Build Tools)
    queda documentado como acción manual (ADR-0011).
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/bootstrap-windows.ps1 -SkipPlugins
#>
[CmdletBinding()]
param(
    # Omite cargo-audit/cargo-deny/sqlite3 (ahorra la compilación inicial).
    [switch] $SkipPlugins
)

$ErrorActionPreference = 'Stop'
$scripts = $PSScriptRoot

Write-Host "==== DataForge :: bootstrap (Windows) ====" -ForegroundColor Cyan

Write-Host "`n[1/4] Diagnóstico inicial" -ForegroundColor Cyan
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'check-environment.ps1')
$needsInstall = ($LASTEXITCODE -ne 0)

if ($needsInstall) {
    Write-Host "`n[2/4] Instalación de herramientas que faltan" -ForegroundColor Cyan
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'install-dev-tools.ps1')
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Bootstrap detenido: install-dev-tools devolvió $LASTEXITCODE" -ForegroundColor Red
        exit $LASTEXITCODE
    }
} else {
    Write-Host "`n[2/4] Nada que instalar" -ForegroundColor Cyan
}

if (-not $SkipPlugins) {
    Write-Host "`n[3/4] Plugins de desarrollo (auditoría, inspección)" -ForegroundColor Cyan
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'install-dev-plugins.ps1')
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Aviso: install-dev-plugins devolvió $LASTEXITCODE (no bloqueante)" -ForegroundColor Yellow
    }
} else {
    Write-Host "`n[3/4] Plugins omitidos (-SkipPlugins)" -ForegroundColor Cyan
}

Write-Host "`n[4/4] Verificación funcional" -ForegroundColor Cyan
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'verify-toolchain.ps1')
if ($LASTEXITCODE -ne 0) {
    Write-Host "`nBootstrap terminado con errores de verificación." -ForegroundColor Red
    exit 1
}

Write-Host "`nBootstrap completado. Siguientes pasos:" -ForegroundColor Green
Write-Host "  cargo build ; cargo test"
Write-Host "  pnpm install ; pnpm --filter dataforge-desktop build"
exit 0
