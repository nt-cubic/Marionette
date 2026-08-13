@echo off
setlocal EnableExtensions
title Marionette rebuild (quick)

cd /d "%~dp0"

if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

set "EXE=%~dp0src-tauri\target\debug\marionette.exe"

echo.
echo  === Marionette quick rebuild ===
echo.

echo  [1/3] Stopping running Marionette instance...
taskkill /IM marionette.exe /F >nul 2>&1
timeout /t 1 /nobreak >nul

echo  [2/3] Rebuilding debug binary (incremental)...
pushd "%~dp0src-tauri"
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