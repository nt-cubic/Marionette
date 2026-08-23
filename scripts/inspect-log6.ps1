param(
  [string]$Log = "$env:USERPROFILE\.marionette\logs\dev.log"
)
# Strict: the log line itself is "[ts] [lvl] [acp] session=<sid> turn/complete | kind=..."
$hits = Select-String -Path $Log -Pattern '^\[\d+\] \[\w+\] \[acp\] session=\S+ turn/complete ' | Select-Object -Last 10
Write-Output ("turn/complete lines: " + $hits.Count)
foreach ($l in $hits) { Write-Output $l.Line; Write-Output "===" }

# turn_completed sessionUpdate (Grok's turn-level usage notification)
$tc = Select-String -Path $Log -Pattern '"sessionUpdate":"turn_completed"' | Select-Object -Last 4
Write-Output ("turn_completed updates: " + $tc.Count)
foreach ($l in $tc) {
  $t = $l.Line
  if ($t.Length -gt 1500) { $t = $t.Substring(0, 1500) }
  Write-Output $t
  Write-Output "==="
}
