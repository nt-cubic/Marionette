@echo off
setlocal EnableExtensions
title Marionette version bump

cd /d "%~dp0"

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\bump-version.ps1" "%~1"
if errorlevel 1 (
  echo.
  echo  [error] version bump failed
  exit /b 1
)
exit /b 0
