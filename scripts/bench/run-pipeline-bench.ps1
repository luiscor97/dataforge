# DataForge M1.0.1 — reproducible pipeline benchmark driver (Windows).
#
# Generates a deterministic corpus (df-corpus profile), runs the full
# pipeline phase by phase with the release CLI, and records per-phase wall
# time, peak working set, CPU time and derived throughput into a JSON file
# plus a Markdown row block under docs/performance/data/.
#
# Usage (from the repository root):
#   powershell -File scripts/bench/run-pipeline-bench.ps1 `
#     -Profile a-small [-Files 100000] [-Seed 42] [-Label baseline] `
#     [-Root D:\df-bench] [-SkipBuild] [-KeepCorpus]
#
# The benchmark is local-only: no telemetry, no network. Every recorded row
# carries commit, profile, files, seed and configuration so any result can
# be reproduced exactly.

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("a-small", "b-mixed", "c-large", "d-million")]
    [string]$Profile,
    [long]$Files = 0,
    [long]$Seed = 42,
    [string]$Label = "baseline",
    [string]$Root = "$env:USERPROFILE\Desktop\dataforge-bench",
    [switch]$SkipBuild,
    [switch]$KeepCorpus
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$cli = Join-Path $repo "target\release\dataforge.exe"
$corpusTool = Join-Path $repo "target\release\df-corpus.exe"

if (-not $SkipBuild) {
    Write-Host "== build (release, locked) =="
    cargo build --release --locked -p dataforge-cli -p df-corpus
    if ($LASTEXITCODE -ne 0) { throw "release build failed" }
}
foreach ($bin in @($cli, $corpusTool)) {
    if (-not (Test-Path $bin)) { throw "missing binary: $bin (run without -SkipBuild)" }
}

$commit = (git -C $repo rev-parse --short HEAD).Trim()
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$caseName = "$Label-$Profile-f$Files-s$Seed-$commit-$stamp"
$caseRoot = Join-Path $Root $caseName
$corpusDir = Join-Path $caseRoot "corpus"
$projectDir = Join-Path $caseRoot "project"
$outputDir = Join-Path $caseRoot "output"
$dataDir = Join-Path $repo "docs\performance\data"
New-Item -ItemType Directory -Force $caseRoot, $dataDir | Out-Null

