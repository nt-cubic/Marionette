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
  [string]$Exe = "",
  # WebView2 UI language (clock/date formatting follows it). "" keeps the OS one.
  [string]$Lang = "zh-CN",
  # Skip the seeded conversation that makes the window look like real work.
  [switch]$NoSeed
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

# --- isolated profile: one project (the seeded conversation comes below) ----
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

# --- seeded conversation: the "screenshot state" ----------------------------
#
# The app renders a transcript straight from JSONL (parseTranscriptEvents), so a
# canned conversation gives a picture that looks like real work without running
# an agent: no quota, no network, same frame every time. Text lives in
# demo-seed\*.tmpl (UTF-8); this script is ASCII only.
if (-not $NoSeed) {
  $seedDir = Join-Path $PSScriptRoot "demo-seed"
  $sessionId = "session-20261001090000"
  $mdDir = Join-Path $app ".marionette"
  $sessionsDir = Join-Path $mdDir "sessions"
  $transcriptsDir = Join-Path $mdDir "transcripts"
  $handoffDir = Join-Path $mdDir "handoff"
  foreach ($dir in @($sessionsDir, $transcriptsDir, $handoffDir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
  }

  $utf8 = New-Object System.Text.UTF8Encoding($false)
  $jsonString = { param($s) $s.Replace('\', '\\').Replace('"', '\"') }
  $transcriptPath = Join-Path $transcriptsDir "$sessionId.jsonl"
  $rawLogPath = Join-Path $sessionsDir "$sessionId.raw.log"
  $handoffPath = Join-Path $handoffDir "$sessionId.md"

  $base = (Get-Date).ToUniversalTime().AddMinutes(-9)
  $stamp = { param($sec) $base.AddSeconds($sec).ToString("yyyy-MM-ddTHH:mm:ss.fffZ") }

  $body = [System.IO.File]::ReadAllText((Join-Path $seedDir "transcript.jsonl.tmpl"), [System.Text.Encoding]::UTF8)
  $tokens = [ordered]@{
    "__SESSION_ID__"   = $sessionId
    "__TS_USER__"      = & $stamp 0
    "__TS_THINK__"     = & $stamp 3
    "__TS_READ__"      = & $stamp 6
    "__TS_READ_DONE__" = & $stamp 10
    "__TS_EDIT__"      = & $stamp 15
    "__TS_EDIT_DONE__" = & $stamp 20
    "__TS_RUN__"       = & $stamp 25
    "__TS_RUN_DONE__"  = & $stamp 31
    "__TS_REPLY__"     = & $stamp 33
    "__TS_REPLY_DONE__" = & $stamp 34
  }
  foreach ($key in $tokens.Keys) { $body = $body.Replace($key, $tokens[$key]) }
  [System.IO.File]::WriteAllText($transcriptPath, $body, $utf8)
  Write-Host "  seeded conversation $transcriptPath"

  $sessionJson = [System.IO.File]::ReadAllText((Join-Path $seedDir "session.json.tmpl"), [System.Text.Encoding]::UTF8)
  $sessionJson = $sessionJson.
    Replace("__SESSION_ID__", $sessionId).
    Replace("__PROJECT_ID__", "demo-app").
    Replace("__CWD_JSON__", (& $jsonString $app)).
    Replace("__TRANSCRIPT_JSON__", (& $jsonString $transcriptPath)).
    Replace("__RAW_LOG_JSON__", (& $jsonString $rawLogPath)).
    Replace("__HANDOFF_JSON__", (& $jsonString $handoffPath)).
    Replace("__STARTED__", (& $stamp 0)).
    Replace("__LAST_ACTIVE__", (& $stamp 34))

  $indexFile = Join-Path $sessionsDir "index.json"
  $rows = @()
  if (Test-Path $indexFile) {
    try {
      $existing = Get-Content -Raw -Encoding UTF8 $indexFile | ConvertFrom-Json
      if ($existing) { $rows = @($existing) }
    } catch {
      Write-Host "  [warn] could not parse $indexFile - reseeding it"
    }
  }
  $rows = @($rows | Where-Object { $_.id -ne $sessionId })
  $rows = @($sessionJson | ConvertFrom-Json) + $rows
  [System.IO.File]::WriteAllText($indexFile, (ConvertTo-Json -InputObject $rows -Depth 8), $utf8)
  Write-Host "  seeded session row $indexFile"
}

# --- pick a build -----------------------------------------------------------
if (-not $Exe) {
  $found = @()
  foreach ($dir in @((Join-Path $repo "release"), (Join-Path $repo "dist-portable"))) {
    if (Test-Path $dir) {
      $found += Get-ChildItem $dir -Filter "Marionette*.exe" -Recurse -Depth 1 |
        Where-Object { $_.Name -notlike "*uncompressed*" }
    }
  }
  $found = $found | Sort-Object LastWriteTime -Descending
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
# Screenshots are shot for a Chinese README: the clock line ("下午 4:52") follows
# the WebView2 UI language, which otherwise inherits the machine's locale.
if ($Lang) { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--lang=$Lang" }
$proc = Start-Process -FilePath $Exe -PassThru

Write-Host ""
Write-Host "launched pid $($proc.Id)"
Write-Host "The window shows only demo-app. If another instance already holds this"
Write-Host "profile, it will hand off and exit instead of opening a second window."
Write-Host "Cleanup: close the window, then delete $DemoRoot (it holds the agent config copy)."
