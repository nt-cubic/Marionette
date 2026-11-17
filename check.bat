@echo off
rem Engine-rust build helper: cargo check/build with the MSVC environment set.
rem Usage: check.bat [extra cargo args...]  (default: check --bin marionette-engine)
setlocal
cd /d "%~dp0src-tauri"
set "VCVARS="
for %%Y in (
  "%ProgramFiles(x86)%\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
) do if not defined VCVARS if exist %%Y set "VCVARS=%%~Y"
if not defined VCVARS (
  echo [check] no vcvars64 found & exit /b 1
)
call "%VCVARS%" >nul
if "%~1"=="" (
  cargo check --bin marionette-engine
) else (
  cargo %*
)
exit /b %ERRORLEVEL%
