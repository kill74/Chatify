@echo off
setlocal

cd /d "%~dp0"

set HOST=127.0.0.1
set PORT=8765

if not "%~1"=="" set HOST=%~1
if not "%~2"=="" set PORT=%~2

echo Launching Chatify server and client...
start "Chatify Server" cmd /k "cd /d %~dp0 && clicord-server.exe --host 0.0.0.0 --port %PORT%"
timeout /t 1 /nobreak >nul
start "Chatify Client" cmd /k "cd /d %~dp0 && clicord-client.exe --host %HOST% --port %PORT%"
