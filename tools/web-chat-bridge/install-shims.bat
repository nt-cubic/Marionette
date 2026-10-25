@echo off
rem 安装网页聊天桥的命令垫片：把 web-chatgpt-acp / web-deepseek-acp 放进
rem %APPDATA%\npm（Marionette 的 agent 命令解析会找到这里）。一次性动作。
setlocal
set HERE=%~dp0
set DEST=%APPDATA%\npm
if not exist "%DEST%" mkdir "%DEST%"

rem 解析 node 绝对路径（垫片不依赖 PATH 里有 node）
set NODE_EXE=
for /f "delims=" %%I in ('where node 2^>nul') do (
  if not defined NODE_EXE set "NODE_EXE=%%I"
)
if not defined NODE_EXE (
  echo [web-chat-bridge] node.exe not found on PATH. Install Node.js first.
  exit /b 1
)
echo [web-chat-bridge] node = %NODE_EXE%

> "%DEST%\web-chatgpt-acp.cmd" echo @echo off
>> "%DEST%\web-chatgpt-acp.cmd" echo "%NODE_EXE%" "%HERE%bridge.mjs" chatgpt %%*

> "%DEST%\web-deepseek-acp.cmd" echo @echo off
>> "%DEST%\web-deepseek-acp.cmd" echo "%NODE_EXE%" "%HERE%bridge.mjs" deepseek %%*

echo [web-chat-bridge] installed:
echo   %DEST%\web-chatgpt-acp.cmd
echo   %DEST%\web-deepseek-acp.cmd
endlocal
