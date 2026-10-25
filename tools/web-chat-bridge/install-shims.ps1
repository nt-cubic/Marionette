$dest = Join-Path $env:APPDATA 'npm'
$node = (Get-Command node -ErrorAction SilentlyContinue).Source
if (-not $node) { Write-Error 'node.exe not found on PATH'; exit 1 }
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
foreach ($s in 'chatgpt','deepseek') {
  $body = "@echo off`r`n`"$node`" `"$here\bridge.mjs`" $s %*`r`n"
  Set-Content -Path (Join-Path $dest "web-$s-acp.cmd") -Value $body -Encoding ASCII
  Write-Host "installed $(Join-Path $dest "web-$s-acp.cmd") (node=$node)"
}
