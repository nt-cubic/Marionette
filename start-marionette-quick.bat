@echo off
setlocal EnableExtensions
title Marionette quick

cd /d "%~dp0"

if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
if exist "%APPDATA%\npm" set "PATH=%APPDATA%\npm;%PATH%"

set "EXE=%~dp0src-tauri\target\debug\marionette.exe"
if not exist "%EXE%" (
  echo  [info] No debug build yet - falling back to full tauri dev...
  call "%~dp0start-marionette.bat"
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
)

echo  Starting Vite on 127.0.0.1:5173 ...
start "Marionette Vite" /D "%~dp0" cmd /c "npm run dev"

timeout /t 2 /nobreak >nul

echo  Starting marionette.exe
start "" "%EXE%"

echo.
echo  Quick launch done.
echo  Keep the Vite window open for the UI.
echo  For full rebuild: start-marionette.bat
echo.
exit /b 0
