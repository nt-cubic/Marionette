@echo off
setlocal EnableExtensions
title Marionette rebuild (quick)

cd /d "%~dp0.."

if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

set "EXE=%~dp0..\src-tauri\target\debug\marionette.exe"

echo.
echo  === Marionette quick rebuild ===
echo.

echo  [1/3] Stopping running Marionette instance (this repo only)...
rem Kill only instances whose exe lives under this repo - a portable build
rem from release\ or another checkout must survive a quick rebuild.
powershell -NoProfile -Command "$root=(Resolve-Path '%~dp0..').Path; Get-Process marionette -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($root,[StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force" >nul 2>&1
timeout /t 1 /nobreak >nul

echo  [2/3] Rebuilding debug binary (incremental)...
call "%~dp0ensure-msvc.bat"
if errorlevel 1 (
  echo.
  echo  BUILD ABORTED - MSVC / Windows SDK environment is incomplete.
  pause
  exit /b 1
)
pushd "%~dp0..\src-tauri"
call cargo build
set "BUILD=%ERRORLEVEL%"
popd
if not "%BUILD%"=="0" (
  echo.
  echo  BUILD FAILED - read the error above.
  pause
  exit /b 1
)

echo  [3/3] Restarting...
set "VITE_PID="
for /f "tokens=5" %%p in ('netstat -ano 2^>nul ^| findstr ":5173" ^| findstr "LISTENING"') do (
  if not defined VITE_PID set "VITE_PID=%%p"
)

if defined VITE_PID (
  echo  Vite still running ^(PID %VITE_PID%^) - starting marionette.exe only.
  start "" "%EXE%"
) else (
  echo  Vite not running - full quick launch.
  call "%~dp0start-marionette-quick.bat"
)

echo.
echo  Done. Window will stay open; close it anytime.
echo.
exit /b 0