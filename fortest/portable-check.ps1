$ErrorActionPreference = 'Continue'
$root = 'D:\Projects\Marionette'
Write-Output '== dist-portable =='
Get-ChildItem "$root\dist-portable" -ErrorAction SilentlyContinue |
  Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize
Write-Output '== dist-portable recursive (newest 15) =='
Get-ChildItem "$root\dist-portable" -Recurse -File -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 15 FullName, LastWriteTime |
  Format-Table -AutoSize
Write-Output '== src newest =='
Get-ChildItem "$root\src" -Recurse -File |
  Sort-Object LastWriteTime -Descending | Select-Object -First 5 FullName, LastWriteTime |
  Format-Table -AutoSize
