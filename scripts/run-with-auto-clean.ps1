<#
.SYNOPSIS
Runs Chatify client/server with automatic cleanup of old build artifacts.

.DESCRIPTION
Before running the selected binary, this script removes stale build/package caches
based on age and enforces a size budget for the target directory. If the budget is
exceeded, it runs profile-specific cargo clean as a hard reset.

.PARAMETER Mode
Selects which binary to run: client or server.

.PARAMETER Profile
Selects cargo profile: dev or release.

.PARAMETER ProgramArgs
Arguments forwarded to the selected Chatify binary.

.PARAMETER MaxAgeDays
Retention window in days for cache entry cleanup.

.PARAMETER MaxTargetSizeGB
Maximum allowed target directory size before hard cleanup.

.PARAMETER CleanupOnly
Runs cleanup and exits without launching cargo run.

.PARAMETER SkipCleanup
Skips cleanup and immediately runs cargo run.

.EXAMPLE
.\scripts\run-with-auto-clean.ps1 -Mode client -ProgramArgs @('--host','127.0.0.1','--port','8765')

.EXAMPLE
.\scripts\run-with-auto-clean.ps1 -CleanupOnly -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [ValidateSet("client", "server")]
  [string]$Mode = "client",

  [ValidateSet("dev", "release")]
  [string]$Profile = "dev",

  [string[]]$ProgramArgs = @(),

  [ValidateRange(1, 365)]
  [int]$MaxAgeDays = 3,

  [ValidateRange(0.5, 200.0)]
  [double]$MaxTargetSizeGB = 4.0,

  [switch]$CleanupOnly,
  [switch]$SkipCleanup
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot

function Assert-CommandAvailable {
  param([Parameter(Mandatory = $true)][string]$CommandName)

  if (-not (Get-Command $CommandName -ErrorAction SilentlyContinue)) {
    throw "Required command '$CommandName' was not found in PATH."
  }
}

function Get-DirectorySizeBytes {
  param([Parameter(Mandatory = $true)][string]$Path)

  if (-not (Test-Path -LiteralPath $Path)) {
    return 0
  }

  $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue |
    Measure-Object -Property Length -Sum).Sum

  if ($null -eq $sum) {
    return 0
  }

  return [int64]$sum
}

function Remove-OldChildren {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][datetime]$Cutoff,
    [Parameter(Mandatory = $true)][string]$Label
  )

  if (-not (Test-Path -LiteralPath $Path)) {
    return [PSCustomObject]@{
      Path           = $Path
      CandidateCount = 0
      RemovedCount   = 0
      FailedCount    = 0
    }
  }

  $stats = [PSCustomObject]@{
    Path           = $Path
    CandidateCount = 0
    RemovedCount   = 0
    FailedCount    = 0
  }

  $entries = Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue |
  Where-Object { $_.LastWriteTime -lt $Cutoff }

  $stats.CandidateCount = @($entries).Count
  if ($stats.CandidateCount -eq 0) {
    return $stats
  }

  $targetMessage = "$Label entries in $Path (count=$($stats.CandidateCount))"
  if (-not $PSCmdlet.ShouldProcess($targetMessage, "Remove old entries")) {
    return $stats
  }

  foreach ($entry in $entries) {
    try {
      Remove-Item -LiteralPath $entry.FullName -Recurse -Force -ErrorAction Stop
      $stats.RemovedCount++
    }
    catch {
      $stats.FailedCount++
      Write-Warning "Could not remove '$($entry.FullName)': $($_.Exception.Message)"
    }
  }

  return $stats
}

function Remove-EmptyDirectories {
  param([Parameter(Mandatory = $true)][string]$Root)

  if ($WhatIfPreference) {
    return
  }

  if (-not (Test-Path -LiteralPath $Root)) {
    return
  }

  $dirs = Get-ChildItem -LiteralPath $Root -Recurse -Directory -ErrorAction SilentlyContinue |
  Sort-Object -Property FullName -Descending

  foreach ($dir in $dirs) {
    try {
      $hasChild = $null -ne (Get-ChildItem -LiteralPath $dir.FullName -Force -ErrorAction SilentlyContinue |
        Select-Object -First 1)
      $isEmpty = -not $hasChild

      if ($isEmpty) {
        Remove-Item -LiteralPath $dir.FullName -Force -ErrorAction SilentlyContinue
      }
    }
    catch {
      # Ignore cleanup failures for empty-dir pruning.
    }
  }
}

