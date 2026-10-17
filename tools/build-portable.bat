@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Marionette portable build

cd /d "%~dp0.."

rem -- variants: default = both (plain + engine). --plain / --engine for one. --
set "BUILD_PLAIN="
set "BUILD_ENGINE="
if "%~1"=="" (
  set "BUILD_PLAIN=1"
  set "BUILD_ENGINE=1"
) else if /I "%~1"=="--both" (
  set "BUILD_PLAIN=1"
  set "BUILD_ENGINE=1"
) else if /I "%~1"=="--engine" (
  set "BUILD_ENGINE=1"
) else if /I "%~1"=="--plain" (
  set "BUILD_PLAIN=1"
) else if /I "%~1"=="--help" (
  echo.
  echo  Usage: tools\build-portable.bat [--both ^| --plain ^| --engine]
  echo.
  echo  Default --both writes two GitHub Release assets into dist-portable\:
  echo    Marionette-^<ver^>-windows-x64.exe
  echo    Marionette-Engine-^<ver^>-windows-x64.exe
  echo  plus local copies Marionette.exe / Marionette-Engine.exe.
  echo.
  echo  Env: SKIP_UPX=1  REQUIRE_UPX=1  INSTALL_UPX=1  NOPAUSE=1
  echo.
  exit /b 0
) else (
  echo  [error] unknown argument: %~1
  echo          use --both, --plain, or --engine
  goto fail
)

echo.
echo  === Marionette portable build ===
if defined BUILD_PLAIN if defined BUILD_ENGINE (
  echo  Variants: PLAIN + ENGINE  ^(GitHub Release pair^)
) else if defined BUILD_ENGINE (
  echo  Variant: ENGINE-EMBEDDED ^(third_party\magpie, offline install^)
) else (
  echo  Variant: plain ^(engine downloads on demand^)
)
echo  Single exe ^| ring-less TLS ^(schannel^)
echo.
echo  Env: SKIP_UPX=1  REQUIRE_UPX=1  INSTALL_UPX=1
echo.

rem -- PATH: cargo / npm --
if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
if exist "%APPDATA%\npm" set "PATH=%APPDATA%\npm;%PATH%"

call "%~dp0ensure-msvc.bat"
if errorlevel 1 goto fail

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
call "%~dp0ensure-rust.bat"
if errorlevel 1 goto fail

if not exist "package.json" (
  echo  [error] package.json missing
  goto fail
)
if not exist "node_modules\" (
  echo  [npm] npm install...
  call npm install
  if errorlevel 1 goto fail
)

rem -- app version for GitHub asset names --
set "APP_VER="
for /f "usebackq delims=" %%V in (`powershell -NoProfile -Command "(Get-Content -Raw -LiteralPath 'package.json' | ConvertFrom-Json).version"`) do set "APP_VER=%%V"
if not defined APP_VER (
  echo  [error] could not read version from package.json
  goto fail
)
echo  [ver]  %APP_VER%

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

if defined BUILD_ENGINE (
  call :prepare_embed
  if errorlevel 1 goto fail
)

set "SRC=%~dp0..\src-tauri\target\release\marionette.exe"
set "OUTDIR=%~dp0..\dist-portable"
if not exist "%OUTDIR%\" mkdir "%OUTDIR%"

if defined BUILD_PLAIN (
  echo.
  echo  --- variant: plain ---
  call :ship_variant plain
  if errorlevel 1 goto fail
)
if defined BUILD_ENGINE (
  echo.
  echo  --- variant: engine ---
  call :ship_variant engine
  if errorlevel 1 goto fail
)

