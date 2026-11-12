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
#       THIRD_PARTY_NOTICES (embedded by main.go; optional so old tags still pin),
#       third_party/ (upstream's own local go.mod replacements; optional, see below).
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

# Temp cleanup must never fail the build. A stale clone (a git child still
# finishing checkout) or the AV scanner holding a 100MB pack/index.lock makes
# Remove-Item throw; the directory is disposable and every run gets a fresh
# unique name, so retry a few times and then just walk away.
function Remove-TempBestEffort {
    param([string]$Path, [int]$Tries = 5)
    if (-not (Test-Path $Path)) { return }
    for ($i = 1; $i -le $Tries; $i++) {
        try { Remove-Item -Recurse -Force $Path -ErrorAction Stop; return }
        catch { if ($i -lt $Tries) { Start-Sleep -Milliseconds (200 * $i) } }
    }
    Write-Output "[import] warn: could not clean $Path (still locked); leaving it for a later run"
}

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

# Unique temp workspace per run, deliberately: a previous aborted clone (or the
# AV scanner on its pack) can hold `index.lock`/`tmp_pack_*` open for minutes,
# and a shared fixed path turned that into "build fails before it starts"
# (2026-11-12). Nothing here needs a stable path, so never reuse one.
# Sweep corpses of earlier runs (best effort, same prefix).
Get-ChildItem $env:TEMP -Directory -Filter 'marionette-magpie-*' -ErrorAction SilentlyContinue |
    ForEach-Object { Remove-TempBestEffort $_.FullName }
Get-ChildItem $env:TEMP -Directory -Filter 'marionette-import-magpie-*' -ErrorAction SilentlyContinue |
    ForEach-Object { Remove-TempBestEffort $_.FullName }

# The name must stay SHORT. Upstream ships a 181-char test fixture path
# (internal/gateway/testdata/qoder-*/…/agent-*.jsonl); TEMP base is ~34 chars,
# so a verbose per-run suffix pushes the checkout past Windows MAX_PATH and
# git dies with "Filename too long" (2026-11-12 — the untagged release bump to
# v0.1.3833 added that fixture). 8 hex chars is plenty to dodge collisions.
$tmp = Join-Path $env:TEMP ("marionette-magpie-" + [guid]::NewGuid().ToString('N').Substring(0, 8))

