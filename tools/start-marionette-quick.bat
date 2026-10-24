@echo off
setlocal EnableExtensions
title Marionette quick

cd /d "%~dp0.."

if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
if exist "%APPDATA%\npm" set "PATH=%APPDATA%\npm;%PATH%"

set "EXE=%~dp0..\src-tauri\target\debug\marionette.exe"
if not exist "%EXE%" (
  echo  [info] No debug build yet - falling back to full tauri dev...
  call "%~dp0..\start-marionette.bat"
  exit /b %ERRORLEVEL%
)

where node >nul 2>&1
if errorlevel 1 (
  echo  [error] node not found.
  pause
  exit /b 1
)

if not exist "node_modules\" (
  echo  [npm] installing dependencies...
  call npm install
  if errorlevel 1 (
    echo  [error] npm install failed - aborting.
    pause
    exit /b 1
  )
)

echo  Starting Vite on 127.0.0.1:5173 ...
start "Marionette Vite" /D "%~dp0.." cmd /c "npm run dev"

echo  [wait] polling for Vite on 127.0.0.1:5173 ...
set "VITE_UP="
for /l %%i in (1,1,30) do (
  if not defined VITE_UP (
    netstat -ano 2>nul | findstr ":5173" | findstr "LISTENING" >nul 2>&1 && set "VITE_UP=1"
    if not defined VITE_UP timeout /t 1 /nobreak >nul
  )
)
if not defined VITE_UP echo  [warn] Vite did not report LISTENING within 30s - starting the exe anyway.

echo  Starting marionette.exe
start "" "%EXE%"

echo.
echo  Quick launch done.
echo  Keep the Vite window open for the UI.
echo  For full rebuild: start-marionette.bat in the repo root
echo.
exit /b 0
