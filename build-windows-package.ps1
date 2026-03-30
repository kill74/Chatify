param(
  [string]$OutputDir = "dist",
  [string]$PackageName = "chatify-windows-x64",
  [switch]$SkipInstaller,
  [string]$IsccPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $RepoRoot

function Get-CargoVersion {
  param([string]$CargoTomlPath)

  $versionLine = Select-String -Path $CargoTomlPath -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
  if (-not $versionLine) {
    throw "Could not determine package version from $CargoTomlPath"
  }

  return $versionLine.Matches[0].Groups[1].Value
}

function Resolve-IsccPath {
  param([string]$ExplicitPath)

  if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
    if (Test-Path -Path $ExplicitPath) {
      return (Resolve-Path $ExplicitPath).Path
    }
    throw "Provided -IsccPath does not exist: $ExplicitPath"
  }

  $command = Get-Command iscc.exe -ErrorAction SilentlyContinue
  if ($command -and (Test-Path -Path $command.Source)) {
    return $command.Source
  }

  $candidates = @(
    (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
    (Join-Path $env:ProgramFiles "Inno Setup 6\\ISCC.exe"),
    (Join-Path $env:LOCALAPPDATA "Programs\\Inno Setup 6\\ISCC.exe")
  )

  foreach ($candidate in $candidates) {
    if (-not [string]::IsNullOrWhiteSpace($candidate) -and (Test-Path -Path $candidate)) {
      return $candidate
    }
  }

  return $null
}

$Version = Get-CargoVersion -CargoTomlPath (Join-Path $RepoRoot "Cargo.toml")

Write-Host "Building release binaries..."
cargo build --release --bin clicord-server --bin clicord-client
if ($LASTEXITCODE -ne 0) {
  throw "Build failed."
}

$PackageRoot = Join-Path $RepoRoot $OutputDir
$PackageDir = Join-Path $PackageRoot $PackageName
$ZipPath = Join-Path $PackageRoot ("{0}.zip" -f $PackageName)
$InstallerPath = Join-Path $PackageRoot ("chatify-setup-{0}.exe" -f $Version)

if (Test-Path $PackageDir) {
  Remove-Item -Recurse -Force $PackageDir
}
if (Test-Path $ZipPath) {
  Remove-Item -Force $ZipPath
}
if (Test-Path $InstallerPath) {
  Remove-Item -Force $InstallerPath
}
if (Test-Path "$InstallerPath.sha256") {
  Remove-Item -Force "$InstallerPath.sha256"
}

New-Item -ItemType Directory -Path $PackageDir -Force | Out-Null

Copy-Item (Join-Path $RepoRoot "target/release/clicord-server.exe") (Join-Path $PackageDir "clicord-server.exe")
Copy-Item (Join-Path $RepoRoot "target/release/clicord-client.exe") (Join-Path $PackageDir "clicord-client.exe")
Copy-Item (Join-Path $RepoRoot "LICENSE") (Join-Path $PackageDir "LICENSE")

$ServerBat = @"
@echo off
setlocal

cd /d "%~dp0"

set HOST=0.0.0.0
set PORT=8765

if not "%~1"=="" set HOST=%~1
if not "%~2"=="" set PORT=%~2

echo Starting Chatify server on %HOST%:%PORT%
clicord-server.exe --host %HOST% --port %PORT%
"@

$ClientBat = @"
@echo off
setlocal

cd /d "%~dp0"

set HOST=127.0.0.1
set PORT=8765

if not "%~1"=="" set HOST=%~1
if not "%~2"=="" set PORT=%~2

echo Starting Chatify client to %HOST%:%PORT%
clicord-client.exe --host %HOST% --port %PORT%
"@

$StartAllBat = @"
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
"@

$LauncherCmd = @"
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
"@

$ReadmeTxt = @"
Chatify Windows Package
=======================

Files:
- clicord-server.exe
- clicord-client.exe
- chatify-launcher.cmd
- start-chatify.bat
- start-server.bat
- start-client.bat

Quick Start:
1) Run chatify-launcher.cmd

Launcher modes:
- [1] Host on this machine (starts server + local client)
- [2] Join existing server (client only)

Manual mode:
1) Run start-server.bat
2) Run start-client.bat

Optional parameters:
- start-server.bat [host] [port]
- start-client.bat [host] [port]
- start-chatify.bat [host] [port]

Examples:
- start-server.bat 0.0.0.0 8765
- start-client.bat 127.0.0.1 8765

Integrity:
- chatify-windows-x64.zip.sha256 contains SHA256 for the ZIP.
"@

Set-Content -Path (Join-Path $PackageDir "start-server.bat") -Value $ServerBat -Encoding ASCII
Set-Content -Path (Join-Path $PackageDir "start-client.bat") -Value $ClientBat -Encoding ASCII
Set-Content -Path (Join-Path $PackageDir "start-chatify.bat") -Value $StartAllBat -Encoding ASCII
Set-Content -Path (Join-Path $PackageDir "chatify-launcher.cmd") -Value $LauncherCmd -Encoding ASCII
Set-Content -Path (Join-Path $PackageDir "README.txt") -Value $ReadmeTxt -Encoding ASCII

Compress-Archive -Path $PackageDir -DestinationPath $ZipPath -CompressionLevel Optimal

$ZipHash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$HashFilePath = "$ZipPath.sha256"
"$ZipHash  $([System.IO.Path]::GetFileName($ZipPath))" | Set-Content -Path $HashFilePath -Encoding ASCII

Write-Host "Package created: $ZipPath"
Write-Host "Checksum created: $HashFilePath"
Write-Host "Contents ready in: $PackageDir"

if ($SkipInstaller) {
  Write-Host "Installer build skipped (-SkipInstaller)."
  return
}

$CompilerPath = Resolve-IsccPath -ExplicitPath $IsccPath
if (-not $CompilerPath) {
  Write-Warning "ISCC.exe not found. Install Inno Setup 6 or pass -IsccPath to generate a Windows installer."
  return
}

$InstallerScript = Join-Path $RepoRoot "windows/chatify-installer.iss"
if (-not (Test-Path $InstallerScript)) {
  throw "Missing installer script: $InstallerScript"
}

Write-Host "Building installer with Inno Setup..."
& $CompilerPath "/DSourceDir=$PackageDir" "/DOutputDir=$PackageRoot" "/DMyAppVersion=$Version" $InstallerScript
if ($LASTEXITCODE -ne 0) {
  throw "Installer build failed."
}

if (-not (Test-Path $InstallerPath)) {
  throw "Installer was not generated at expected path: $InstallerPath"
}

$InstallerHash = (Get-FileHash -Path $InstallerPath -Algorithm SHA256).Hash.ToLowerInvariant()
$InstallerHashPath = "$InstallerPath.sha256"
"$InstallerHash  $([System.IO.Path]::GetFileName($InstallerPath))" | Set-Content -Path $InstallerHashPath -Encoding ASCII

Write-Host "Installer created: $InstallerPath"
Write-Host "Installer checksum created: $InstallerHashPath"