Write-Output "[import] shallow clone $Tag ..."
# cmd /c: PS 5.1 turns git's stderr progress into a terminating error under Stop.
# Keep git's own output: on failure it is the only thing that says why (unknown
# tag, path too long, network) — swallowing it turned a 5-second diagnosis into
# guesswork.
$cloneOut = cmd /c "git -c core.longpaths=true clone --depth 1 --branch $Tag https://github.com/yetone/magpie.git `"$tmp`" 2>&1"
if ($LASTEXITCODE -ne 0) {
    $cloneOut | Where-Object { $_ -and ($_ -notmatch 'Updating files:') } | Select-Object -Last 12 |
        ForEach-Object { Write-Output "  git: $_" }
    Remove-TempBestEffort $tmp
    throw "git clone failed for tag $Tag"
}

if (Test-Path $dest) {
    try {
        Remove-Item -Recurse -Force $dest -ErrorAction Stop
    } catch {
        # Unlike the temp dir this one MUST be wiped (stale files would mix with
        # the new tag), so fail — but say what to do about it.
        throw "cannot wipe $dest before import (a running build/dev instance or the AV scanner is holding it): $($_.Exception.Message)"
    }
}
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

# Upstream vendors patched dependencies under its own third_party/ and points at
# them from go.mod (`replace ... => ./third_party/jsonschema`, added in
# v0.1.37xx). Missing that folder kills the Go build with
# "replacement directory ./third_party/jsonschema does not exist", long before
# anything Marionette-side runs — so carry it whenever the tag has one.
# Optional on purpose: older tags have no third_party/ at all and must keep pinning.
$upstreamThirdParty = Join-Path $tmp 'third_party'
if (Test-Path $upstreamThirdParty) {
    Copy-Item -Recurse -Force $upstreamThirdParty $dest
    $tpNames = (Get-ChildItem $upstreamThirdParty -Name) -join ', '
    Write-Output "[import] upstream third_party/ vendored (local go.mod replacements): $tpNames"
} else {
    Write-Output "[import] note: $Tag has no upstream third_party/ (no local go.mod replacements)"
}

# Guard: every local path replacement in go.mod must resolve inside the subset we
# copied. This is what caught the v0.1.3727 break — fail here with a clear message
# instead of a bare Go error ten minutes into the portable build.
$goModText = [IO.File]::ReadAllText((Join-Path $dest 'go.mod'))
$localReplaces = @([regex]::Matches($goModText, '(?m)^replace\s+(\S+)\s*=>\s*(\.\.?[\\/][^\s]*)') |
    ForEach-Object { [pscustomobject]@{ Module = $_.Groups[1].Value; Path = $_.Groups[2].Value } })
foreach ($r in $localReplaces) {
    $resolved = Join-Path $dest $r.Path
    if (-not (Test-Path $resolved)) {
        throw "go.mod replaces $($r.Module) with local dir '$($r.Path)' but it was not imported — add that upstream folder to the copy list in tools\import-magpie.ps1"
    }
}
if ($localReplaces.Count -gt 0) {
    Write-Output ("[import] go.mod local replacements OK: {0}" -f (($localReplaces | ForEach-Object { $_.Path }) -join ', '))
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

# Codex ACP encodes effort as model[effort] (gpt-5.6-luna[xhigh]). Stock magpie
# GroupFor only peels Claude Code's [1m] window mark, so a share alias saved
# under the bare builtin name 404s. Replay the helper + swap the stock peels.
$providerDir = Join-Path $dest 'internal\provider'
$peelSrc = Join-Path $PSScriptRoot 'magpie-seed\marionette_peel.go'
$peelTest = Join-Path $PSScriptRoot 'magpie-seed\marionette_peel_test.go'
if (-not (Test-Path $peelSrc)) { throw 'tools\magpie-seed\marionette_peel.go missing — Codex effort peel cannot be applied' }
Copy-Item -Force $peelSrc (Join-Path $providerDir 'marionette_peel.go')
if (Test-Path $peelTest) {
    Copy-Item -Force $peelTest (Join-Path $providerDir 'marionette_peel_test.go')
}
Get-ChildItem $providerDir -Filter *.go | ForEach-Object {
    if ($_.Name -eq 'marionette_peel.go' -or $_.Name -eq 'marionette_peel_test.go') { return }
    $text = [IO.File]::ReadAllText($_.FullName)
    $next = $text.Replace('strings.TrimSuffix(strings.TrimSpace(id), "[1m]")', 'peelAgentModelMark(id)')
    $next = $next.Replace('strings.TrimSuffix(strings.TrimSpace(ref), "[1m]")', 'peelAgentModelMark(ref)')
    if ($next -ne $text) {
        [IO.File]::WriteAllText($_.FullName, $next)
        Write-Output ("[import] seed patch: Codex [effort] peel in {0}" -f $_.Name)
    }
}

@(
    "Vendored from https://github.com/yetone/magpie (MIT) at tag $Tag on $(Get-Date -Format yyyy-MM-dd).",
    "Imported by tools\import-magpie.ps1 — default Tag is GitHub latest stable; pass -Tag or MAGPIE_TAG to pin.",
    "Subset only: build inputs for the nogui CLI, plus upstream third_party/ when the tag ships local go.mod replacements. Upstream site/, build/, .github/, docs/ are intentionally absent.",
    "Marionette patch: main_seed.go (build tag marionette_seed) is the slim gateway-only main; import re-applies it. See tools\magpie-seed\main_seed.go.",
    "Marionette patch: marionette_peel.go peels Codex [effort] (and Claude [1m]) in GroupFor so share aliases match. See tools\magpie-seed\marionette_peel.go.",
    "License: LICENSE (MIT) in this folder."
) | Set-Content (Join-Path $dest 'MARIONETTE-VENDOR.md')

Remove-TempBestEffort $tmp
$size = (Get-ChildItem $dest -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
Write-Output ("[import] done: {0}  ({1:N1} MB)  tag={2}" -f $dest, $size, $Tag)