# --- helper: run one phase, sampling peak memory and CPU every 250 ms ----
function Invoke-Phase {
    param([string]$Name, [string[]]$CliArgs)
    $stdout = Join-Path $caseRoot "$Name.stdout.json"
    $stderr = Join-Path $caseRoot "$Name.stderr.txt"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $cli -ArgumentList $CliArgs -NoNewWindow -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    # Touch the handle so the Process object caches it; without this, ExitCode
    # reads back null after the process exits when it was started this way.
    $null = $p.Handle
    $peakWs = 0; $cpuSeconds = 0.0
    while (-not $p.HasExited) {
        try {
            $p.Refresh()
            if ($p.PeakWorkingSet64 -gt $peakWs) { $peakWs = $p.PeakWorkingSet64 }
            $cpuSeconds = $p.TotalProcessorTime.TotalSeconds
        } catch {}
        Start-Sleep -Milliseconds 250
    }
    $p.WaitForExit()
    $sw.Stop()
    $exit = $p.ExitCode
    Write-Host ("  {0,-10} {1,10:N1}s  exit={2}  peakWS={3:N0} MB" -f `
        $Name, $sw.Elapsed.TotalSeconds, $exit, ($peakWs / 1MB))
    [pscustomobject]@{
        phase        = $Name
        seconds      = [math]::Round($sw.Elapsed.TotalSeconds, 3)
        exit_code    = $exit
        peak_ws_mb   = [math]::Round($peakWs / 1MB, 1)
        cpu_seconds  = [math]::Round($cpuSeconds, 1)
        stdout_file  = $stdout
    }
}

# --- corpus ---------------------------------------------------------------
Write-Host "== corpus ($Profile, files=$Files, seed=$Seed) =="
$corpusArgs = @("--output", $corpusDir, "--profile", $Profile, "--seed", $Seed)
if ($Files -gt 0) { $corpusArgs += @("--files", $Files) }
$swCorpus = [System.Diagnostics.Stopwatch]::StartNew()
& $corpusTool @corpusArgs | Tee-Object -FilePath (Join-Path $caseRoot "corpus.txt")
if ($LASTEXITCODE -ne 0) { throw "corpus generation failed" }
$swCorpus.Stop()
$corpusInfo = Get-Content (Join-Path $caseRoot "corpus.txt")
$corpusFiles = [long](($corpusInfo | Select-String "^Files\s*:\s*(\d+)").Matches.Groups[1].Value)
$corpusBytes = [long](($corpusInfo | Select-String "^Bytes\s*:\s*(\d+)").Matches.Groups[1].Value)

# --- pipeline -------------------------------------------------------------
Write-Host "== pipeline =="
$phases = @()
$phases += Invoke-Phase "create" @("project", "create", "--name", "bench-$Profile",
    "--path", $projectDir, "--output-root", $outputDir,
    "--profile", "generic", "--source", $corpusDir, "--json")
$phases += Invoke-Phase "scan" @("scan", "--path", $projectDir, "--json")
$phases += Invoke-Phase "hash" @("hash", "--path", $projectDir, "--json")
$phases += Invoke-Phase "analyze" @("analyze", "--path", $projectDir, "--json")
$phases += Invoke-Phase "plan" @("plan", "create", "--path", $projectDir,
    "--duplicate-policy", "REPORT_ONLY", "--json")
$phases += Invoke-Phase "approve" @("plan", "approve", "--path", $projectDir, "--json")
$phases += Invoke-Phase "execute" @("execute", "--path", $projectDir, "--json")
$phases += Invoke-Phase "verify" @("verify", "--path", $projectDir, "--json")

foreach ($ph in $phases) {
    if ($ph.exit_code -ne 0) {
        Write-Warning "phase $($ph.phase) exited $($ph.exit_code) — see $($ph.stdout_file)"
    }
}

# --- derived throughput ----------------------------------------------------
$avgFile = if ($corpusFiles -gt 0) { [math]::Round($corpusBytes / $corpusFiles) } else { 0 }
foreach ($ph in $phases) {
    $fps = if ($ph.seconds -gt 0 -and $corpusFiles -gt 0 -and $ph.phase -in @("scan", "hash", "execute", "verify")) {
        [math]::Round($corpusFiles / $ph.seconds, 1)
    } else { $null }
    $mibps = if ($ph.seconds -gt 0 -and $corpusBytes -gt 0 -and $ph.phase -in @("hash", "execute", "verify")) {
        [math]::Round(($corpusBytes / 1MB) / $ph.seconds, 1)
    } else { $null }
    $ph | Add-Member files_per_second $fps
    $ph | Add-Member mib_per_second $mibps
}

# --- record ----------------------------------------------------------------
$vol = Get-Volume -FilePath $Root -ErrorAction SilentlyContinue
$result = [pscustomobject]@{
    label            = $Label
    profile          = $Profile
    files            = $corpusFiles
    bytes            = $corpusBytes
    avg_file_bytes   = $avgFile
    seed             = $Seed
    commit           = $commit
    build            = "release --locked"
    hardware         = (Get-CimInstance Win32_Processor).Name
    os               = (Get-CimInstance Win32_OperatingSystem).Caption
    filesystem       = if ($vol) { $vol.FileSystemType } else { "unknown" }
    corpus_seconds   = [math]::Round($swCorpus.Elapsed.TotalSeconds, 1)
    generated_at_utc = (Get-Date).ToUniversalTime().ToString("s") + "Z"
    phases           = $phases | Select-Object phase, seconds, exit_code, peak_ws_mb,
                        cpu_seconds, files_per_second, mib_per_second
}
$jsonPath = Join-Path $dataDir "$caseName.json"
# .NET WriteAllText emits UTF-8 without a BOM; Set-Content -Encoding utf8 on
# PowerShell 5.1 prepends one and breaks strict JSON parsers.
[System.IO.File]::WriteAllText($jsonPath, ($result | ConvertTo-Json -Depth 5))
Write-Host "== resultado: $jsonPath =="
$result.phases | Format-Table -AutoSize | Out-String | Write-Host

# --- cleanup ----------------------------------------------------------------
if (-not $KeepCorpus) {
    Write-Host "== limpiando corpus y salida (use -KeepCorpus para conservarlos) =="
    Remove-Item -Recurse -Force $corpusDir, $outputDir, $projectDir -ErrorAction SilentlyContinue
}
