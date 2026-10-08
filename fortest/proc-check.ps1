$ErrorActionPreference = 'Continue'
Get-Process -Name Marionette -ErrorAction SilentlyContinue |
  Select-Object Id, StartTime, Path |
  Format-List
Write-Output '----'
Get-Process -Name node -ErrorAction SilentlyContinue |
  Select-Object Id, StartTime, Path |
  Format-List
