@echo off
setlocal EnableExtensions
cd /d "%~dp0..\src-tauri"
if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

set "OUT=%~dp0cargo-verify-out.txt"
echo === cargo verify started %DATE% %TIME% === > "%OUT%"
echo cwd=%CD% >> "%OUT%"
echo. >> "%OUT%"

echo === cargo test --bin marionette exit_plan_mode -- --nocapture === >> "%OUT%"
cargo test --bin marionette exit_plan_mode -- --nocapture >> "%OUT%" 2>&1
set "TEST_EC=%ERRORLEVEL%"
echo. >> "%OUT%"
echo test_exit_code=%TEST_EC% >> "%OUT%"
echo. >> "%OUT%"

echo === cargo check --bin marionette === >> "%OUT%"
cargo check --bin marionette >> "%OUT%" 2>&1
set "CHECK_EC=%ERRORLEVEL%"
echo. >> "%OUT%"
echo check_exit_code=%CHECK_EC% >> "%OUT%"
echo === cargo verify finished %DATE% %TIME% === >> "%OUT%"

echo Wrote %OUT%
echo test_exit_code=%TEST_EC%
echo check_exit_code=%CHECK_EC%
exit /b %CHECK_EC%
