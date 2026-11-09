# Installs the last portable build as a regular Windows app:
#
#   powershell -ExecutionPolicy Bypass -File tools\install-app.ps1
#
# Copies release\<version>\Marionette.exe (the build-portable.bat output,
# recorded in release\current-build.txt) into %LOCALAPPDATA%\Programs\Marionette
# and puts a Start menu shortcut there. Running the script again upgrades in
# place: the shortcut keeps working because the install path never changes.
#
# In-app updates (app_update.rs) keep replacing that same exe, so the
# shortcut survives every update. No admin needed.
#
# -Uninstall removes the install dir, the Start menu shortcut and the PATH
# entry (user data in ~/.marionette is left alone). Close a running
# Marionette first; the script refuses to report success on leftovers.
param(
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'

$dir = Join-Path $env:LOCALAPPDATA "Programs\Marionette"
$menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Marionette.lnk"
$exe = Join-Path $dir "Marionette.exe"

# Add-Type once per session; the type stays loaded for both call sites.
if (-not ("MarionetteInstall.Native" -as [type])) {
    Add-Type -Namespace MarionetteInstall -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint msg, System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);
'@
}

# Tell running programs (Explorer and the terminals it spawns) that the user
# environment changed, so `marionette` on PATH works in a freshly opened
# terminal without re-login. HWND_BROADCAST + WM_SETTINGCHANGE +
# SMTO_ABORTIFHUNG; best-effort by design.
function Broadcast-EnvironmentChange {
    $result = [UIntPtr]::Zero
    [void][MarionetteInstall.Native]::SendMessageTimeout(
        [IntPtr]0xFFFF, 0x1A, [UIntPtr]::Zero, "Environment", 0x0002, 5000, [ref]$result)
}

# Append $Entry to, or remove it from, the raw (unexpanded) user PATH.
# Returns $true when the value changed. The raw value keeps %VARIABLES%
# unexpanded and the original registry kind intact.
function Set-UserPathEntry {
    param([string]$Entry, [switch]$Remove)
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
    try {
        $raw = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($Remove) {
            $kept = ($raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ine $Entry.TrimEnd("\") }) -join ";"
        } else {
            $present = $raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ieq $Entry.TrimEnd("\") }
            if ($present) { return $false }
            $kept = if ($raw) { "$($raw.TrimEnd(';'));$Entry" } else { $Entry }
        }
        # Nothing to do (also covers "value absent, still absent").
        if ($kept -eq $raw) { return $false }
        $kind = if ($key.GetValueNames() -contains "Path") { $key.GetValueKind("Path") } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
        if (-not $kept) {
            # Last entry removed: drop the value instead of leaving an empty one.
            $key.DeleteValue("Path", $false)
            return $true
        }
        $key.SetValue("Path", $kept, $kind)
        return $true
    } finally {
        $key.Close()
    }
}

if ($Uninstall) {
    if (Get-Process -Name Marionette -ErrorAction SilentlyContinue) {
        throw "Marionette is running; exit it first, then uninstall again."
    }
    Remove-Item -LiteralPath $menu -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    if (Set-UserPathEntry -Entry $dir -Remove) { Broadcast-EnvironmentChange }

    $leftovers = @($menu, $dir) | Where-Object { Test-Path -LiteralPath $_ }
    if ($leftovers) {
        throw "Uninstall incomplete, could not remove: $($leftovers -join ', ')"
    }
    Write-Host "Marionette uninstalled (user data in ~\.marionette kept)."
    exit 0
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$pointer = Join-Path $repoRoot "release\current-build.txt"
if (-not (Test-Path -LiteralPath $pointer -PathType Leaf)) {
    throw "No build recorded. Run tools\build-portable.bat first (see release\current-build.txt)."
}
$relative = [IO.File]::ReadAllText($pointer).Trim()
if ($relative -notmatch '^[^\\/:*?"<>|\r\n]+[\\/]Marionette\.exe$' -or $relative -match '^\.\.?[\\/]') {
    throw "Invalid current build pointer: $pointer"
}
$source = Join-Path (Join-Path $repoRoot "release") $relative
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "The recorded build no longer exists: $source. Build again to update the pointer."
}

# A running exe can be renamed, not deleted: move the old image aside, drop
# the new build in, then try to clean up.
# Move-Item -Force overwrites (Rename-Item never does). If the previous
# upgrade's .old is still locked by a running app, clear the way first:
# best-effort delete, then fall back to a versioned .old name so the
# upgrade still succeeds and the running old build stays intact.
New-Item -ItemType Directory -Force $dir | Out-Null
if (Test-Path -LiteralPath $exe) {
    Remove-Item -LiteralPath "$exe.old" -Force -ErrorAction SilentlyContinue
    try {
        Move-Item -LiteralPath $exe -Destination "$exe.old" -Force -ErrorAction Stop
    } catch [System.IO.IOException] {
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        Move-Item -LiteralPath $exe -Destination "$exe.old.$stamp" -Force -ErrorAction Stop
    }
}
Copy-Item -LiteralPath $source -Destination $exe -Force
# Sweep every renamed-aside image, including the timestamped ones from
# upgrades done while the app was running. Locked (still executing) files
# are skipped silently and picked up on a later run. -Filter with a literal
# dir: brackets in the user's folder name are legal and a wildcard -Path
# would silently match nothing there.
Get-ChildItem -LiteralPath $dir -Filter 'Marionette.exe.old*' -File -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

$link = (New-Object -ComObject WScript.Shell).CreateShortcut($menu)
$link.TargetPath = $exe
$link.WorkingDirectory = $env:USERPROFILE
$link.Description = "Marionette - local multi-agent CLI shell"
$link.Save()

if (Set-UserPathEntry -Entry $dir) { Broadcast-EnvironmentChange }

$version = ($relative -split '[\\/]')[0]
Write-Host "Marionette $version installed."
Write-Host "  exe:        $exe"
Write-Host "  Start menu: $menu"
Write-Host "Search 'Marionette' in the Start menu, or run:"
Write-Host "  start-process `"$exe`""