echo.
echo  === Done ===
echo  TLS:          ring-less (ureq + native-tls / schannel)
if defined UPX_NOTE echo  UPX:          %UPX_NOTE%
if defined BUILD_PLAIN (
  echo.
  echo  Plain:
  echo    Ship:         %OUTDIR%\Marionette.exe
  echo                  !PLAIN_KB! KB ^(~!PLAIN_MB_W!.!PLAIN_MB_F! MB^)
  echo    Release:      %OUTDIR%\Marionette-%APP_VER%-windows-x64.exe
  echo    Uncompressed: %OUTDIR%\Marionette-uncompressed.exe  !PLAIN_RAW_KB! KB
  if !PLAIN_BYTES! GEQ 10485760 (
    echo    [warn] Ship over 10 MB
  ) else if !PLAIN_BYTES! GEQ 3145728 (
    echo    [warn] Over 3 MB after UPX
  ) else (
    echo    [ok] Portable band
  )
)
if defined BUILD_ENGINE (
  echo.
  echo  Engine:
  echo    Ship:         %OUTDIR%\Marionette-Engine.exe
  echo                  !ENGINE_KB! KB ^(~!ENGINE_MB_W!.!ENGINE_MB_F! MB^)
  echo    Release:      %OUTDIR%\Marionette-Engine-%APP_VER%-windows-x64.exe
  echo    Uncompressed: %OUTDIR%\Marionette-Engine-uncompressed.exe  !ENGINE_RAW_KB! KB
  echo    [ok] Embedded-engine build includes the UPX-packed magpie
)
echo.
echo  Copy the exe you want and run. Needs WebView2 Runtime.
echo  Upload both Release files with tools\publish-release.bat
echo.
if /I not "%NOPAUSE%"=="1" pause
exit /b 0

:fail
echo.
echo  Build failed.
if /I not "%NOPAUSE%"=="1" pause
exit /b 1

:prepare_embed
echo.
echo  [engine] building nogui magpie from third_party\magpie ...
if not exist "third_party\magpie\main.go" (
  echo  [error] third_party\magpie missing - first run: powershell -File tools\import-magpie.ps1
  exit /b 1
)
set "GO_BIN="
where go >nul 2>&1
if not errorlevel 1 (
  for /f "delims=" %%P in ('where go 2^>nul') do (
    set "GO_BIN=%%P"
    goto go_found
  )
)
if exist "C:\Program Files\Go\bin\go.exe" (
  set "GO_BIN=C:\Program Files\Go\bin\go.exe"
  goto go_found
)
echo  [error] Go toolchain not found - winget install -e --id GoLang.Go
exit /b 1

:go_found
echo  [engine] go: !GO_BIN!
set "MAG_VER="
for /f "usebackq delims=" %%V in ("third_party\magpie\VERSION") do set "MAG_VER=%%V"
if not defined MAG_VER (
  echo  [error] third_party\magpie\VERSION missing - re-run tools\import-magpie.ps1
  exit /b 1
)
if not defined UPX_BIN (
  echo  [error] engine variant needs UPX to pack the embedded magpie ^(winget install -e --id UPX.UPX^)
  exit /b 1
)
if not exist "src-tauri\engine-embedded" mkdir "src-tauri\engine-embedded"
pushd "third_party\magpie"
set "CGO_ENABLED=0"
"!GO_BIN!" build -tags nogui -trimpath -ldflags "-s -w -X main.version=!MAG_VER!" -o "..\..\src-tauri\engine-embedded\magpie-windows-amd64.exe" .
if errorlevel 1 (
  popd
  echo  [hint] module download failures: set GOPROXY=https://goproxy.cn,direct and retry
  exit /b 1
)
popd
rem UPX the engine itself (2026-10-17 measured: 47.9MB -^> ~13.6MB, 5MB better than
rem gzip and no decode step at runtime - the packed exe self-extracts on launch).
echo  [engine] upx-packing embedded magpie ...
"!UPX_BIN!" --best --lzma -q "src-tauri\engine-embedded\magpie-windows-amd64.exe"
if errorlevel 1 (
  echo  [error] UPX failed on the embedded magpie
  exit /b 1
)
copy /Y "third_party\magpie\VERSION" "src-tauri\engine-embedded\VERSION" >nul
echo  [engine] embedded artifact ready: version !MAG_VER!
exit /b 0

:ship_variant
set "VARIANT=%~1"
if /I "%VARIANT%"=="engine" (
  set "SHIP_NAME=Marionette-Engine.exe"
  set "SHIP_RAW=Marionette-Engine-uncompressed.exe"
  set "SHIP_REL=Marionette-Engine-%APP_VER%-windows-x64.exe"
) else (
  set "SHIP_NAME=Marionette.exe"
  set "SHIP_RAW=Marionette-uncompressed.exe"
  set "SHIP_REL=Marionette-%APP_VER%-windows-x64.exe"
)

