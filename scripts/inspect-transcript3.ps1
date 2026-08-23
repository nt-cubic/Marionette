param(
  [string]$File = "D:\Projects\Marionette\.marionette\transcripts\session-1787487693671.jsonl"
)
$lines = Get-Content $File
Write-Output ("total lines: " + $lines.Count)
$am = @($lines | Where-Object { $_ -match '"type"\s*:\s*"assistant_message"' })
Write-Output ("assistant_message total: " + $am.Count)
$completed = @($am | Where-Object { $_ -match '"durationMs"\s*:\s*[0-9]' })
Write-Output ("completed (durationMs set): " + $completed.Count)
$withStats = @($completed | Where-Object { $_ -match '"turnStats"\s*:\s*\{' })
Write-Output ("completed WITH turnStats object: " + $withStats.Count)
Write-Output "--- last 6 completed turns ---"
foreach ($l in ($completed | Select-Object -Last 6)) {
  $i = $l.IndexOf('"durationMs"')
  $seg = $l.Substring($i, [Math]::Min(400, $l.Length - $i))
  $len = $l.Length
  Write-Output ("lineLen=$len :: $seg")
  Write-Output "==="
}
Write-Output "--- all completed turns WITH stats (if any) ---"
foreach ($l in ($withStats | Select-Object -Last 3)) {
  $i = $l.IndexOf('"turnStats"')
  $seg = $l.Substring($i, [Math]::Min(500, $l.Length - $i))
  Write-Output $seg
  Write-Output "==="
}
