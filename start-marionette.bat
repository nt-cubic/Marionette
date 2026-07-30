@echo off
setlocal EnableExtensions
title Marionette

cd /d "%~dp0"

echo.
echo  === Marionette ===
echo  Project: %CD%
echo.

if exist "%USERPROFILE%\.cargo\bin" (
  set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
)
if exist "%APPDATA%\npm" (
  set "PATH=%APPDATA%\npm;%PATH%"
)

call "%~dp0ensure-msvc.bat"
if errorlevel 1 goto fail

where node >nul 2>&1
if errorlevel 1 (
  echo  [error] node not found on PATH. Install Node.js and retry.
  goto fail
)
where npm >nul 2>&1
if errorlevel 1 (
  echo  [error] npm not found on PATH.
  goto fail
)

call "%~dp0ensure-rust.bat"
if errorlevel 1 goto fail

if not exist "package.json" (
  echo  [error] package.json missing - wrong folder?
  goto fail
)
if not exist "node_modules\" (
  echo  [npm] node_modules missing - running npm install...
  call npm install
  if errorlevel 1 goto fail
)

for /f "tokens=5" %%p in ('netstat -ano 2^>nul ^| findstr ":5173" ^| findstr "LISTENING"') do (
  echo  [clean] Stopping process on port 5173 PID=%%p
  taskkill /F /PID %%p >nul 2>&1
)

echo.
echo  Starting: npm run tauri -- dev
echo  First run or Rust changes may take a while.
echo  Close this window to stop the app.
echo.

call npm run tauri -- dev
set "EXITCODE=%ERRORLEVEL%"

echo.
if not "%EXITCODE%"=="0" (
  echo  Marionette exited with code %EXITCODE%.
  goto fail
)
echo  Marionette stopped.
pause
exit /b 0

:fail
echo.
echo  Failed. Window stays open so you can read the error.
pause
exit /b 1