function Invoke-AutoCleanup {
  param(
    [Parameter(Mandatory = $true)][int]$RetentionDays,
    [Parameter(Mandatory = $true)][double]$SizeBudgetGB,
    [Parameter(Mandatory = $true)][string]$RequestedProfile
  )

  $cutoff = (Get-Date).AddDays(-$RetentionDays)
  Write-Host "Cleaning artifacts older than $RetentionDays day(s) (before $($cutoff.ToString('yyyy-MM-dd HH:mm:ss')))..."

  $cleanupTargets = @(
    @{ Path = "target/debug/incremental"; Label = "debug incremental cache" },
    @{ Path = "target/release/incremental"; Label = "release incremental cache" },
    @{ Path = "target/debug/build"; Label = "debug build cache" },
    @{ Path = "target/release/build"; Label = "release build cache" },
    @{ Path = "target/debug/.fingerprint"; Label = "debug fingerprint cache" },
    @{ Path = "target/release/.fingerprint"; Label = "release fingerprint cache" },
    @{ Path = "target/tmp"; Label = "temporary build cache" },
    @{ Path = "target/flycheck0"; Label = "flycheck cache" },
    @{ Path = "dist"; Label = "old package output" }
  )

  $candidateTotal = 0
  $removedTotal = 0
  $failedTotal = 0
  foreach ($target in $cleanupTargets) {
    $fullPath = Join-Path $RepoRoot $target.Path
    $stats = Remove-OldChildren -Path $fullPath -Cutoff $cutoff -Label $target.Label
    $candidateTotal += $stats.CandidateCount
    $removedTotal += $stats.RemovedCount
    $failedTotal += $stats.FailedCount

    if ($stats.CandidateCount -gt 0) {
      Write-Host "Cleanup scan for $($target.Path): candidates=$($stats.CandidateCount), removed=$($stats.RemovedCount), failed=$($stats.FailedCount)."
    }
  }

  Remove-EmptyDirectories -Root (Join-Path $RepoRoot "target")

  $targetPath = Join-Path $RepoRoot "target"
  $sizeBytes = Get-DirectorySizeBytes -Path $targetPath
  $sizeGB = [Math]::Round(($sizeBytes / 1GB), 2)
  Write-Host "Current target size: $sizeGB GB"

  if ($sizeGB -le $SizeBudgetGB) {
    Write-Host "target is within budget ($SizeBudgetGB GB)."
    return
  }

  Write-Warning "target exceeds budget ($SizeBudgetGB GB). Running profile clean for a hard reset."
  $profilesToClean = if ($RequestedProfile -eq "release") { @("release") } else { @("dev", "test") }

  foreach ($profileName in $profilesToClean) {
    & cargo clean --profile $profileName
    if ($LASTEXITCODE -ne 0) {
      throw "cargo clean --profile $profileName failed with exit code $LASTEXITCODE"
    }
  }

  $finalSizeBytes = Get-DirectorySizeBytes -Path $targetPath
  $finalSizeGB = [Math]::Round(($finalSizeBytes / 1GB), 2)
  Write-Host "Final target size after hard cleanup: $finalSizeGB GB"
  Write-Host "Cleanup summary before hard cleanup: candidates=$candidateTotal, removed=$removedTotal, failed=$failedTotal"
}

function Invoke-Main {
  Assert-CommandAvailable -CommandName "cargo"

  $binary = if ($Mode -eq "server") { "clicord-server" } else { "clicord-client" }

  if (-not $SkipCleanup) {
    Invoke-AutoCleanup -RetentionDays $MaxAgeDays -SizeBudgetGB $MaxTargetSizeGB -RequestedProfile $Profile
  }
  else {
    Write-Host "Cleanup skipped (-SkipCleanup)."
  }

  if ($CleanupOnly) {
    Write-Host "Cleanup-only mode complete."
    return 0
  }

  $cargoArgs = @("run")
  if ($Profile -eq "release") {
    $cargoArgs += "--release"
  }

  $cargoArgs += @("--bin", $binary)
  if ($ProgramArgs.Count -gt 0) {
    $cargoArgs += "--"
    $cargoArgs += $ProgramArgs
  }

  Write-Host "Running: cargo $($cargoArgs -join ' ')"
  & cargo @cargoArgs
  return [int]$LASTEXITCODE
}

$exitCode = 1
Push-Location -LiteralPath $RepoRoot
try {
  $exitCode = Invoke-Main
}
finally {
  Pop-Location
}

exit $exitCode
