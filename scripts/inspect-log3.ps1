param(
  [string]$Log = "$env:USERPROFILE\.marionette\logs\dev.log"
)
# The prompt JSON-RPC response carries stopReason in its result
$hits = Select-String -Path $Log -Pattern 'stopReason' | Where-Object { $_.Line -notmatch 'arguments_delta' } | Select-Object -Last 8
Write-Output ("matched: " + $hits.Count)
foreach ($l in $hits) {
  $t = $l.Line
  if ($t.Length -gt 1500) { $t = $t.Substring(0, 1500) }
  Write-Output $t
  Write-Output "==="
}
