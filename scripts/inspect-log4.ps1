param(
  [string]$Log = "$env:USERPROFILE\.marionette\logs\dev.log"
)
# Log line shape: [ts] [level] [acp] session=<sid> <method> | <json>
$hits = Select-String -Path $Log -Pattern 'session=session-1787487693671 (turn/complete|turn/completed)' | Select-Object -Last 6
Write-Output ("matched: " + $hits.Count)
foreach ($l in $hits) {
  $t = $l.Line
  if ($t.Length -gt 1600) { $t = $t.Substring(0, 1600) }
  Write-Output $t
  Write-Output "==="
}
