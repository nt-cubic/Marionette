param(
  [string]$Dir = "$env:USERPROFILE\.marionette\logs"
)
foreach ($f in @("dev.log", "dev.log.1")) {
  $p = Join-Path $Dir $f
  if (-not (Test-Path $p)) { continue }
  Write-Output "===== $f ====="
  # Genuine update events only: line has "sessionUpdate":"turn_completed" but is NOT one of my diagnostic writes
  $tc = Select-String -Path $p -Pattern '"sessionUpdate":"turn_completed"' | Where-Object { $_.Line -notmatch 'inspect-log|arguments_delta|rawInput|diff' }
  Write-Output ("genuine turn_completed: " + $tc.Count)
  foreach ($l in $tc) {
    $t = $l.Line
    if ($t.Length -gt 900) { $t = $t.Substring(0, 900) }
    Write-Output $t
    Write-Output "==="
  }
  $rc = Select-String -Path $p -Pattern '"sessionUpdate":"response_completed"' | Where-Object { $_.Line -notmatch 'inspect-log|arguments_delta|rawInput|diff' }
  Write-Output ("genuine response_completed: " + $rc.Count)
  foreach ($l in $rc) {
    $t = $l.Line
    if ($t.Length -gt 500) { $t = $t.Substring(0, 500) }
    Write-Output $t
    Write-Output "==="
  }
}
