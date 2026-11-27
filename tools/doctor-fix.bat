@echo off
rem Same check as doctor.bat, but repairs the two data-class findings first:
rem dead bridge bindings and saved shared-model choices the engine no longer
rem lists. Close Marionette before running this (the doctor refuses otherwise).
call "%~dp0doctor.bat" --fix
