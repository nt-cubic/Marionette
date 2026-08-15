$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot

$pkg = Join-Path $root 'package.json'
if (-not (Test-Path $pkg)) { Write-Error "missing $pkg"; exit 1 }
$json = [System.IO.File]::ReadAllText($pkg)
if ($json -notmatch '"version"\s*:\s*"([^"]+)"') {
    Write-Error 'cannot find "version" in package.json'
    exit 1
}
$oldVersion = $Matches[1]

if ($args.Count -ge 1 -and -not [string]::IsNullOrWhiteSpace($args[0])) {
    $newVersion = $args[0].Trim()
} else {
    $newVersion = Read-Host "current version is $oldVersion. enter new version (e.g. 0.2.2)"
    if ([string]::IsNullOrWhiteSpace($newVersion)) {
        Write-Error 'no version entered'
        exit 1
    }
    $newVersion = $newVersion.Trim()
}

if ($newVersion.StartsWith('v') -or $newVersion.StartsWith('V')) {
    $newVersion = $newVersion.Substring(1)
}

if ($newVersion -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    Write-Error "invalid version: '$newVersion' (expected e.g. 0.2.2 or 0.2.2-beta.1)"
    exit 1
}

if ($oldVersion -eq $newVersion) {
    Write-Host "already at $newVersion - nothing to do"
    exit 0
}

Write-Host "bumping $oldVersion -> $newVersion"

$script:matchCount = 0
$script:changed = @()

function Update-File([string]$path, [string]$pattern, [string]$replacement, [int]$maxMatches = -1) {
    if (-not (Test-Path $path)) {
        Write-Host "  [skip] $path (missing)"
        return
    }
    $content = [System.IO.File]::ReadAllText($path)
    $script:matchCount = 0
    $regex = [regex]::new($pattern, [System.Text.RegularExpressions.RegexOptions]::Multiline)
    $newContent = $regex.Replace($content, {
        param($m)
        $script:matchCount++
        if ($maxMatches -ge 0 -and $script:matchCount -gt $maxMatches) { return $m.Value }
        return $m.Result($replacement)
    })
    if ($newContent -ne $content) {
        [System.IO.File]::WriteAllText($path, $newContent)
        $script:changed += $path
        Write-Host "  [ok] $path"
    } else {
        Write-Host "  [warn] $path (no change)"
    }
}

$escaped = [regex]::Escape($oldVersion)

Update-File (Join-Path $root 'package.json') `
    ('"version"\s*:\s*"' + $escaped + '"') `
    ('"version": "' + $newVersion + '"') 1

Update-File (Join-Path $root 'package-lock.json') `
    ('"version"\s*:\s*"' + $escaped + '"') `
    ('"version": "' + $newVersion + '"') 2

Update-File (Join-Path $root 'src-tauri\tauri.conf.json') `
    ('"version"\s*:\s*"' + $escaped + '"') `
    ('"version": "' + $newVersion + '"') 1

Update-File (Join-Path $root 'src-tauri\Cargo.toml') `
    ('^version\s*=\s*"' + $escaped + '"$') `
    ('version = "' + $newVersion + '"') 1

Update-File (Join-Path $root 'src-tauri\Cargo.lock') `
    ('(?m)(^name\s*=\s*"marionette"\r?\n^version\s*=\s*")' + $escaped + '(")') `
    ('${1}' + $newVersion + '${2}') 1

if ($script:changed.Count -eq 0) {
    Write-Host "no files changed - old version '$oldVersion' not found?"
    exit 1
}

Write-Host
Write-Host "done - $($script:changed.Count) file(s) updated to $newVersion"