echo  [build] tauri release (LTO+strip+opt-z^) - %VARIANT%
echo.
if /I "%VARIANT%"=="engine" (
  call npm run build:portable -- --features embedded-engine
) else (
  call npm run build:portable
)
if errorlevel 1 exit /b 1

if not exist "%SRC%" (
  echo  [error] binary missing: %SRC%
  exit /b 1
)

copy /Y "%SRC%" "%OUTDIR%\%SHIP_RAW%" >nul
if errorlevel 1 exit /b 1
copy /Y "%SRC%" "%OUTDIR%\%SHIP_REL%" >nul
if errorlevel 1 (
  echo  [error] could not write %SHIP_REL%
  exit /b 1
)

for %%A in ("%OUTDIR%\%SHIP_RAW%") do set "RAW_BYTES=%%~zA"

if /I "%SKIP_UPX%"=="1" goto after_upx_ship
if not defined UPX_BIN goto after_upx_ship

rem Clear any accidental UPX option env that confuses upx.exe
set "UPX="
echo  [upx] compressing: %OUTDIR%\%SHIP_REL%
"!UPX_BIN!" --best --lzma -q "%OUTDIR%\%SHIP_REL%"
if errorlevel 1 (
  echo  [warn] UPX failed - leaving uncompressed
  set "UPX_NOTE=FAILED"
  copy /Y "%OUTDIR%\%SHIP_RAW%" "%OUTDIR%\%SHIP_REL%" >nul
) else (
  set "UPX_NOTE=ok --best --lzma"
)

:after_upx_ship
copy /Y "%OUTDIR%\%SHIP_REL%" "%OUTDIR%\%SHIP_NAME%" >nul
if errorlevel 1 (
  echo  [warn] %SHIP_NAME% is in use - Release asset is still %SHIP_REL%
)

for %%A in ("%OUTDIR%\%SHIP_REL%") do set "BYTES=%%~zA"
set /a "KB=%BYTES%/1024"
set /a "MB_X10=%BYTES%*10/1024/1024"
set /a "MB_W=%MB_X10%/10"
set /a "MB_F=%MB_X10%%%10"
set /a "RAW_KB=%RAW_BYTES%/1024"

if /I "%VARIANT%"=="engine" (
  set "ENGINE_BYTES=%BYTES%"
  set "ENGINE_KB=%KB%"
  set "ENGINE_MB_W=%MB_W%"
  set "ENGINE_MB_F=%MB_F%"
  set "ENGINE_RAW_KB=%RAW_KB%"
) else (
  set "PLAIN_BYTES=%BYTES%"
  set "PLAIN_KB=%KB%"
  set "PLAIN_MB_W=%MB_W%"
  set "PLAIN_MB_F=%MB_F%"
  set "PLAIN_RAW_KB=%RAW_KB%"
)
echo  [ship] %SHIP_NAME%  %KB% KB
echo  [ship] %SHIP_REL%
exit /b 0

:find_upx
set "UPX_BIN="
where upx >nul 2>&1
if not errorlevel 1 (
  for /f "delims=" %%P in ('where upx 2^>nul') do (
    set "UPX_BIN=%%P"
    goto :eof
  )
)
if exist "%~dp0upx.exe" set "UPX_BIN=%~dp0upx.exe" & goto :eof
if exist "%~dp0upx\upx.exe" set "UPX_BIN=%~dp0upx\upx.exe" & goto :eof
if exist "%USERPROFILE%\bin\upx.exe" set "UPX_BIN=%USERPROFILE%\bin\upx.exe" & goto :eof
if exist "C:\tools\upx\upx.exe" set "UPX_BIN=C:\tools\upx\upx.exe" & goto :eof
for /f "delims=" %%P in ('dir /s /b "%LOCALAPPDATA%\Microsoft\WinGet\Packages\upx.exe" 2^>nul') do (
  set "UPX_BIN=%%P"
  goto :eof
)
goto :eof
