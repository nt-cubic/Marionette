@echo off
rem Dev helper: run marionette cargo tests with the MSVC env loaded.
call "%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
cd /d "%~dp0..\src-tauri"
cargo test --bin marionette %*
