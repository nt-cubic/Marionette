param(
  [string]$Log = "$env:USERPROFILE\.marionette\logs\dev.log"
)
# turn-level usage: turn_completed notifications and prompt RPC results
$hits = Select-String -Path $Log -Pattern '"sessionUpdate":"turn_completed"|turn/complete|prompt.*result|"usage":\{' | Where-Object { $_.Line -notmatch 'arguments_delta' } | Select-Object -Last 12
Write-Output ("matched: " + $hits.Count)
foreach ($l in $hits) {
  $t = $l.Line
  if ($t.Length -gt 1200) { $t = $t.Substring(0, 1200) }
  Write-Output $t
  Write-Output "==="
}
