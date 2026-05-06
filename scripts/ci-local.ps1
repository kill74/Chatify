param(
  [switch]$PassThru,
  [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$results = New-Object System.Collections.Generic.List[object]

function Write-CiLine {
  param([string]$Message)

  if (-not $Quiet) {
    Write-Host $Message
  }
}

function Format-CommandLine {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Command,
    [string[]]$Arguments = @()
  )

  if ($Arguments.Count -eq 0) {
    return $Command
  }

  return ("{0} {1}" -f $Command, ($Arguments -join " "))
}

function Invoke-LocalCiStep {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,
    [Parameter(Mandatory = $true)]
    [string]$Command,
    [string[]]$Arguments = @()
  )

  $commandLine = Format-CommandLine -Command $Command -Arguments $Arguments
  Write-CiLine ""
  Write-CiLine ("==> {0}" -f $Name)
  Write-CiLine ("    {0}" -f $commandLine)

  $global:LASTEXITCODE = 0
  $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
  $errorMessage = $null
  $exitCode = 0

  try {
    & $Command @Arguments
    if ($null -ne $global:LASTEXITCODE) {
      $exitCode = [int]$global:LASTEXITCODE
    }
  }
  catch {
    $exitCode = 1
    $errorMessage = $_.Exception.Message
  }
  finally {
    $stopwatch.Stop()
  }

  $passed = $exitCode -eq 0
  $result = [ordered]@{
    name        = $Name
    command     = $commandLine
    passed      = $passed
    exit_code   = $exitCode
    duration_ms = [int64]$stopwatch.Elapsed.TotalMilliseconds
  }
  if (-not [string]::IsNullOrWhiteSpace($errorMessage)) {
    $result.error = $errorMessage
  }

  $results.Add([pscustomobject]$result) | Out-Null

  if ($passed) {
    Write-CiLine ("<== passed in {0:n1}s" -f $stopwatch.Elapsed.TotalSeconds)
    return
  }

  Write-CiLine ("<== failed in {0:n1}s (exit {1})" -f $stopwatch.Elapsed.TotalSeconds, $exitCode)
  if (-not [string]::IsNullOrWhiteSpace($errorMessage)) {
    Write-CiLine ("    {0}" -f $errorMessage)
  }
  throw ("Local CI step failed: {0}" -f $Name)
}

function Write-LocalCiSummary {
  $passedCount = @($results | Where-Object { $_.passed }).Count
  $failedCount = $results.Count - $passedCount

  Write-CiLine ""
  Write-CiLine "Local CI summary"
  Write-CiLine ("- passed: {0}" -f $passedCount)
  Write-CiLine ("- failed: {0}" -f $failedCount)

  foreach ($result in $results) {
    $status = if ($result.passed) { "PASS" } else { "FAIL" }
    Write-CiLine ("  [{0}] {1} ({2:n1}s)" -f $status, $result.name, ($result.duration_ms / 1000.0))
  }
}

try {
  Invoke-LocalCiStep "Validate release target inventory" (Join-Path $PSScriptRoot "assert-release-targets.ps1") @()
  Invoke-LocalCiStep "Workspace compile check" "cargo" @("check", "--workspace", "--bins", "--locked")
  Invoke-LocalCiStep "Format check" "cargo" @("fmt", "--all", "--check")
  Invoke-LocalCiStep "Clippy" "cargo" @("clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings")
  Invoke-LocalCiStep "Workspace tests" "cargo" @("test", "--workspace", "--all-targets", "--locked")

  Invoke-LocalCiStep "Protocol contract: auth fields" "cargo" @("test", "--locked", "--test", "message_contracts", "auth_contract_returns_expected_fields")
  Invoke-LocalCiStep "Protocol contract: bootstrap compatibility" "cargo" @("test", "--locked", "--test", "message_contracts", "compatibility_contract_client_bootstrap_flow_stays_stable")
  Invoke-LocalCiStep "Protocol contract: protocol version" "cargo" @("test", "--locked", "--test", "message_contracts", "protocol_contract_advertises_backward_compatible_version")
  Invoke-LocalCiStep "Protocol contract: media relay" "cargo" @("test", "--locked", "--test", "message_contracts", "file_contract_relays_media_metadata_and_chunks")

  Invoke-LocalCiStep "Discord bridge compile gate" "cargo" @("check", "--features", "discord-bridge", "--bin", "discord_bot", "--locked")
  Invoke-LocalCiStep "Bridge client compile gate" "cargo" @("check", "-p", "chatify-client", "--features", "bridge-client", "--locked")

  Write-LocalCiSummary
  if ($PassThru) {
    $results
  }
}
catch {
  Write-LocalCiSummary
  if ($PassThru) {
    $results
  }
  throw
}
