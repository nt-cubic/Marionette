# Import the magpie engine source into third_party/magpie (build-only subset).
#
# Vendoring model (product decision 2026-10-16, updated 2026-10): Marionette ships
# the engine INSIDE the exe (compressed) as a factory seed. This importer snapshots
# the build-relevant subset of https://github.com/yetone/magpie (MIT) at a tag;
# updating the baked-in seed = re-run + tools\build-portable.bat.
#
# Default Tag is the latest *stable* GitHub release (non-prerelease). Pin with
# -Tag v0.1.xxxx or env MAGPIE_TAG. build-portable.bat calls this unless
# SKIP_MAGPIE_IMPORT=1.
#
# Kept: main.go, go.mod, go.sum, internal/, plugins/, LICENSE, README.md,
#       THIRD_PARTY_NOTICES (embedded by main.go; optional so old tags still pin).
# Dropped: site/ (marketing, ~59MB), build/ (per-OS packaging assets),
#          .github/, docs/ — none of them are needed by the nogui build.
#
# Usage:
#   powershell -File tools\import-magpie.ps1
#   powershell -File tools\import-magpie.ps1 -Tag v0.1.1319
param(
    [string]$Tag = ''
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $repoRoot 'third_party\magpie'

if (-not $Tag) {
    if ($env:MAGPIE_TAG -and $env:MAGPIE_TAG.Trim()) {
        $Tag = $env:MAGPIE_TAG.Trim()
    }
}

if (-not $Tag) {
    Write-Output "[import] resolving latest stable release from yetone/magpie ..."
    $headers = @{
        'Accept'     = 'application/vnd.github+json'
        'User-Agent' = 'Marionette-import-magpie'
    }
    try {
        $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/yetone/magpie/releases/latest' -Headers $headers
    } catch {
        throw "failed to query GitHub releases/latest: $_"
    }
    $Tag = [string]$release.tag_name
    if (-not $Tag) { throw 'GitHub releases/latest returned no tag_name' }
    Write-Output "[import] latest stable: $Tag"
}

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

# THIRD_PARTY_NOTICES: main.go carries it with `//go:embed` (upstream >= the
# release that added in-binary third-party notices, v0.1.18xx). Without it the
# nogui build dies with `pattern THIRD_PARTY_NOTICES: no matching files found`.
# Optional on purpose: older tags have neither the file nor the embed directive,
# so pinning one must keep working.
$notices = Join-Path $tmp 'THIRD_PARTY_NOTICES'
if (Test-Path $notices) {
    Copy-Item -Force $notices $dest
} else {
    Write-Output "[import] note: $Tag has no THIRD_PARTY_NOTICES (older upstream does not embed it)"
}

# Version stamp consumed by the portable build (engine-embedded/VERSION).
[IO.File]::WriteAllText((Join-Path $dest 'VERSION'), "$Tag`n")

# ── Marionette engine seed patch ────────────────────────────────────────────
# The shipped exe embeds a SLIM gateway-only magpie (see tools\magpie-seed\
# main_seed.go): stock `internal/cli.Main` keeps the whole multi-agent manager
# reachable from main (58MB raw / 19.6MB UPX at v0.1.1877); the seed main
# serves only the model-share gateway (~38MB / ~12MB UPX). This script wipes
# third_party\magpie on every import, so the patch is re-applied here:
#   1. main_seed.go copied in (build tag `marionette_seed`)
#   2. stock main.go gets the matching `!marionette_seed` guard
# build-portable.bat compiles with `-tags "nogui marionette_seed"`. Upstream
# refactors of the serve path surface as compile errors in main_seed.go —
# loud, never silent.
$seedSrc = Join-Path $PSScriptRoot 'magpie-seed\main_seed.go'
if (-not (Test-Path $seedSrc)) { throw 'tools\magpie-seed\main_seed.go missing — the seed patch cannot be applied' }
Copy-Item -Force $seedSrc (Join-Path $dest 'main_seed.go')
$mainGo = Join-Path $dest 'main.go'
$mainText = [IO.File]::ReadAllText($mainGo)
if ($mainText -notmatch '(?m)^//go:build !marionette_seed') {
    [IO.File]::WriteAllText($mainGo, "//go:build !marionette_seed`r`n`r`n" + $mainText)
    Write-Output '[import] seed patch applied: !marionette_seed guard on main.go + main_seed.go'
} else {
    Write-Output '[import] seed patch: main.go already carries the guard (upstream merged it?)'
}

@(
    "Vendored from https://github.com/yetone/magpie (MIT) at tag $Tag on $(Get-Date -Format yyyy-MM-dd).",
    "Imported by tools\import-magpie.ps1 — default Tag is GitHub latest stable; pass -Tag or MAGPIE_TAG to pin.",
    "Subset only: build inputs for the nogui CLI. Upstream site/, build/, .github/, docs/ are intentionally absent.",
    "Marionette patch: main_seed.go (build tag marionette_seed) is the slim gateway-only main; import re-applies it. See tools\magpie-seed\main_seed.go.",
    "License: LICENSE (MIT) in this folder."
) | Set-Content (Join-Path $dest 'MARIONETTE-VENDOR.md')

Remove-Item -Recurse -Force $tmp
$size = (Get-ChildItem $dest -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
Write-Output ("[import] done: {0}  ({1:N1} MB)  tag={2}" -f $dest, $size, $Tag)
