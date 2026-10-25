# 注册「ChatGPT 网页版 / DeepSeek 网页版」两个自定义 agent 到 Marionette。
#
# 为什么是 node + 绝对路径参数，而不是一个 .cmd 垫片命令：
# process_util 对 .cmd 会做「拆 node」优化，会把非 node_modules 形态的垫片
# 拆成裸 node（参数全丢 → node 进 REPL 假死）。直接注册 node + args 绕开整
# 个垫片机制。幂等：已存在（按 id）则更新命令与参数。
#
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File register-agents.ps1

$ErrorActionPreference = 'Stop'

$node = (Get-Command node -ErrorAction SilentlyContinue).Source
if (-not $node) { Write-Error 'node.exe not found on PATH'; exit 1 }
$bridge = Join-Path $PSScriptRoot 'bridge.mjs'
if (-not (Test-Path $bridge)) { Write-Error "bridge.mjs not found: $bridge"; exit 1 }

$target = Join-Path $env:USERPROFILE '.marionette\custom_agents.json'
$list = @()
if (Test-Path $target) {
  $raw = Get-Content $target -Raw -Encoding UTF8
  if ($raw -and $raw.Trim()) { $list = $raw | ConvertFrom-Json }
}

$agents = @(
  @{
    id    = 'web-chatgpt'
    label = 'ChatGPT 网页版'
    note  = '纯聊天：桥接 ChatGPT 网页版，额度走网页份额。首次使用会弹出一个浏览器窗口，请在那里登录一次。'
  },
  @{
    id    = 'web-deepseek'
    label = 'DeepSeek 网页版'
    note  = '纯聊天：桥接 DeepSeek 网页版，额度走网页份额。首次使用会弹出一个浏览器窗口，请在那里登录一次。'
  }
)

foreach ($a in $agents) {
  $entry = [pscustomobject]@{
    id          = $a.id
    label       = $a.label
    command     = $node
    args        = @($bridge, $a.id)
    npmPackage  = $null
    note        = $a.note
  }
  $existing = $list | Where-Object { $_.id -eq $a.id }
  if ($existing) {
    $existing.command = $node
    $existing.args = @($bridge, $a.id)
    $existing.label = $a.label
    $existing.note = $a.note
    Write-Host "updated $($a.id) (command=$node)"
  } else {
    $list = @($list) + $entry
    Write-Host "added $($a.id) (command=$node)"
  }
}

$json = $list | ConvertTo-Json -Depth 5
if (@($list).Count -eq 1) { $json = '[{0}]' -f $json }
New-Item -ItemType Directory -Force -Path (Split-Path $target) | Out-Null
[System.IO.File]::WriteAllText($target, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "written $target"
