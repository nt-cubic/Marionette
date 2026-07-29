@echo off
setlocal EnableExtensions
cd /d "%~dp0..\src-tauri"
if exist "%USERPROFILE%\.cargo\bin" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

set "OUT=%~dp0cargo-out.txt"
echo === cargo verify started %DATE% %TIME% === > "%OUT%"
echo cwd=%CD% >> "%OUT%"
where cargo >> "%OUT%" 2>&1
echo. >> "%OUT%"

echo === 1) cargo test --bin marionette elicitation -- --nocapture === >> "%OUT%"
cargo test --bin marionette elicitation -- --nocapture >> "%OUT%" 2>&1
set "EC1=%ERRORLEVEL%"
echo. >> "%OUT%"
echo elicitation_test_exit_code=%EC1% >> "%OUT%"
echo. >> "%OUT%"

echo === 2) cargo test --bin marionette exit_plan_mode -- --nocapture === >> "%OUT%"
cargo test --bin marionette exit_plan_mode -- --nocapture >> "%OUT%" 2>&1
set "EC2=%ERRORLEVEL%"
echo. >> "%OUT%"
echo exit_plan_mode_test_exit_code=%EC2% >> "%OUT%"
echo. >> "%OUT%"

echo === 3) cargo check --bin marionette === >> "%OUT%"
cargo check --bin marionette >> "%OUT%" 2>&1
set "EC3=%ERRORLEVEL%"
echo. >> "%OUT%"
echo check_exit_code=%EC3% >> "%OUT%"
echo === cargo verify finished %DATE% %TIME% === >> "%OUT%"

echo Wrote %OUT%
echo elicitation_test_exit_code=%EC1%
echo exit_plan_mode_test_exit_code=%EC2%
echo check_exit_code=%EC3%
exit /b %EC3%
