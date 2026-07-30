@echo off
rem Find or install a usable MSVC environment for Tauri's Windows build.

:detect
where cl >nul 2>&1
if not errorlevel 1 exit /b 0

set "VCVARS="
if exist "%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if not defined VCVARS if exist "%ProgramFiles%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%ProgramFiles%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if not defined VCVARS if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if not defined VCVARS if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%ProgramFiles%\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat"
if not defined VCVARS if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%ProgramFiles%\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat"

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not defined VCVARS if exist "%VSWHERE%" (
  for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.VC.Tools.x86.x64 -property installationPath`) do if exist "%%i\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%%i\VC\Auxiliary\Build\vcvars64.bat"
)

if defined VCVARS (
  echo  [env] Loading MSVC vcvars64
  call "%VCVARS%" >nul 2>&1
)

where cl >nul 2>&1
if not errorlevel 1 exit /b 0

:missing
echo.
echo  [error] MSVC C++ Build Tools and Windows SDK are required for a first build.
echo          Select "Desktop development with C++" in Visual Studio Build Tools.
set "WINGET_AVAILABLE="
where winget >nul 2>&1
if not errorlevel 1 set "WINGET_AVAILABLE=1"

if not defined WINGET_AVAILABLE goto manual
if defined WINGET_ATTEMPTED goto manual
set "INSTALL_MSVC="
set /p "INSTALL_MSVC=  Install Build Tools automatically with winget? (Y/N): "
if /i not "%INSTALL_MSVC%"=="Y" goto manual

set "WINGET_ATTEMPTED=1"
echo  [winget] Installing Visual Studio Build Tools and Windows SDK ...
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements --override "--wait --quiet --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
set "WINGET_EXITCODE=%ERRORLEVEL%"
if "%WINGET_EXITCODE%"=="0" goto detect
if "%WINGET_EXITCODE%"=="3010" goto detect
echo  [error] winget installation failed. You can install manually from the official page.

:manual
set "OPEN_MSVC="
set /p "OPEN_MSVC=  Open the official download page? (Y/N): "
if /i "%OPEN_MSVC%"=="Y" start "" "https://visualstudio.microsoft.com/visual-cpp-build-tools/"
exit /b 1
