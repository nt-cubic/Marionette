param(
  [string]$Dir = "$env:USERPROFILE\.marionette\logs"
)
Get-ChildItem $Dir -File | Sort-Object Name | ForEach-Object {
  Write-Output ($_.Name + "  " + [Math]::Round($_.Length / 1KB) + " KB  " + $_.LastWriteTime)
}
Write-Output "---"
foreach ($f in @("dev.log", "dev.log.1")) {
  $p = Join-Path $Dir $f
  if (-not (Test-Path $p)) { continue }
  $first = Get-Content $p -TotalCount 1
  $last = Get-Content $p -Tail 1
  Write-Output ("$f first: " + $first.Substring(0, [Math]::Min(120, $first.Length)))
  Write-Output ("$f last:  " + $last.Substring(0, [Math]::Min(120, $last.Length)))
  $n = (Select-String -Path $p -Pattern ' session=session-1787487693671 turn/complete ' | Measure-Object).Count
  Write-Output ("$f turn/complete lines: $n")
  Write-Output "==="
}
