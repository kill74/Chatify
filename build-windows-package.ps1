param(
  [string]$OutputDir = "dist",
  [string]$PackageName = "chatify-windows-x64"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $RepoRoot

Write-Host "Building release binaries..."
cargo build --release --bin clicord-server --bin clicord-client
if ($LASTEXITCODE -ne 0) {
  throw "Build failed."
}

$PackageRoot = Join-Path $RepoRoot $OutputDir
$PackageDir = Join-Path $PackageRoot $PackageName
$ZipPath = Join-Path $PackageRoot ("{0}.zip" -f $PackageName)

if (Test-Path $PackageDir) {
  Remove-Item -Recurse -Force $PackageDir
}
if (Test-Path $ZipPath) {
  Remove-Item -Force $ZipPath
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

$ReadmeTxt = @"
Chatify Windows Package
=======================

Files:
- clicord-server.exe
- clicord-client.exe
- start-server.bat
- start-client.bat

Quick Start:
1) Run start-server.bat
2) Run start-client.bat

Optional parameters:
- start-server.bat [host] [port]
- start-client.bat [host] [port]

Examples:
- start-server.bat 0.0.0.0 8765
- start-client.bat 127.0.0.1 8765
"@

Set-Content -Path (Join-Path $PackageDir "start-server.bat") -Value $ServerBat -Encoding ASCII
Set-Content -Path (Join-Path $PackageDir "start-client.bat") -Value $ClientBat -Encoding ASCII
Set-Content -Path (Join-Path $PackageDir "README.txt") -Value $ReadmeTxt -Encoding ASCII

Compress-Archive -Path $PackageDir -DestinationPath $ZipPath -CompressionLevel Optimal

Write-Host "Package created: $ZipPath"
Write-Host "Contents ready in: $PackageDir"
