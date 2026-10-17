@echo off
setlocal EnableExtensions
title Marionette publish release

cd /d "%~dp0.."

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0publish-release.ps1" %*
if errorlevel 1 (
  echo.
  echo  [error] publish failed
  exit /b 1
)
exit /b 0
