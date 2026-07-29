@echo off
setlocal EnableExtensions
title Marionette portable build

cd /d "%~dp0"

echo.
echo  === Marionette portable build ===
echo  Output: single .exe  (no installer)
echo  Target: under 10 MB
echo.

if exist "%USERPROFILE%\.cargo\bin" (
  set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
)
if exist "%APPDATA%\npm" (
  set "PATH=%APPDATA%\npm;%PATH%"
)

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
if defined VCVARS (
  echo  [env] Loading MSVC vcvars64
  call "%VCVARS%" >nul 2>&1
) else (
  echo  [env] MSVC vcvars not found - continuing; link may fail if cl is missing
)

where node >nul 2>&1
if errorlevel 1 (
  echo  [error] node not found on PATH.
  goto fail
)
where npm >nul 2>&1
if errorlevel 1 (
  echo  [error] npm not found on PATH.
  goto fail
)
where cargo >nul 2>&1
if errorlevel 1 (
  echo  [error] cargo not found on PATH. Install Rust and retry.
  goto fail
)

if not exist "package.json" (
  echo  [error] package.json missing - wrong folder?
  goto fail
)
if not exist "node_modules\" (
  echo  [npm] node_modules missing - running npm install...
  call npm install
  if errorlevel 1 goto fail
)

echo.
echo  Building release ^(first run with LTO can take several minutes^)...
echo.

call npm run build:portable
if errorlevel 1 goto fail

set "SRC=%~dp0src-tauri\target\release\marionette.exe"
if not exist "%SRC%" (
  echo  [error] Expected binary not found:
  echo          %SRC%
  goto fail
)

set "OUTDIR=%~dp0dist-portable"
if not exist "%OUTDIR%\" mkdir "%OUTDIR%"

set "OUT=%OUTDIR%\Marionette.exe"
copy /Y "%SRC%" "%OUT%" >nul
if errorlevel 1 goto fail

for %%A in ("%OUT%") do set "BYTES=%%~zA"
set /a "KB=%BYTES%/1024"
set /a "MB_X10=%BYTES%*10/1024/1024"
set /a "MB_WHOLE=%MB_X10%/10"
set /a "MB_FRAC=%MB_X10%%%10"

echo.
echo  === Done ===
echo  File:  %OUT%
echo  Size:  %KB% KB  ^(~%MB_WHOLE%.%MB_FRAC% MB^)
echo.
if %BYTES% GEQ 10485760 (
  echo  [warn] Over 10 MB a€? product claim is "under 10MB". Investigate deps / features.
) else (
  echo  [ok] Under 10 MB.
)
echo.
echo  Copy Marionette.exe anywhere and double-click.
echo  Needs WebView2 Runtime ^(Win10/11 usually already have it^).
echo  If missing, the app shows a dialog with the Microsoft download link.
echo.
pause
exit /b 0

:fail
echo.
echo  Build failed. Window stays open so you can read the error.
pause
exit /b 1
