@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Marionette portable build

cd /d "%~dp0.."

if /I "%~1"=="--help" (
  echo.
  echo  Usage: tools\build-portable.bat
  echo.
  echo  Writes release\^<version^>\Marionette.exe ^(embedded magpie, UPX^).
  echo  Env: SKIP_UPX=1  REQUIRE_UPX=1  INSTALL_UPX=1  NOPAUSE=1
  echo       SKIP_MAGPIE_IMPORT=1  ^(reuse third_party\magpie as-is^)
  echo       MAGPIE_TAG=v0.1.xxxx  ^(pin engine seed; default = GitHub latest stable^)
  echo.
  exit /b 0
)
if not "%~1"=="" (
  echo  [error] unknown argument: %~1
  echo          this script takes no flags; see --help
  goto fail
)

echo.
echo  === Marionette portable build ===
echo  Single exe with embedded engine
echo  TLS: ring-less ^(schannel^)
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

rem -- find UPX (embedded magpie always needs it; SKIP_UPX only skips the ship exe) --
set "UPX_BIN="
set "UPX_NOTE="

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
  set "UPX_NOTE=MISSING - ship stays uncompressed"
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

call :prepare_embed
if errorlevel 1 goto fail

set "SRC=%~dp0..\src-tauri\target\release\marionette.exe"
rem Per-version output dir: a running old build can never lock this one.
set "OUTDIR=%~dp0..\release\%APP_VER%"
if not exist "%OUTDIR%\" mkdir "%OUTDIR%"

echo.
echo  [build] tauri release (LTO+strip+opt-z, embedded-engine)
echo.
call npm run build:portable -- --features embedded-engine
if errorlevel 1 goto fail

if not exist "%SRC%" (
  echo  [error] binary missing: %SRC%
  goto fail
)

rem -- drop leftover names from the old dual-SKU layout --
del /q "%OUTDIR%\Marionette-uncompressed.exe" >nul 2>&1
del /q "%OUTDIR%\Marionette-Engine.exe" >nul 2>&1
del /q "%OUTDIR%\Marionette-Engine-uncompressed.exe" >nul 2>&1
del /q "%OUTDIR%\Marionette.new.exe" >nul 2>&1
del /q "%OUTDIR%\Marionette-*-windows-x64.exe" >nul 2>&1
del /q "%OUTDIR%\Marionette-Engine-*.exe" >nul 2>&1

set "STAGE=%OUTDIR%\_staging.exe"
set "SHIP=%OUTDIR%\Marionette.exe"
copy /Y "%SRC%" "%STAGE%" >nul
if errorlevel 1 (
  echo  [error] could not write staging copy
  goto fail
)

if /I "%SKIP_UPX%"=="1" goto after_upx_ship
if not defined UPX_BIN goto after_upx_ship

set "UPX="
echo  [upx] compressing: %SHIP%
"!UPX_BIN!" --best --lzma -q "%STAGE%"
if errorlevel 1 (
  if /I "%REQUIRE_UPX%"=="1" (
    echo  [error] UPX failed and REQUIRE_UPX=1 - refusing to ship uncompressed
    goto fail
  )
  echo  [warn] UPX failed - leaving uncompressed
  set "UPX_NOTE=FAILED"
  copy /Y "%SRC%" "%STAGE%" >nul
) else (
  set "UPX_NOTE=ok --best --lzma"
)

:after_upx_ship
copy /Y "%STAGE%" "%SHIP%" >nul
if errorlevel 1 (
  echo  [warn] Marionette.exe is in use - wrote Marionette.new.exe instead
  echo         close the running app and rename it
  copy /Y "%STAGE%" "%OUTDIR%\Marionette.new.exe" >nul 2>&1
  if errorlevel 1 (
    echo  [error] could not ship the build at all - both targets failed
    goto fail
  )
  set "SHIP=%OUTDIR%\Marionette.new.exe"
  del /q "%STAGE%" >nul 2>&1
  echo  [error] Marionette.exe was in use - a Marionette.new.exe was left behind
  if /I "%REQUIRE_UPX%"=="1" goto fail
  if /I not "%NOPAUSE%"=="1" pause
  exit /b 1
)
del /q "%STAGE%" >nul 2>&1

for %%A in ("%SHIP%") do set "BYTES=%%~zA"
set /a "KB=%BYTES%/1024"
set /a "MB_X10=%BYTES%*10/1024/1024"
set /a "MB_W=%MB_X10%/10"
set /a "MB_F=%MB_X10%%%10"

echo.
echo  === Done ===
echo  TLS:          ring-less (ureq + native-tls / schannel)
if defined UPX_NOTE echo  UPX:          %UPX_NOTE%
echo  Ship:         %SHIP%
echo                %KB% KB ^(~%MB_W%.%MB_F% MB^)
echo.
echo  Copy that exe and run. Needs WebView2 Runtime.
echo  Engine starts on launch. Publish with tools\publish-release.bat
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
if /I "%SKIP_MAGPIE_IMPORT%"=="1" (
  echo  [engine] SKIP_MAGPIE_IMPORT=1 — keeping third_party\magpie as-is
) else (
  echo  [engine] importing latest stable magpie into third_party\magpie ...
  if defined MAGPIE_TAG (
    echo  [engine] MAGPIE_TAG=!MAGPIE_TAG!
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0import-magpie.ps1" -Tag "!MAGPIE_TAG!"
  ) else (
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0import-magpie.ps1"
  )
  if errorlevel 1 (
    echo  [error] import-magpie.ps1 failed — set SKIP_MAGPIE_IMPORT=1 to reuse existing vendor, or MAGPIE_TAG=v... to pin
    exit /b 1
  )
)
echo  [engine] building nogui magpie from third_party\magpie ...
if not exist "third_party\magpie\main.go" (
  echo  [error] third_party\magpie missing - run: powershell -File tools\import-magpie.ps1
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
  echo  [error] embedded magpie needs UPX ^(winget install -e --id UPX.UPX^)
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
rem Release hygiene: the baked seed must equal what the repo vendors. If the import
rem pulled a newer tag, commit third_party\magpie before publishing so the release
rem commit and the shipped exe tell the same story.
git -C "%~dp0.." diff --quiet -- third_party\magpie 2>nul
if errorlevel 1 echo  [engine] note: third_party\magpie differs from HEAD - commit it so the release matches this build
echo  [engine] embedded artifact ready: version !MAG_VER!
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
