# Publish both portable variants to a GitHub Release.
#
# Assets (produced by tools\build-portable.bat):
#   dist-portable\Marionette-<ver>-windows-x64.exe
#   dist-portable\Marionette-Engine-<ver>-windows-x64.exe
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

$dist = Join-Path $root 'dist-portable'
$plain = Join-Path $dist "Marionette-$version-windows-x64.exe"
$engine = Join-Path $dist "Marionette-Engine-$version-windows-x64.exe"

foreach ($path in @($plain, $engine)) {
    if (-not (Test-Path $path)) {
        Write-Error "missing $path`nRun tools\build-portable.bat first (default builds both variants)."
        exit 1
    }
}

function Get-Sha256([string]$path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
}

$plainBytes = (Get-Item $plain).Length
$engineBytes = (Get-Item $engine).Length
$plainSha = Get-Sha256 $plain
$engineSha = Get-Sha256 $engine

Write-Host "publishing $tag"
Write-Host "  plain  $([math]::Round($plainBytes / 1MB, 2)) MB  $plainSha"
Write-Host "  engine $([math]::Round($engineBytes / 1MB, 2)) MB  $engineSha"

$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) { Write-Error 'gh not on PATH — install GitHub CLI'; exit 1 }

$existing = & gh release view $tag --json tagName 2>$null
$hasRelease = $LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($existing)

if ($hasRelease) {
    Write-Host "release $tag already exists — uploading assets (--clobber)"
    & gh release upload $tag $plain $engine --clobber
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} else {
    $args = @(
        'release', 'create', $tag,
        $plain, $engine,
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

两种便携版，功能相同，差在引擎怎么带：

| | 普通版 | 内置引擎版 |
|---|---|---|
| 文件 | Marionette-$version-windows-x64.exe | Marionette-Engine-$version-windows-x64.exe |
| 体积 | $([math]::Round($plainBytes / 1MB, 1)) MB | $([math]::Round($engineBytes / 1MB, 1)) MB |
| 引擎 | 第一次用再下载 | 打进 exe，离线也能开跨用 |
| SHA-256 | ``$plainSha`` | ``$engineSha`` |

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
