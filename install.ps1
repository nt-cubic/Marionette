# Marionette installer - one line on Windows 10 1803+ (x64):
#
#   irm https://github.com/nt-cubic/Marionette/releases/latest/download/install.ps1 | iex
#
# Downloads the latest release build and installs it as a proper app, no
# admin needed:
#   - Marionette.exe into %LOCALAPPDATA%\Programs\Marionette
#   - a Start menu shortcut
#   - the install dir on the user PATH, so `marionette` works in terminals
#
# Running it again upgrades in place: the install path never changes, so the
# shortcut keeps working and in-app updates (app_update.rs) keep replacing
# the same exe.
#
# Uninstall (run it as a file, not through iex):
#   curl.exe -fsSLo install.ps1 https://github.com/nt-cubic/Marionette/releases/latest/download/install.ps1
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
#   del install.ps1
#
# User data in ~/.marionette is never touched by -Uninstall.
#
# Kept in sync with tools\install-app.ps1 (the local dev install path, which
# copies from release\ instead of downloading). Both stay standalone on
# purpose: a release asset must not depend on a repo checkout.
param(
    [switch]$Uninstall
)
# Own scope: under `irm | iex` this would otherwise set the caller's variables.
& {
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# throw, not exit: under `irm | iex`, exit would close the person's window.
function Fail($why) { throw "marionette: $why" }

$repo = 'nt-cubic/Marionette'
$dir = Join-Path $env:LOCALAPPDATA "Programs\Marionette"
$menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Marionette.lnk"
$exe = Join-Path $dir "Marionette.exe"

if ($env:PROCESSOR_ARCHITEW6432 -or $env:PROCESSOR_ARCHITECTURE) {
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64') { Fail "no build for $arch Windows yet (x64 only)" }
}

if (-not ("MarionetteInstall.Native" -as [type])) {
    Add-Type -Namespace MarionetteInstall -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint msg, System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);
'@
}

# Tell running programs (Explorer and the terminals it spawns) that the user
# environment changed, so `marionette` on PATH works in a freshly opened
# terminal without re-login. Best-effort by design.
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
        Fail "Marionette is running; exit it first, then uninstall again."
    }
    Remove-Item -LiteralPath $menu -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    if (Set-UserPathEntry -Entry $dir -Remove) { Broadcast-EnvironmentChange }

    $leftovers = @($menu, $dir) | Where-Object { Test-Path -LiteralPath $_ }
    if ($leftovers) { Fail "uninstall incomplete, could not remove: $($leftovers -join ', ')" }
    Write-Host "Marionette uninstalled (user data in ~\.marionette kept)."
    return
}

if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue)) {
    Fail "needs curl.exe (Windows 10 1803 or newer)"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) "marionette-install-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    # releases/latest/download/ redirects to the newest stable asset. A HEAD
    # without -L gives the first hop (.../download/vX.Y.Z/Marionette.exe), which
    # names the version without fetching 19 MB twice. Best-effort.
    $url = "https://github.com/$repo/releases/latest/download/Marionette.exe"
    $firstHop = (& curl.exe -fsSI -o NUL -w '%{redirect_url}' $url | Out-String).Trim()
    if ($LASTEXITCODE) { Fail "cannot reach GitHub releases" }
    $version = if ($firstHop -match '/download/(v[^/]+)/') { $Matches[1] } else { '' }
    $display = if ($version) { $version } else { "(latest)" }

    Write-Host "Installing Marionette $display for windows-x86_64..."
    $download = Join-Path $tmp 'Marionette.exe'
    & curl.exe -fsSL $url -o $download
    if ($LASTEXITCODE) { Fail "download failed: $url" }

    # A release-asset mixup or an error page must not become the installed app.
    $magic = [byte[]]::new(2)
    $fs = [IO.File]::OpenRead($download)
    try { [void]$fs.Read($magic, 0, 2) } finally { $fs.Close() }
    $size = (Get-Item -LiteralPath $download).Length
    if ($size -lt 1MB -or $magic[0] -ne 0x4D -or $magic[1] -ne 0x5A) {
        Fail "the download is not a Marionette build; try again"
    }

    # A running exe can be renamed, not deleted: move the old image aside,
    # drop the new build in, then try to clean up. Move-Item -Force
    # overwrites (Rename-Item never does). If the previous upgrade's .old is
    # still locked by a running app, fall back to a timestamped .old name so
    # the upgrade still succeeds and the running old build stays intact.
    New-Item -ItemType Directory -Force $dir | Out-Null
    if (Test-Path -LiteralPath $exe) {
        Remove-Item -LiteralPath "$exe.old" -Force -ErrorAction SilentlyContinue
        try {
            Move-Item -LiteralPath $exe -Destination "$exe.old" -Force -ErrorAction Stop
        } catch [System.IO.IOException] {
            Move-Item -LiteralPath $exe -Destination "$exe.old.$(Get-Date -Format 'yyyyMMdd-HHmmss')" -Force -ErrorAction Stop
        }
    }
    Copy-Item -LiteralPath $download -Destination $exe -Force
    # Sweep every renamed-aside image, including timestamped leftovers from
    # upgrades done while the app was running; locked ones are skipped.
    # -Filter with a literal dir: brackets in the user's folder name are legal
    # and a wildcard -Path would silently match nothing there.
    Get-ChildItem -LiteralPath $dir -Filter 'Marionette.exe.old*' -File -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue

    $link = (New-Object -ComObject WScript.Shell).CreateShortcut($menu)
    $link.TargetPath = $exe
    $link.WorkingDirectory = $env:USERPROFILE
    $link.Description = "Marionette - local multi-agent CLI shell"
    $link.Save()

    if (Set-UserPathEntry -Entry $dir) { Broadcast-EnvironmentChange }

    Write-Host "Marionette $display installed."
    Write-Host "  exe:        $exe"
    Write-Host "  Start menu: $menu"
    Write-Host "Search 'Marionette' in the Start menu, or run:"
    Write-Host "  start-process `"$exe`""
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
}
