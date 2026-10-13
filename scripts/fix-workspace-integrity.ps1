# Repair workspace integrity labels (Marionette)
#
# Symptom this fixes
# ------------------
# Every executable inside the project tree starts at LOW integrity and can only
# write inside the project: no %USERPROFILE%\.marionette (instance lock, dev
# diary, sessions) and no %LOCALAPPDATA%\Temp (esbuild's service temp file).
# Marionette then exits silently with code 0 — no window, no log line — and
# `npm run build` fails with:
#     [vite:esbuild-transpile] remove C:\...\Temp\esbuild-<hex>: Access is denied.
#
# Cause
# -----
# A Windows sandbox (for example DSH's workspace-write confinement) marks the
# workspace with an inheritable Low mandatory integrity label. Windows derives a
# process's integrity level from the lowest of its token and the mandatory label
# on its image file, so every .exe in the tree inherits the restriction — long
# after the sandbox that created it is gone.
#
# What this script does
# ---------------------
# Sets the project directory's mandatory integrity label to Medium with (OI)(CI)
# inheritance, which propagates to existing files and to everything built later.
# It changes integrity labels only: no file contents, owners or access (DACL)
# entries are touched.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\fix-workspace-integrity.ps1 [-Path <dir>] [-Quiet]

[CmdletBinding()]
param(
    [string]$Path,
    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'

function Write-Status([string]$text) { if (-not $Quiet) { Write-Host $text } }

if (-not $Path) {
    # $PSScriptRoot is not always populated (Windows PowerShell 5.1 with -File).
    $scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
    $Path = Split-Path -Parent $scriptDir
}

if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
    throw "Not a directory: $Path"
}
$Path = (Resolve-Path -LiteralPath $Path).Path

function Get-IntegrityLabel([string]$target) {
    $match = icacls $target 2>$null | Select-String -Pattern 'Mandatory Label\\'
    if (-not $match) { return '(none)' }
    return ($match.Line.Trim() -replace '\s+', ' ')
}

Write-Status "Project: $Path"
Write-Status ("Root label before: {0}" -f (Get-IntegrityLabel $Path))

# Report the executables that were actually broken before touching anything.
$low = @()
Get-ChildItem -LiteralPath $Path -Recurse -File -Include *.exe -ErrorAction SilentlyContinue | ForEach-Object {
    $label = Get-IntegrityLabel $_.FullName
    if ($label -match 'Low Mandatory Level') { $low += $_.FullName }
}
Write-Status ("Executables at Low integrity: {0}" -f $low.Count)

Write-Status "Setting the label to Medium (inherited)..."
icacls $Path /setintegritylevel '(OI)(CI)Medium' | Out-Null

Write-Status ("Root label after:  {0}" -f (Get-IntegrityLabel $Path))

$stillLow = @()
Get-ChildItem -LiteralPath $Path -Recurse -File -Include *.exe -ErrorAction SilentlyContinue | ForEach-Object {
    $label = Get-IntegrityLabel $_.FullName
    if ($label -match 'Low Mandatory Level') { $stillLow += $_.FullName }
}

if ($stillLow.Count -gt 0) {
    Write-Status ("WARN: {0} executable(s) still carry an explicit Low label; relabel them individually:" -f $stillLow.Count)
    $stillLow | Select-Object -First 10 | ForEach-Object { Write-Status "  $_" }
    Write-Status "  icacls <file> /setintegritylevel Medium"
    exit 1
}

Write-Status "OK: every executable in the tree now runs at Medium integrity or higher."
Write-Status "Next: npm run build   /   build-portable.bat"
