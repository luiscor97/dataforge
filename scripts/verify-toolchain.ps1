<#
.SYNOPSIS
    Verifica que el toolchain de DataForge funciona de verdad.
.DESCRIPTION
    Además de comprobar versiones, ejecuta pruebas mínimas reales:
      1. compila y ejecuta un hola-mundo Rust (compilador + enlazador C);
      2. comprueba node/pnpm ejecutando código;
    Devuelve 0 solo si todo funciona. Idempotente; usa un directorio
    temporal que se elimina al final.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$failures = @()

# PATH de sesión: añade ubicaciones de usuario conocidas.
$userPaths = @("$env:USERPROFILE\.cargo\bin", "$env:APPDATA\npm", "$env:LOCALAPPDATA\Microsoft\WinGet\Links")
$winlibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter 'BrechtSanders.WinLibs*' -ErrorAction SilentlyContinue |
    ForEach-Object { Join-Path $_.FullName 'mingw64\bin' } | Where-Object { Test-Path $_ }
$env:PATH = (($userPaths + $winlibs) -join ';') + ';' + $env:PATH

Write-Host "== DataForge :: verificación de toolchain =="

Write-Host "`nVersiones:"
foreach ($pair in @(
        @('git', '--version'), @('rustup', '--version'), @('rustc', '--version'),
        @('cargo', '--version'), @('node', '--version'), @('pnpm', '--version'))) {
    $name = $pair[0]
    try {
        $v = & $name $pair[1] 2>$null | Select-Object -First 1
        Write-Host ("  {0,-8} {1}" -f $name, $v)
    } catch {
        Write-Host ("  {0,-8} NO DISPONIBLE" -f $name) -ForegroundColor Red
        $failures += $name
    }
}
Write-Host ("  {0,-8} {1}" -f 'triple', (rustup show active-toolchain 2>$null | Select-Object -First 1))

# --- Prueba real 1: compilar y ejecutar Rust --------------------------------
$work = Join-Path $env:TEMP ("dataforge-verify-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    Set-Content -Path (Join-Path $work 'hello.rs') -Encoding utf8 -Value 'fn main() { println!("dataforge-toolchain-ok"); }'
    Push-Location $work
    rustc hello.rs -o hello.exe 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n  [FALLO] rustc no pudo compilar (¿falta el enlazador C?)" -ForegroundColor Red
        $failures += 'rustc-link'
    } else {
        $out = & .\hello.exe
        if ($out -eq 'dataforge-toolchain-ok') {
            Write-Host "`n  [OK] rustc compila y enlaza ejecutables"
        } else {
            $failures += 'rustc-run'
        }
    }
    Pop-Location
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

# --- Prueba real 2: node y pnpm ejecutan ------------------------------------
$nodeOut = node -e "console.log('node-ok')" 2>$null
if ($nodeOut -ne 'node-ok') { $failures += 'node-exec' } else { Write-Host "  [OK] node ejecuta código" }
$pnpmOut = pnpm --version 2>$null
if (-not $pnpmOut) { $failures += 'pnpm-exec' } else { Write-Host "  [OK] pnpm responde ($pnpmOut)" }

# --- Aviso OneDrive ----------------------------------------------------------
$repoRoot = Split-Path -Parent $PSScriptRoot
if ($repoRoot -like "*OneDrive*") {
    Write-Host "`nAviso: el repositorio está dentro de OneDrive. Para builds más rápidas y sin conflictos de sincronización, exporta:" -ForegroundColor Yellow
    Write-Host "  `$env:CARGO_TARGET_DIR = `"$env:LOCALAPPDATA\dataforge-target`""
}

if ($failures.Count -gt 0) {
    Write-Host "`nVerificación FALLIDA: $($failures -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "`nToolchain verificado correctamente." -ForegroundColor Green
exit 0
