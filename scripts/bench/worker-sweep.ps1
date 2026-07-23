# DataForge M1.0.1 — worker scaling sweep.
#
# Builds the release binaries once, then measures hash and verify at a range
# of worker counts so the scaling curves are comparable. Run on a QUIET
# machine (no concurrent cargo/builds) — background compilation contaminates
# wall-clock timings.
#
#   powershell -File scripts/bench/worker-sweep.ps1
#
# Results land in docs/performance/data/ as one JSON per point.

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$driver = Join-Path $repo "scripts\bench\run-pipeline-bench.ps1"

Write-Host "== build once (release, locked) =="
Push-Location $repo
cargo build --release --locked -p dataforge-cli -p df-corpus
if ($LASTEXITCODE -ne 0) { throw "release build failed" }
Pop-Location

# Hash scaling on small files (100k): create+scan+hash only, so execute (which
# is not parallel yet) is not paid for repeatedly.
foreach ($w in 1, 2, 4, 8, 16) {
    Write-Host "== hash sweep: workers=$w =="
    & $driver -Profile a-small -Label hashsweep -Workers $w -StopAfter hash -SkipBuild
}

# Verify (and hash) scaling on large files (50 x ~100MiB-2GiB): the full
# pipeline runs, but execute on 50 files is cheap, so verify/hash scaling is
# what the run measures.
foreach ($w in 1, 2, 4, 8) {
    Write-Host "== verify sweep (large): workers=$w =="
    & $driver -Profile c-large -Label vsweep -Workers $w -SkipBuild
}

Write-Host "== worker sweep done =="
