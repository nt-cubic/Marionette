<#
  Start an isolated Marionette instance for screenshots and trial runs.

  USERPROFILE and WEBVIEW2_USER_DATA_FOLDER both point at a throwaway profile, so
  the window shows only the seeded demo project: no real projects, no real
  conversations, no remembered layout. A running instance is left alone, because
  the single-instance lock lives in <USERPROFILE>\.marionette\instance.lock.

  The demo profile gets a copy of one agent config so an agent can actually
  answer. That copy holds credentials - delete the demo root when done.

  Usage:
    tools\demo-instance.bat
    tools\demo-instance.bat -DemoRoot D:\demo -AgentConfig none
#>
param(
  [string]$DemoRoot = (Join-Path $env:TEMP "marionette-demo"),
  [ValidateSet("grok", "none")]
  [string]$AgentConfig = "grok",
  [string]$Exe = ""
)

$ErrorActionPreference = "Stop"
$realHome = $env:USERPROFILE
$repo = Split-Path -Parent $PSScriptRoot
$app = Join-Path $DemoRoot "app"
$profile = Join-Path $DemoRoot "profile"
$webview = Join-Path $DemoRoot "webview2"

function Write-Text([string]$Path, [string]$Content) {
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
  [System.IO.File]::WriteAllText($Path, $Content)
  Write-Host "  wrote $Path"
}

function Write-TextIfMissing([string]$Path, [string]$Content) {
  if (Test-Path $Path) {
    Write-Host "  kept  $Path"
  } else {
    Write-Text $Path $Content
  }
}

Write-Host ""
Write-Host "=== Marionette demo instance ==="
Write-Host "demo root: $DemoRoot"
Write-Host ""

# --- demo project: small, neutral, nothing real -----------------------------
Write-TextIfMissing (Join-Path $app "package.json") @"
{
  "name": "demo-app",
  "version": "0.1.0",
  "private": true,
  "type": "module"
}
"@
Write-TextIfMissing (Join-Path $app "README.md") "# demo-app`n`nA throwaway workspace used for screenshots. Nothing here is real.`n"
Write-TextIfMissing (Join-Path $app "src\greeting.ts") @"
export function greet(name: string): string {
  return ``hello, `${name}``;
}
"@
Write-TextIfMissing (Join-Path $app "src\greeting.test.ts") @"
import { greet } from "./greeting";

if (greet("world") !== "hello, world") {
  throw new Error("greet() is broken");
}
console.log("ok");
"@

# --- a git repo with one pending change, so the diff panels have content ----
if (-not (Test-Path (Join-Path $app ".git"))) {
  Push-Location $app
  try {
    & git init -q
    & git add -A
    & git -c user.name=demo -c user.email=demo@example.com commit -qm init
    Add-Content -Path (Join-Path $app "src\greeting.ts") -Value 'export const version = "0.1.0";'
    Write-Host "  initialised demo git repo with one pending change"
  } finally {
    Pop-Location
  }
}

# --- isolated profile: one project, no sessions -----------------------------
$projectsFile = Join-Path $profile ".marionette\projects.json"
if (-not (Test-Path $projectsFile)) {
  $now = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
  $jsonPath = $app.Replace('\', '\\')
  Write-Text $projectsFile @"
[
  {
    "id": "demo-app",
    "name": "demo-app",
    "rootPath": "$jsonPath",
    "createdAt": "$now",
    "lastOpenedAt": "$now"
  }
]
"@
} else {
  Write-Host "  kept  $projectsFile"
}

# --- agent config so the demo instance can talk to an agent -----------------
if ($AgentConfig -eq "grok") {
  $grokConfig = Join-Path $realHome ".grok\config.toml"
  $target = Join-Path $profile ".grok\config.toml"
  if (Test-Path $grokConfig) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
    Copy-Item -Force $grokConfig $target
    Write-Host "  copied grok config into the demo profile (holds credentials)"
  } else {
    Write-Host "  [warn] no $grokConfig - the demo instance will have no configured agent"
  }
}

# --- pick a build -----------------------------------------------------------
if (-not $Exe) {
  $found = @()
  $portable = Join-Path $repo "dist-portable"
  if (Test-Path $portable) {
    $found += Get-ChildItem $portable -Filter "Marionette*.exe" |
      Where-Object { $_.Name -notlike "*uncompressed*" } |
      Sort-Object LastWriteTime -Descending
  }
  foreach ($fallback in @("src-tauri\target\release\marionette.exe", "src-tauri\target\debug\marionette.exe")) {
    $p = Join-Path $repo $fallback
    if (Test-Path $p) { $found += Get-Item $p }
  }
  if ($found.Count -gt 0) { $Exe = $found[0].FullName }
}
if (-not $Exe -or -not (Test-Path $Exe)) {
  throw "No Marionette build found. Run tools\build-portable.bat first, or pass -Exe."
}
Write-Host "  exe   $Exe"

# --- launch, isolated -------------------------------------------------------
New-Item -ItemType Directory -Force -Path $webview | Out-Null
$env:USERPROFILE = $profile
$env:WEBVIEW2_USER_DATA_FOLDER = $webview
$proc = Start-Process -FilePath $Exe -PassThru

Write-Host ""
Write-Host "launched pid $($proc.Id)"
Write-Host "The window shows only demo-app. If another instance already holds this"
Write-Host "profile, it will hand off and exit instead of opening a second window."
Write-Host "Cleanup: close the window, then delete $DemoRoot (it holds the agent config copy)."
