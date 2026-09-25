@echo off
setlocal EnableExtensions
title Marionette demo instance

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0demo-instance.ps1" %*
if errorlevel 1 (
  echo.
  echo  Demo instance failed. Window stays open so you can read the error.
  pause
)
exit /b %ERRORLEVEL%
