@echo off
rem Marionette dev launcher, in-process Rust engine SKU (feature engine-rust).
rem Same as start-marionette.bat, only the engine differs.
set "MARIONETTE_ENGINE=rust"
call "%~dp0start-marionette.bat"
