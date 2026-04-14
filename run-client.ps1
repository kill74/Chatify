<#
.SYNOPSIS
Runs Chatify client with automatic old-artifact cleanup.

.DESCRIPTION
Thin wrapper around scripts/run-with-auto-clean.ps1 in client mode.

.EXAMPLE
.\run-client.ps1

.EXAMPLE
.\run-client.ps1 -CleanupOnly -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [ValidateSet("dev", "release")]
  [string]$Profile = "dev",

  [ValidateRange(1, 365)]
  [int]$MaxAgeDays = 3,

  [ValidateRange(0.5, 200.0)]
  [double]$MaxTargetSizeGB = 4.0,

  [switch]$CleanupOnly,
  [switch]$SkipCleanup,

  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]]$ProgramArgs = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$runner = Join-Path $PSScriptRoot "scripts/run-with-auto-clean.ps1"
if (-not (Test-Path -LiteralPath $runner)) {
  throw "Runner script not found: $runner"
}

& $runner `
  -Mode client `
  -BuildProfile $Profile `
  -ProgramArgs $ProgramArgs `
  -MaxAgeDays $MaxAgeDays `
  -MaxTargetSizeGB $MaxTargetSizeGB `
  -CleanupOnly:$CleanupOnly `
  -SkipCleanup:$SkipCleanup `
  -WhatIf:$WhatIfPreference

exit $LASTEXITCODE
