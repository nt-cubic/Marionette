@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Marionette portable build

cd /d "%~dp0"

echo.
echo  === Marionette portable build ===
echo  Single exe ^| ring-less TLS ^(schannel^) ^| UPX ship ~1.2 MB
echo.
echo  Env: SKIP_UPX=1  REQUIRE_UPX=1  INSTALL_UPX=1
echo.

rem -- PATH: cargo / npm --
if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
if exist "%APPDATA%\npm" set "PATH=%APPDATA%\npm;%PATH%"

rem -- MSVC --
set "VCVARS="
if exist "%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" (
  set "VCVARS=%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
)
if not defined VCVARS if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" (
  set "VCVARS=%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
)
if not defined VCVARS if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat" (
  set "VCVARS=%ProgramFiles%\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat"
)
if not defined VCVARS if exist "%ProgramFiles(x86)%\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat" (
  set "VCVARS=%ProgramFiles(x86)%\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
)
if defined VCVARS (
  echo  [env] MSVC vcvars64
  call "%VCVARS%" >nul 2>&1
) else (
  echo  [env] MSVC vcvars not found - continuing
)

where node >nul 2>&1
if errorlevel 1 (
  echo  [error] node not on PATH
  goto fail
)
where npm >nul 2>&1
if errorlevel 1 (
  echo  [error] npm not on PATH
  goto fail
)
where cargo >nul 2>&1
if errorlevel 1 (
  echo  [error] cargo not on PATH
  goto fail
)

if not exist "package.json" (
  echo  [error] package.json missing
  goto fail
)
if not exist "node_modules\" (
  echo  [npm] npm install...
  call npm install
  if errorlevel 1 goto fail
)

rem -- ring-less guard --
echo  [deps] check ring / rustls absent...
pushd "src-tauri"
cargo tree -i ring >nul 2>&1
if not errorlevel 1 (
  echo  [error] ring is in the tree - use ureq native-tls only
  popd
  goto fail
)
cargo tree -i rustls >nul 2>&1
if not errorlevel 1 (
  echo  [error] rustls is in the tree - keep native-tls / schannel
  popd
  goto fail
)
cargo tree -i native-tls >nul 2>&1
if errorlevel 1 (
  echo  [warn] native-tls not found via cargo tree
) else (
  echo  [deps] OK: no ring/rustls; native-tls present
)
popd

rem -- find UPX binary (do NOT name env UPX — upx.exe uses that for options) --
set "UPX_BIN="
set "UPX_NOTE="
if /I "%SKIP_UPX%"=="1" (
  set "UPX_NOTE=skipped (SKIP_UPX=1)"
  echo  [upx] !UPX_NOTE!
  goto after_upx_find
)

call :find_upx
if defined UPX_BIN goto after_upx_ver

if /I "%INSTALL_UPX%"=="1" (
  echo  [upx] not found - winget install UPX.UPX ...
  where winget >nul 2>&1
  if not errorlevel 1 (
    winget install -e --id UPX.UPX --accept-package-agreements --accept-source-agreements
    set "PATH=%PATH%;%LOCALAPPDATA%\Microsoft\WinGet\Links"
    call :find_upx
  )
)

if not defined UPX_BIN (
  set "UPX_NOTE=MISSING - ship stays ~4-7 MB uncompressed"
  echo  [upx] !UPX_NOTE!
  echo         winget install -e --id UPX.UPX
  echo         or: tools\upx.exe
  if /I "%REQUIRE_UPX%"=="1" (
    echo  [error] REQUIRE_UPX=1 and upx missing
    goto fail
  )
  goto after_upx_find
)

:after_upx_ver
echo  [upx] found: !UPX_BIN!
"!UPX_BIN!" --version 2>nul | more +0

:after_upx_find

echo.
echo  [build] tauri release (LTO+strip+opt-z)...
echo.
call npm run build:portable
if errorlevel 1 goto fail

set "SRC=%~dp0src-tauri\target\release\marionette.exe"
if not exist "%SRC%" (
  echo  [error] binary missing: %SRC%
  goto fail
)

