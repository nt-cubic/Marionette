param(
  [string]$Dir = "$env:USERPROFILE\.marionette\logs"
)
foreach ($f in @("dev.log", "dev.log.1")) {
  $p = Join-Path $Dir $f
  if (-not (Test-Path $p)) { continue }
  Write-Output ("--- $f ---")
  $hits = Select-String -Path $p -Pattern ' session=session-1787487693671 turn/complete '
  foreach ($l in $hits) {
    $t = $l.Line
    if ($t.Length -gt 400) { $t = $t.Substring(0, 400) }
    Write-Output $t
  }
  $tc = Select-String -Path $p -Pattern 'turn_completed'
  Write-Output ("turn_completed mentions: " + $tc.Count)
  foreach ($l in ($tc | Select-Object -Last 3)) {
    $t = $l.Line
    if ($t.Length -gt 700) { $t = $t.Substring(0, 700) }
    Write-Output $t
    Write-Output "==="
  }
}
