param(
    [Parameter(Mandatory = $true)][string]$Target,
    [string]$ReleaseDir = '',
    [string]$ExpectedVersion = ''
)
$ErrorActionPreference = 'Stop'
if (-not $ReleaseDir) { $ReleaseDir = Join-Path (Split-Path -Parent $PSScriptRoot) 'release' }
$release = (Get-Item -LiteralPath $ReleaseDir).FullName.TrimEnd('\')
$exe = Get-Item -LiteralPath $Target
if ($exe.PSIsContainer -or $exe.Name -cne 'Marionette.exe' -or
    -not [string]::Equals($exe.Directory.Parent.FullName.TrimEnd('\'), $release, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Target must be release/<version>/Marionette.exe.'
}
if ($ExpectedVersion) {
    $actual = $exe.VersionInfo.ProductVersion
    if (-not $actual -or ($actual -split '[-+]')[0] -ne ($ExpectedVersion -split '[-+]')[0]) {
        throw "Build version mismatch: expected $ExpectedVersion; executable reports $actual."
    }
}
$pointer = Join-Path $release 'current-build.txt'
$temp = Join-Path $release ('.current-build-' + [guid]::NewGuid().ToString('N') + '.tmp')
$backup = $temp + '.bak'
try {
    [IO.File]::WriteAllText($temp, ($exe.Directory.Name + '\Marionette.exe' + "`n"), (New-Object Text.UTF8Encoding $false))
    if (Test-Path -LiteralPath $pointer) { [IO.File]::Replace($temp, $pointer, $backup) }
    else { [IO.File]::Move($temp, $pointer) }
} finally {
    if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force }
    if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Force }
}
Write-Output "Current build: $($exe.FullName)"
