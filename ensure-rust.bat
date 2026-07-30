@echo off
rem Check for Rust and optionally install the official stable toolchain.

if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

where cargo >nul 2>&1
if not errorlevel 1 (
  where rustc >nul 2>&1
  if not errorlevel 1 exit /b 0
)

echo.
echo  [info] Rust/Cargo is required to build the Tauri backend.
set "INSTALL_RUST="
set /p "INSTALL_RUST=  Install Rust stable via rustup? (Y/N): "
if /i not "%INSTALL_RUST%"=="Y" (
  echo  [error] Rust is required. Install it manually from https://rustup.rs
  exit /b 1
)

set "POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%POWERSHELL%" (
  echo  [error] Windows PowerShell was not found. Install Rust manually from https://rustup.rs
  exit /b 1
)

rem rustup-init determines its mode from its executable name; keep the official name.
set "RUSTUP_EXE=%TEMP%\rustup-init.exe"
if exist "%RUSTUP_EXE%" del /q "%RUSTUP_EXE%" >nul 2>&1

echo  [rustup] Downloading rustup-init.exe ...
"%POWERSHELL%" -NoProfile -ExecutionPolicy Bypass -Command "Invoke-WebRequest -UseBasicParsing -Uri 'https://win.rustup.rs/x86_64' -OutFile '%RUSTUP_EXE%'"
if errorlevel 1 (
  echo  [error] Rust download failed. Check your internet connection or install manually from https://rustup.rs
  exit /b 1
)
if not exist "%RUSTUP_EXE%" (
  echo  [error] Rust installer was not downloaded.
  exit /b 1
)

echo  [rustup] Installing Rust stable toolchain ...
"%RUSTUP_EXE%" -y --default-toolchain stable
set "RUSTUP_EXITCODE=%ERRORLEVEL%"
del /q "%RUSTUP_EXE%" >nul 2>&1
if not "%RUSTUP_EXITCODE%"=="0" (
  echo  [error] Rust installation failed with code %RUSTUP_EXITCODE%.
  exit /b 1
)

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>&1
if errorlevel 1 (
  echo  [error] Cargo was not found after installation.
  echo          Restart this script after checking https://rustup.rs
  exit /b 1
)
where rustc >nul 2>&1
if errorlevel 1 (
  echo  [error] Rust compiler was not found after installation.
  exit /b 1
)

echo  [rustup] Rust is ready.
exit /b 0
