# Publish the portable exe to a GitHub Release.
#
# Asset (produced by tools\build-portable.bat):
#   release\<version>\Marionette.exe
#
# Usage:
#   tools\publish-release.bat
#   tools\publish-release.bat -NotesFile path\to\notes.md
#   tools\publish-release.bat -Draft
param(
    [string]$NotesFile = '',
    [switch]$Draft
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$pkg = Join-Path $root 'package.json'
if (-not (Test-Path $pkg)) { Write-Error "missing $pkg"; exit 1 }
$version = (Get-Content -Raw -LiteralPath $pkg | ConvertFrom-Json).version
if ([string]::IsNullOrWhiteSpace($version)) { Write-Error 'package.json has no version'; exit 1 }
if ($version.StartsWith('v') -or $version.StartsWith('V')) { $version = $version.Substring(1) }
$tag = "v$version"

$dist = Join-Path $root ('release\' + $version)
$asset = Join-Path $dist 'Marionette.exe'

if (-not (Test-Path $asset)) {
    Write-Error "missing $asset`nRun tools\build-portable.bat first."
    exit 1
}

function Get-Sha256([string]$path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
}

$bytes = (Get-Item $asset).Length
$sha = Get-Sha256 $asset

Write-Host "publishing $tag"
Write-Host "  Marionette.exe  $([math]::Round($bytes / 1MB, 2)) MB  $sha"

$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) { Write-Error 'gh not on PATH — install GitHub CLI'; exit 1 }

$ErrorActionPreference = 'Continue'
    $existing = & gh release view $tag --json tagName 2>$null
    $ErrorActionPreference = 'Stop'
$hasRelease = $LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($existing)

if ($hasRelease) {
    Write-Host "release $tag already exists — uploading asset (--clobber)"
    & gh release upload $tag $asset --clobber
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} else {
    $args = @(
        'release', 'create', $tag,
        $asset,
        '--title', "Marionette $tag",
        '--latest'
    )
    if ($Draft) { $args += '--draft' }
    if (-not [string]::IsNullOrWhiteSpace($NotesFile)) {
        if (-not (Test-Path $NotesFile)) { Write-Error "notes file missing: $NotesFile"; exit 1 }
        $args += @('--notes-file', $NotesFile)
    } else {
        $notes = @"
## Marionette $tag

便携版，内置跨用引擎。单文件，打开即用。

- 文件：Marionette.exe
- 体积：$([math]::Round($bytes / 1MB, 1)) MB
- SHA-256：``$sha``

需要 WebView2 Runtime。
"@
        $tmpNotes = Join-Path $env:TEMP "marionette-release-$version-notes.md"
        [System.IO.File]::WriteAllText($tmpNotes, $notes)
        $args += @('--notes-file', $tmpNotes)
    }
    Write-Host "`$ gh $($args -join ' ')"
    & gh @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host
Write-Host "done — https://github.com/nt-cubic/Marionette/releases/tag/$tag"
