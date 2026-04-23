<#
.SYNOPSIS
Starts a clean local Chatify demo server and prints a two-terminal walkthrough.

.DESCRIPTION
The demo uses a temporary SQLite database and a generated DB key under the
system temp directory. Nothing is written to the repository. Keep this terminal
open for the server, then use the printed commands from another terminal to
connect one or two clients.

.PARAMETER Port
Port for the demo server. Use 0 to ask the OS for a free port.

.PARAMETER Release
Run release binaries with cargo instead of dev binaries.

.PARAMETER DryRun
Print the server/client commands without launching the server.

.EXAMPLE
.\scripts\demo-local.ps1

.EXAMPLE
.\scripts\demo-local.ps1 -Port 0 -Release
#>
[CmdletBinding()]
param(
  [ValidateRange(0, 65535)]
  [int]$Port = 8765,

  [switch]$Release,
  [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$ProfileArg = if ($Release) { "--release" } else { $null }

function Assert-CommandAvailable {
  param([Parameter(Mandatory = $true)][string]$CommandName)

  if (-not (Get-Command $CommandName -ErrorAction SilentlyContinue)) {
    throw "Required command '$CommandName' was not found in PATH."
  }
}

function Get-FreeTcpPort {
  $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
  try {
    $listener.Start()
    return $listener.LocalEndpoint.Port
  }
  finally {
    $listener.Stop()
  }
}

function Test-TcpPortAvailable {
  param([Parameter(Mandatory = $true)][int]$CandidatePort)

  $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $CandidatePort)
  try {
    $listener.Start()
    return $true
  }
  catch {
    return $false
  }
  finally {
    $listener.Stop()
  }
}

function New-HexKey {
  $bytes = New-Object byte[] 32
  $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
  try {
    $rng.GetBytes($bytes)
  }
  finally {
    $rng.Dispose()
  }
  return -join ($bytes | ForEach-Object { $_.ToString("x2") })
}

function Join-Args {
  param([Parameter(Mandatory = $true)][string[]]$Args)

  return ($Args | ForEach-Object {
      if ($_ -match '\s') {
        "'$($_ -replace "'", "''")'"
      }
      else {
        $_
      }
    }) -join " "
}

Assert-CommandAvailable cargo

$selectedPort = if ($Port -eq 0) { Get-FreeTcpPort } else { $Port }
if ($Port -ne 0 -and -not (Test-TcpPortAvailable -CandidatePort $selectedPort)) {
  throw "Port $selectedPort is already in use. Re-run with -Port 0 or choose another port."
}

$demoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("chatify-demo-" + [Guid]::NewGuid().ToString("N"))
$dbPath = Join-Path $demoRoot "chatify-demo.db"
$dbKey = New-HexKey

$serverArgs = @("run")
if ($ProfileArg) {
  $serverArgs += $ProfileArg
}
$serverArgs += @(
  "-p", "chatify-server",
  "--bin", "chatify-server",
  "--",
  "--host", "127.0.0.1",
  "--port", "$selectedPort",
  "--db", $dbPath,
  "--db-key", $dbKey,
  "--db-durability", "balanced"
)

$clientArgs = @("run")
if ($ProfileArg) {
  $clientArgs += $ProfileArg
}
$clientArgs += @(
  "-p", "chatify-client",
  "--bin", "chatify-client",
  "--",
  "--host", "127.0.0.1",
  "--port", "$selectedPort"
)

Write-Host ""
Write-Host "Chatify local demo" -ForegroundColor Cyan
Write-Host "Repo: $RepoRoot"
Write-Host "Temp DB: $dbPath"
Write-Host "Port: $selectedPort"
Write-Host ""
Write-Host "Terminal 1 - server:" -ForegroundColor Yellow
Write-Host ("  cargo " + (Join-Args -Args $serverArgs))
Write-Host ""
Write-Host "Terminal 2 - first client:" -ForegroundColor Yellow
Write-Host ("  cargo " + (Join-Args -Args $clientArgs))
Write-Host ""
Write-Host "Terminal 3 - optional second client:" -ForegroundColor Yellow
Write-Host ("  cargo " + (Join-Args -Args $clientArgs))
Write-Host ""
Write-Host "Try these in the client:" -ForegroundColor Yellow
Write-Host "  /join general"
Write-Host "  hello from the local demo"
Write-Host "  /history #general 20"
Write-Host "  /search #general demo limit=10"
Write-Host "  /fingerprint bob"
Write-Host "  /doctor"
Write-Host ""

if ($DryRun) {
  Write-Host "Dry run complete. Server was not started." -ForegroundColor Green
  return
}

New-Item -ItemType Directory -Force -Path $demoRoot | Out-Null

Write-Host "Starting server. Press Ctrl+C to stop; temp demo files will be removed." -ForegroundColor Green
try {
  Push-Location $RepoRoot
  & cargo @serverArgs
}
finally {
  Pop-Location
  Remove-Item -LiteralPath $demoRoot -Recurse -Force -ErrorAction SilentlyContinue
}