set "OUTDIR=%~dp0dist-portable"
if not exist "%OUTDIR%\" mkdir "%OUTDIR%"

set "OUT_RAW=%OUTDIR%\Marionette-uncompressed.exe"
set "OUT=%OUTDIR%\Marionette.exe"
copy /Y "%SRC%" "%OUT_RAW%" >nul
if errorlevel 1 goto fail
copy /Y "%SRC%" "%OUT%" >nul
if errorlevel 1 goto fail

for %%A in ("%OUT_RAW%") do set "RAW_BYTES=%%~zA"
set /a "RAW_KB=%RAW_BYTES%/1024"

if /I "%SKIP_UPX%"=="1" goto size_report
if not defined UPX_BIN goto size_report

rem Clear any accidental UPX option env that confuses upx.exe
set "UPX="
echo  [upx] compressing: %OUT%
"!UPX_BIN!" --best --lzma -q "%OUT%"
if errorlevel 1 (
  echo  [warn] UPX failed - leaving uncompressed
  set "UPX_NOTE=FAILED"
  copy /Y "%OUT_RAW%" "%OUT%" >nul
) else (
  set "UPX_NOTE=ok --best --lzma"
)

:size_report
for %%A in ("%OUT%") do set "BYTES=%%~zA"
set /a "KB=%BYTES%/1024"
set /a "MB_X10=%BYTES%*10/1024/1024"
set /a "MB_W=%MB_X10%/10"
set /a "MB_F=%MB_X10%%%10"
set /a "RAW_MB_X10=%RAW_BYTES%*10/1024/1024"
set /a "RAW_MB_W=%RAW_MB_X10%/10"
set /a "RAW_MB_F=%RAW_MB_X10%%%10"

echo.
echo  === Done ===
echo  Uncompressed: %OUT_RAW%
echo                %RAW_KB% KB (~%RAW_MB_W%.%RAW_MB_F% MB)
echo  Ship:         %OUT%
echo                %KB% KB (~%MB_W%.%MB_F% MB)
if defined UPX_NOTE echo  UPX:          %UPX_NOTE%
echo  TLS:          ring-less (ureq + native-tls / schannel)
echo.

if %BYTES% GEQ 10485760 (
  echo  [warn] Ship over 10 MB
) else if %BYTES% GEQ 3145728 (
  if /I "%UPX_NOTE:~0,2%"=="ok" (
    echo  [warn] Over 3 MB after UPX
  ) else (
    echo  [ok] Uncompressed under 10 MB. Install UPX for ~1.2 MB ship.
  )
) else (
  echo  [ok] Ship size in portable band (target ~1.2 MB with UPX)
)

echo.
echo  Copy dist-portable\Marionette.exe and run. Needs WebView2 Runtime.
echo.
if /I not "%NOPAUSE%"=="1" pause
exit /b 0

:fail
echo.
echo  Build failed.
if /I not "%NOPAUSE%"=="1" pause
exit /b 1

:find_upx
set "UPX_BIN="
where upx >nul 2>&1
if not errorlevel 1 (
  for /f "delims=" %%P in ('where upx 2^>nul') do (
    set "UPX_BIN=%%P"
    goto :eof
  )
)
if exist "%~dp0tools\upx.exe" set "UPX_BIN=%~dp0tools\upx.exe" & goto :eof
if exist "%~dp0tools\upx\upx.exe" set "UPX_BIN=%~dp0tools\upx\upx.exe" & goto :eof
if exist "%USERPROFILE%\bin\upx.exe" set "UPX_BIN=%USERPROFILE%\bin\upx.exe" & goto :eof
if exist "C:\tools\upx\upx.exe" set "UPX_BIN=C:\tools\upx\upx.exe" & goto :eof
for /f "delims=" %%P in ('dir /s /b "%LOCALAPPDATA%\Microsoft\WinGet\Packages\upx.exe" 2^>nul') do (
  set "UPX_BIN=%%P"
  goto :eof
)
goto :eof
