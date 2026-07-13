<#
.SYNOPSIS
    Instala los plugins/herramientas de desarrollo opcionales de DataForge.
.DESCRIPTION
    Complementa install-dev-tools.ps1 con utilidades de auditoría y calidad
    (ADR-0013). Idempotente: omite lo ya instalado. Todas las fuentes son
    registros oficiales (crates.io, winget).
      - cargo-audit  : auditoría de vulnerabilidades (RustSec)
      - cargo-deny   : licencias, fuentes y duplicados de dependencias
      - sqlite3 CLI  : inspección manual de bases de proyecto (winget)
    Nota: cargo install compila desde fuentes; puede tardar varios minutos.
#>
[CmdletBinding()]
param(
    [switch] $SkipCargoTools,
    [switch] $SkipSqliteCli
)

$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:LOCALAPPDATA\Microsoft\WinGet\Links;$env:PATH"

function Test-Cmd { param([string] $Name) [bool](Get-Command $Name -ErrorAction SilentlyContinue) }

if (-not (Test-Cmd 'cargo')) {
    Write-Host "cargo no encontrado; ejecuta antes scripts/install-dev-tools.ps1" -ForegroundColor Red
    exit 2
}

if (-not $SkipCargoTools) {
    foreach ($tool in @('cargo-audit', 'cargo-deny')) {
        if (Test-Cmd $tool) {
            Write-Host "$tool ya presente: $(& $tool --version 2>$null | Select-Object -First 1)"
        } else {
            Write-Host "Instalando $tool desde crates.io (compila desde fuentes)..."
            cargo install $tool --locked
            if ($LASTEXITCODE -ne 0) { Write-Host "Fallo instalando $tool" -ForegroundColor Red; exit 3 }
        }
    }
}

if (-not $SkipSqliteCli) {
    if (Test-Cmd 'sqlite3') {
        Write-Host "sqlite3 ya presente: $(sqlite3 --version 2>$null)"
    } else {
        Write-Host "Instalando sqlite3 CLI (winget SQLite.SQLite, portable)..."
        winget install --id SQLite.SQLite --silent --accept-package-agreements --accept-source-agreements --disable-interactivity
        if ($LASTEXITCODE -ne 0) {
            # Herramienta de conveniencia: no bloquea el desarrollo.
            Write-Host "No se pudo instalar sqlite3 (opcional); continúa sin él." -ForegroundColor Yellow
        }
    }
}

Write-Host "`nPlugins de desarrollo listos." -ForegroundColor Green
exit 0
