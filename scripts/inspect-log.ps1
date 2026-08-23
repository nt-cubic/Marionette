param(
  [string]$Log = "$env:USERPROFILE\.marionette\logs\dev.log",
  [int]$Count = 8
)
$lines = Select-String -Path $Log -Pattern 'turn/complete|turn_completed|session_notification' | Select-Object -Last ($Count * 4)
Write-Output ("matched: " + $lines.Count)
foreach ($l in $lines) {
  $t = $l.Line
  if ($t.Length -gt 900) { $t = $t.Substring(0, 900) }
  Write-Output $t
  Write-Output "==="
}
