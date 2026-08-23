param(
  [string]$Log = "$env:USERPROFILE\.marionette\logs\dev.log"
)
$hits = Select-String -Path $Log -Pattern 'turn/complete' | Where-Object { $_.Line -match 'kind=|no-app' } | Select-Object -Last 10
Write-Output ("matched: " + $hits.Count)
foreach ($l in $hits) { Write-Output $l.Line; Write-Output "===" }
# Also: what does the raw JSON-RPC prompt response look like? Search for "result" lines with an id
$resp = Select-String -Path $Log -Pattern '"jsonrpc":"2\.0","id":' | Select-Object -Last 8
Write-Output ("--- rpc responses (last 8) ---")
foreach ($l in $resp) {
  $t = $l.Line
  if ($t.Length -gt 1200) { $t = $t.Substring(0, 1200) }
  Write-Output $t
  Write-Output "==="
}
