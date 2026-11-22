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
    # With "keep running in background", X only hides the window, and starting a
    # second build just brings that old instance back. Say so instead of doing it silently.
    $version = $target.VersionInfo.ProductVersion
    $older = @(Get-Process -Name 'Marionette' -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and $_.Path -ne $target.FullName -and
        (Get-Item -LiteralPath $_.Path -ErrorAction SilentlyContinue).VersionInfo.ProductVersion -ne $version
    })
    if ($older.Count -gt 0) {
        $message = "旧版 Marionette 还在后台运行，所以暂时打不开 $version。`n`n" +
            "点「确定」后会显示旧窗口：按 Ctrl+K，选「退出 Marionette」(Quit Marionette)，再点一次这个快捷方式即可。"
        Add-Type -AssemblyName PresentationFramework
        [System.Windows.MessageBox]::Show($message, 'Marionette', 'OK', 'Information') | Out-Null
    }
    Start-Process -FilePath $target.FullName -WorkingDirectory $target.DirectoryName
} catch {
    if ($ShowTarget) { Write-Error $_; exit 1 }
    Add-Type -AssemblyName PresentationFramework
    [System.Windows.MessageBox]::Show($_.Exception.Message, 'Marionette', 'OK', 'Error') | Out-Null
    exit 1
}
