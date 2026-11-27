@echo off
setlocal EnableExtensions
chcp 437 >nul
title Marionette doctor

set "REPORT=%USERPROFILE%\.marionette\logs\doctor-report.txt"
set "FALLBACK=%TEMP%\marionette-doctor.txt"
set "EXE="

rem 1) newest build in this repo (release\0.x.y\Marionette.exe)
for /f "delims=" %%D in ('dir /b /ad /o-d "%~dp0..\release\0.*" 2^>nul') do (
  if not defined EXE if exist "%~dp0..\release\%%D\Marionette.exe" set "EXE=%~dp0..\release\%%D\Marionette.exe"
)

rem 2) installed app (install.ps1 target)
if not defined EXE if exist "%LOCALAPPDATA%\Programs\Marionette\Marionette.exe" set "EXE=%LOCALAPPDATA%\Programs\Marionette\Marionette.exe"

rem 3) cargo build output
if not defined EXE if exist "%~dp0..\target\release\marionette.exe" set "EXE=%~dp0..\target\release\marionette.exe"

if not defined EXE (
  echo No Marionette.exe found. Build one first:  tools\build-portable.bat
  echo.
  pause
  exit /b 2
)

echo Checking: %EXE%
echo.

del /q "%REPORT%" 2>nul
del /q "%FALLBACK%" 2>nul

rem Optional flags: --fix (repair dead bindings / stale prefs), --json
set "FLAGS=%*"
"%EXE%" --doctor %FLAGS% >nul 2>&1

if exist "%REPORT%" (
  type "%REPORT%"
  goto done
)

if exist "%FALLBACK%" (
  type "%FALLBACK%"
  goto done
)

echo The doctor wrote no report at all.
echo.
echo That usually means the exe could not start or ~/.marionette is not writable.
echo Run  icacls "<repo>" /setintegritylevel (OI)(CI)Medium  and try again.

:done
echo.
pause
exit /b 0
