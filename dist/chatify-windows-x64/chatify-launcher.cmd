@echo off
setlocal

cd /d "%~dp0"

set DEFAULT_HOST=127.0.0.1
set DEFAULT_PORT=8765

:menu
cls
echo ===================================
echo            Chatify Launcher
echo ===================================
echo [1] Host on this machine
echo [2] Join existing server
echo [Q] Quit
echo.
set /p CHOICE=Select mode: 

if /I "%CHOICE%"=="1" goto host
if /I "%CHOICE%"=="2" goto join
if /I "%CHOICE%"=="Q" exit /b 0

echo Invalid option.
timeout /t 1 /nobreak >nul
goto menu

:host
set PORT=%DEFAULT_PORT%
set /p PORT=Server port [%DEFAULT_PORT%]: 
if "%PORT%"=="" set PORT=%DEFAULT_PORT%

echo Starting server on 0.0.0.0:%PORT%
start "Chatify Server" cmd /k "cd /d %~dp0 && clicord-server.exe --host 0.0.0.0 --port %PORT%"
timeout /t 1 /nobreak >nul

echo Connecting local client to 127.0.0.1:%PORT%
start "Chatify Client" cmd /k "cd /d %~dp0 && clicord-client.exe --host 127.0.0.1 --port %PORT%"
exit /b 0

:join
set HOST=%DEFAULT_HOST%
set PORT=%DEFAULT_PORT%
set /p HOST=Server host/IP [%DEFAULT_HOST%]: 
if "%HOST%"=="" set HOST=%DEFAULT_HOST%
set /p PORT=Server port [%DEFAULT_PORT%]: 
if "%PORT%"=="" set PORT=%DEFAULT_PORT%

echo Connecting to %HOST%:%PORT%
clicord-client.exe --host %HOST% --port %PORT%
exit /b %ERRORLEVEL%
