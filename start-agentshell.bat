@echo off
setlocal EnableExtensions
title AgentShell

cd /d "%~dp0"

echo.
echo  === AgentShell ===
echo  Project: %CD%
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
  echo  [env] MSVC vcvars not found - ok if binary already built
)

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
  echo  AgentShell exited with code %EXITCODE%.
  goto fail
)
echo  AgentShell stopped.
pause
exit /b 0

:fail
echo.
echo  Failed. Window stays open so you can read the error.
pause
exit /b 1
