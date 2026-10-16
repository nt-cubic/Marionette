# Import the magpie engine source into third_party/magpie (build-only subset).
#
# Vendoring model (product decision 2026-10-16): Marionette ships the engine
# INSIDE the exe (compressed). This importer snapshots the build-relevant
# subset of https://github.com/yetone/magpie (MIT) at a pinned tag; updating
# the built-in engine = re-run with a newer tag + rebuild the portable exe.
#
# Kept: main.go, go.mod, go.sum, internal/, plugins/, LICENSE, README.md.
# Dropped: site/ (marketing, ~59MB), build/ (per-OS packaging assets),
#          .github/, docs/ — none of them are needed by the nogui build.
#
# Usage:  powershell -File tools\import-magpie.ps1 [-Tag v0.1.1274]
param(
    [string]$Tag = 'v0.1.1274'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $repoRoot 'third_party\magpie'
$tmp = Join-Path $env:TEMP "marionette-import-magpie-$Tag"

if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
Write-Output "[import] shallow clone $Tag ..."
# cmd /c: PS 5.1 turns git's stderr progress into a terminating error under Stop.
cmd /c "git clone --depth 1 --branch $Tag https://github.com/yetone/magpie.git `"$tmp`" 2>&1" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "git clone failed for tag $Tag" }

if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
New-Item -ItemType Directory -Path $dest | Out-Null

foreach ($item in @('main.go', 'go.mod', 'go.sum', 'internal', 'plugins', 'LICENSE', 'README.md')) {
    $src = Join-Path $tmp $item
    if (-not (Test-Path $src)) { throw "upstream is missing expected item: $item" }
    Copy-Item -Recurse -Force $src $dest
}

# Version stamp consumed by the portable build (engine-embedded/VERSION).
[IO.File]::WriteAllText((Join-Path $dest 'VERSION'), "$Tag`n")

@(
    "Vendored from https://github.com/yetone/magpie (MIT) at tag $Tag on $(Get-Date -Format yyyy-MM-dd).",
    "Imported by tools\import-magpie.ps1 — regenerate this folder with a newer tag to update the built-in engine.",
    "Subset only: build inputs for the nogui CLI. Upstream site/, build/, .github/, docs/ are intentionally absent.",
    "License: LICENSE (MIT) in this folder."
) | Set-Content (Join-Path $dest 'MARIONETTE-VENDOR.md')

Remove-Item -Recurse -Force $tmp
$size = (Get-ChildItem $dest -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
Write-Output ("[import] done: {0}  ({1:N1} MB)" -f $dest, $size)
