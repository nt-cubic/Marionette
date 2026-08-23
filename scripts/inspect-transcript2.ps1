param(
  [string]$File = "D:\Projects\Marionette\.marionette\transcripts\session-1787487693671.jsonl"
)
$lines = Get-Content $File -Tail 40
$am = @($lines | Where-Object { $_ -match '"type"\s*:\s*"assistant_message"' })
Write-Output ("assistant_message lines in tail 40: " + $am.Count)
foreach ($l in ($am | Select-Object -Last 4)) {
  # strip the huge text content; keep type, ids, stats, duration
  $j = $l | ConvertFrom-Json
  $out = [ordered]@{
    type = $j.type
    messageId = $j.messageId
    ts = $j.ts
    durationMs = $j.durationMs
    turnStats = $j.turnStats
    textLen = $(if ($j.text) { $j.text.Length } else { 0 })
    textHead = $(if ($j.text) { $j.text.Substring(0, [Math]::Min(80, $j.text.Length)) } else { "" })
  }
  Write-Output ($out | ConvertTo-Json -Compress)
  Write-Output "==="
}
# also: what other event types are in the tail, and do any carry usage-ish fields
$types = @{}
foreach ($l in $lines) {
  try {
    $j = $l | ConvertFrom-Json
    $t = [string]$j.type
    $types[$t] = [int]$($types[$t]) + 1
  } catch {}
}
Write-Output ("event types in tail 40: " + (($types.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join ", "))
