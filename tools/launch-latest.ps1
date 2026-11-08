param(
    [string]$ReleaseDir = (Join-Path (Split-Path -Parent $PSScriptRoot) 'release'),
    [switch]$ShowTarget
)
$ErrorActionPreference = 'Stop'
try {
    $pointer = Join-Path $ReleaseDir 'current-build.txt'
    if (-not (Test-Path -LiteralPath $pointer -PathType Leaf)) {
        throw "No current build is recorded. Run tools\build-portable.bat first."
    }
    $relative = [IO.File]::ReadAllText($pointer).Trim()
    if ($relative -notmatch '^[^\\/:*?"<>|\r\n]+[\\/]Marionette\.exe$' -or $relative -match '^\.\.?[\\/]') {
        throw "Invalid current build pointer: $pointer"
    }
    $target = Join-Path $ReleaseDir $relative
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
        throw "The recorded build no longer exists: $target. Build again to update the shortcut."
    }
    $target = Get-Item -LiteralPath $target
    if ($ShowTarget) { Write-Output $target.FullName; exit 0 }
    Start-Process -FilePath $target.FullName -WorkingDirectory $target.DirectoryName
} catch {
    if ($ShowTarget) { Write-Error $_; exit 1 }
    Add-Type -AssemblyName PresentationFramework
    [System.Windows.MessageBox]::Show($_.Exception.Message, 'Marionette', 'OK', 'Error') | Out-Null
    exit 1
}
