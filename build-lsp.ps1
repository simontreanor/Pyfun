#!/usr/bin/env pwsh
# Rebuild the Pyfun compiler and refresh the binary the VS Code language server runs.
#
# `pyfun.server.path` (see .vscode/settings.json) points VS Code at
# target/debug/pyfun.exe. The running language server holds that file open, so a
# plain `cargo build` cannot relink it ("Access is denied"). This script builds
# into the gitignored secondary `target-test` dir (no lock contention), then
# hot-swaps the fresh exe into target/debug, briefly stopping the server to release
# the lock. VS Code's language client then auto-restarts the server from the
# refreshed binary, so diagnostics pick up the new compiler.

$ErrorActionPreference = 'Stop'
Set-Location -Path $PSScriptRoot

Write-Host '==> Building into ./target-test (no lock contention) ...'
cargo build --target-dir target-test
if ($LASTEXITCODE -ne 0) { Write-Error 'cargo build failed'; exit 1 }

$fresh = Join-Path $PSScriptRoot 'target-test\debug\pyfun.exe'
$dest = Join-Path $PSScriptRoot 'target\debug\pyfun.exe'
New-Item -ItemType Directory -Force -Path (Split-Path $dest) | Out-Null

Write-Host '==> Swapping the fresh binary into target/debug ...'
# Try the copy first (0 kills if the server is already down / the file is free).
# Only when it is locked do we stop the server, then copy rapidly in the brief
# window before VS Code respawns it. Kept to <=3 kills so VS Code's crash-restart
# limit is not tripped (which would leave the server down until a manual reload).
$swapped = $false
for ($round = 0; $round -lt 3 -and -not $swapped; $round++) {
    try { Copy-Item -Force -Path $fresh -Destination $dest; $swapped = $true; break } catch { }
    Get-Process pyfun -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    for ($try = 0; $try -lt 15; $try++) {
        Start-Sleep -Milliseconds 50
        try { Copy-Item -Force -Path $fresh -Destination $dest; $swapped = $true; break } catch { }
    }
}
if (-not $swapped) {
    Write-Error 'Could not replace target/debug/pyfun.exe (the server kept re-locking it).'
    exit 1
}

Write-Host '==> Done. The language server will restart from the refreshed binary.'
Write-Host '    If diagnostics do not refresh, run "Developer: Reload Window".'
