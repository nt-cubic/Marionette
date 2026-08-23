param(
  [string]$File = "D:\Projects\Marionette\.marionette\transcripts\session-1787487693671.jsonl",
  [int]$Tail = 300
)
$item = Get-Item $File
Write-Output ("size=" + $item.Length + "  mtime=" + $item.LastWriteTime)
$lines = Get-Content $File -Tail $Tail
Write-Output ("--- last " + $lines.Count + " lines, turnStats hits: " + (@($lines | Where-Object { $_ -match '"turnStats"' }).Count) + " ---")
$hits = @($lines | Where-Object { $_ -match '"turnStats"' })
foreach ($h in ($hits | Select-Object -Last 3)) {
  # Print only the type + turnStats portion to keep it readable
  if ($h -match '"type"\s*:\s*"([^"]+)"') { Write-Output ("event type: " + $matches[1]) }
  $i = $h.IndexOf('"turnStats"')
  if ($i -ge 0) {
    Write-Output ("turnStats: " + $h.Substring($i, [Math]::Min(600, $h.Length - $i)))
  }
  Write-Output "==="
}
