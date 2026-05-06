param(
  [switch]$AllowDirty,
  [switch]$Json
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

function Invoke-GitText {
  param([string[]]$Arguments)

  $global:LASTEXITCODE = 0
  $output = & git @Arguments 2>$null
  if ($global:LASTEXITCODE -ne 0) {
    return $null
  }
  return (($output | Out-String).Trim())
}

function New-ReadinessReport {
  param(
    [string]$Status,
    [string]$Reason,
    [object]$CiEquivalent
  )

  $branch = Invoke-GitText @("rev-parse", "--abbrev-ref", "HEAD")
  $commit = Invoke-GitText @("rev-parse", "HEAD")
  $trackedChanges = @(git status --porcelain --untracked-files=no)

  return [ordered]@{
    status             = $Status
    reason             = $Reason
    branch             = $branch
    commit             = $commit
    dirty_tracked      = $trackedChanges.Count -gt 0
    dirty_tracked_count = $trackedChanges.Count
    dirty_tracked_paths = @($trackedChanges)
    allow_dirty        = [bool]$AllowDirty
    ci_equivalent      = $CiEquivalent
    generated_at_utc   = [DateTimeOffset]::UtcNow.ToString("o")
  }
}

function Write-ReadinessReport {
  param([object]$Report)

  if ($Json) {
    $Report | ConvertTo-Json -Depth 8
    return
  }

  Write-Host "Release readiness"
  Write-Host ("- status: {0}" -f $Report.status)
  if (-not [string]::IsNullOrWhiteSpace($Report.reason)) {
    Write-Host ("- reason: {0}" -f $Report.reason)
  }
  Write-Host ("- branch: {0}" -f $Report.branch)
  Write-Host ("- commit: {0}" -f $Report.commit)
  Write-Host ("- dirty tracked changes: {0} ({1})" -f $Report.dirty_tracked, $Report.dirty_tracked_count)
  Write-Host ("- CI-equivalent: {0}" -f $Report.ci_equivalent.status)
  if ($null -ne $Report.ci_equivalent.duration_ms) {
    Write-Host ("- CI-equivalent duration: {0:n1}s" -f ($Report.ci_equivalent.duration_ms / 1000.0))
  }
}

$initialTrackedChanges = @(git status --porcelain --untracked-files=no)
if ($initialTrackedChanges.Count -gt 0 -and -not $AllowDirty) {
  $ciEquivalent = [ordered]@{
    status      = "not_run"
    passed      = $false
    duration_ms = $null
    steps       = @()
  }
  $report = New-ReadinessReport -Status "failed" -Reason "Tracked worktree changes are present. Re-run with -AllowDirty to verify intentionally dirty work." -CiEquivalent $ciEquivalent
  Write-ReadinessReport -Report $report
  exit 1
}

$ciScript = Join-Path $PSScriptRoot "ci-local.ps1"
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$ciSteps = @()
$ciError = $null

try {
  $ciSteps = @(& $ciScript -PassThru -Quiet)
}
catch {
  $ciError = $_.Exception.Message
}
finally {
  $stopwatch.Stop()
}

$failedSteps = @($ciSteps | Where-Object { -not $_.passed })
$ciPassed = $null -eq $ciError -and $failedSteps.Count -eq 0
$ciEquivalent = [ordered]@{
  status      = if ($ciPassed) { "passed" } else { "failed" }
  passed      = $ciPassed
  duration_ms = [int64]$stopwatch.Elapsed.TotalMilliseconds
  steps       = @($ciSteps)
}
if (-not [string]::IsNullOrWhiteSpace($ciError)) {
  $ciEquivalent.error = $ciError
}

$reportStatus = if ($ciPassed) { "passed" } else { "failed" }
$reason = if ($ciPassed) { "" } else { "Local CI-equivalent gate failed." }
$report = New-ReadinessReport -Status $reportStatus -Reason $reason -CiEquivalent $ciEquivalent
Write-ReadinessReport -Report $report

if (-not $ciPassed) {
  exit 1
}
