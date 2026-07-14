<#
.SYNOPSIS
    Comprueba qué herramientas de desarrollo de DataForge están disponibles.
.DESCRIPTION
    Solo lee el sistema: no instala nada. Devuelve 0 si todo lo obligatorio
    está presente; 1 si falta algo (la lista se imprime al final).
    Idempotente y seguro de ejecutar tantas veces como se quiera.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Continue'
$missing = @()

function Test-Tool {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [string] $Command,
        [string] $VersionArgs = '--version',
        [switch] $Required
    )
    $cmd = Get-Command $Command -ErrorAction SilentlyContinue
    if ($cmd) {
        $version = ''
        try { $version = (& $Command $VersionArgs 2>$null | Select-Object -First 1) } catch {}
        Write-Host ("  [OK]      {0,-14} {1}" -f $Name, $version)
        return $true
    }
    if ($Required) { $script:missing += $Name }
    $tag = if ($Required) { '[FALTA]' } else { '[opcional]' }
    Write-Host ("  {0,-9} {1}" -f $tag, $Name)
    return $false
}

Write-Host "== DataForge :: comprobación de entorno =="
Write-Host ("Sistema: {0} ({1})" -f (Get-CimInstance Win32_OperatingSystem).Caption, (Get-CimInstance Win32_OperatingSystem).OSArchitecture)

# El PATH de esta sesión puede no incluir instalaciones recientes en espacio
# de usuario; se añaden las ubicaciones conocidas antes de comprobar.
$userPaths = @(
    "$env:USERPROFILE\.cargo\bin",
    "$env:APPDATA\npm",
    "$env:LOCALAPPDATA\Microsoft\WinGet\Links"
)
$winlibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter 'BrechtSanders.WinLibs*' -ErrorAction SilentlyContinue |
    ForEach-Object { Join-Path $_.FullName 'mingw64\bin' } | Where-Object { Test-Path $_ }
$env:PATH = (($userPaths + $winlibs) -join ';') + ';' + $env:PATH

Write-Host "`nObligatorias:"
$null = Test-Tool -Name 'Git'    -Command 'git'    -Required
$null = Test-Tool -Name 'Rustup' -Command 'rustup' -Required
$null = Test-Tool -Name 'Cargo'  -Command 'cargo'  -Required
$null = Test-Tool -Name 'Rustc'  -Command 'rustc'  -Required
$null = Test-Tool -Name 'Node'   -Command 'node'   -Required
$null = Test-Tool -Name 'pnpm'   -Command 'pnpm'   -Required

Write-Host "`nToolchain C (hace falta MSVC o MinGW-w64):"
$hasMsvc = Test-Tool -Name 'MSVC (cl)' -Command 'cl' -VersionArgs '/?'
$hasGcc  = Test-Tool -Name 'GCC (MinGW-w64)' -Command 'gcc'
if (-not ($hasMsvc -or $hasGcc)) { $missing += 'Toolchain C (MSVC o GCC)' }

Write-Host "`nOpcionales:"
$null = Test-Tool -Name 'GitHub CLI'  -Command 'gh'
$null = Test-Tool -Name 'sqlite3'     -Command 'sqlite3'
$null = Test-Tool -Name 'cargo-audit' -Command 'cargo-audit'
$null = Test-Tool -Name 'cargo-deny'  -Command 'cargo-deny'

if ($missing.Count -gt 0) {
    Write-Host "`nFaltan herramientas obligatorias: $($missing -join ', ')" -ForegroundColor Yellow
    Write-Host "Ejecuta scripts/install-dev-tools.ps1 para instalarlas."
    exit 1
}
Write-Host "`nEntorno completo." -ForegroundColor Green
exit 0
