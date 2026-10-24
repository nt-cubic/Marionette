$env:SKIP_MAGPIE_IMPORT = '1'
$env:NOPAUSE = '1'
Set-Location (Join-Path $PSScriptRoot '..')
& cmd.exe /c 'tools\build-portable.bat'
exit $LASTEXITCODE
