param(
  [Parameter(Mandatory = $true)]
  [string]$Tag,
  [string]$OutputDir = "dist/security-reports",
  [switch]$FailOnRequiredFailure
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

if ([string]::IsNullOrWhiteSpace($Tag)) {
  throw "Tag is required."
}

if ([System.IO.Path]::IsPathRooted($OutputDir)) {
  $ReportDir = $OutputDir
}
else {
  $ReportDir = Join-Path $RepoRoot $OutputDir
}
New-Item -ItemType Directory -Path $ReportDir -Force | Out-Null

function Parse-TestSummary {
  param([string[]]$OutputLines)

  $line = ($OutputLines | Where-Object { $_ -match "test result:" } | Select-Object -Last 1)
  if (-not $line) {
    return $null
  }

  $match = [regex]::Match(
    $line,
    "test result:\s+(\w+)\.\s+(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored;\s+(\d+)\s+measured;\s+(\d+)\s+filtered out"
  )
  if (-not $match.Success) {
    return $null
  }

  return [ordered]@{
    state        = $match.Groups[1].Value
    passed       = [int]$match.Groups[2].Value
    failed       = [int]$match.Groups[3].Value
    ignored      = [int]$match.Groups[4].Value
    measured     = [int]$match.Groups[5].Value
    filtered_out = [int]$match.Groups[6].Value
  }
}

function Parse-CargoAuditData {
  param([string[]]$OutputLines)

  $joined = ($OutputLines -join "`n").Trim()
  if ([string]::IsNullOrWhiteSpace($joined)) {
    return $null
  }

  $start = $joined.IndexOf("{")
  $end = $joined.LastIndexOf("}")
  if ($start -lt 0 -or $end -lt $start) {
    return $null
  }

  $jsonText = $joined.Substring($start, $end - $start + 1)
  try {
    $audit = $jsonText | ConvertFrom-Json -Depth 100
  }
  catch {
    return $null
  }

  $vulns = @()
  if ($null -ne $audit.vulnerabilities -and $null -ne $audit.vulnerabilities.list) {
    $vulns = @($audit.vulnerabilities.list)
  }

  $severityCounts = @{}
  foreach ($entry in $vulns) {
    $severity = "unknown"
    if ($null -ne $entry.advisory -and -not [string]::IsNullOrWhiteSpace([string]$entry.advisory.severity)) {
      $severity = [string]$entry.advisory.severity
    }
    if (-not $severityCounts.ContainsKey($severity)) {
      $severityCounts[$severity] = 0
    }
    $severityCounts[$severity] += 1
  }

  $orderedSeverities = [ordered]@{}
  foreach ($key in ($severityCounts.Keys | Sort-Object)) {
    $orderedSeverities[$key] = $severityCounts[$key]
  }

  return [ordered]@{
    vulnerability_count = $vulns.Count
    severities          = $orderedSeverities
  }
}

function Invoke-ReportCheck {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,
    [Parameter(Mandatory = $true)]
    [string]$Command,
    [string[]]$Args = @(),
    [bool]$Required = $true,
    [switch]$AuditMode
  )

  $safeName = ($Name -replace "[^0-9A-Za-z._-]", "_")
  $logPath = Join-Path $ReportDir ("{0}.log" -f $safeName)
  $commandLine = ("{0} {1}" -f $Command, ($Args -join " ")).Trim()
  $notes = New-Object System.Collections.Generic.List[string]
  $metrics = [ordered]@{}

  Write-Host ("[security-report] running check: {0}" -f $Name)

  $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
  $outputLines = @()
  $exitCode = 1
  $tmpOut = Join-Path $ReportDir ("{0}.stdout.tmp" -f $safeName)
  $tmpErr = Join-Path $ReportDir ("{0}.stderr.tmp" -f $safeName)
  try {
    if (Test-Path $tmpOut) {
      Remove-Item -Force $tmpOut
    }
    if (Test-Path $tmpErr) {
      Remove-Item -Force $tmpErr
    }

    $proc = Start-Process -FilePath $Command -ArgumentList $Args -NoNewWindow -Wait -PassThru `
      -RedirectStandardOutput $tmpOut -RedirectStandardError $tmpErr
    $exitCode = $proc.ExitCode

    $stdout = if (Test-Path $tmpOut) { Get-Content -Path $tmpOut } else { @() }
    $stderr = if (Test-Path $tmpErr) { Get-Content -Path $tmpErr } else { @() }
    $outputLines = @($stdout + $stderr)
  }
  catch {
    $outputLines = @($_.Exception.Message)
    $exitCode = 1
  }
  finally {
    if (Test-Path $tmpOut) {
      Remove-Item -Force $tmpOut
    }
    if (Test-Path $tmpErr) {
      Remove-Item -Force $tmpErr
    }
  }
  if ($null -eq $exitCode) {
    $exitCode = 1
  }
  $stopwatch.Stop()

  $outputLines | Set-Content -Path $logPath -Encoding UTF8

  $testSummary = Parse-TestSummary -OutputLines $outputLines
  if ($null -ne $testSummary) {
    $metrics.test_summary = $testSummary
    $testSummaryNote = "{0} passed, {1} failed" -f $testSummary.passed, $testSummary.failed
    $notes.Add($testSummaryNote)
  }

  $status = if ($exitCode -eq 0) { "pass" } else { "fail" }

  if ($AuditMode) {
    $auditData = Parse-CargoAuditData -OutputLines $outputLines
    if ($null -ne $auditData) {
      $metrics.audit = $auditData
      $notes.Add(
        "{0} known vulnerability entries" -f $auditData.vulnerability_count
      )
    }

    if ($exitCode -eq 0) {
      $status = "pass"
    }
    elseif ($null -ne $auditData -and $auditData.vulnerability_count -gt 0) {
      $status = "warn"
    }
    else {
      $status = "fail"
    }

    $auditOutput = ($outputLines -join "`n")
    if ($auditOutput -match "no such subcommand|no such command") {
      $notes.Add("cargo-audit is not installed")
      if ($status -eq "fail") {
        $status = "warn"
      }
    }
  }

  if ($status -eq "fail" -and $outputLines.Count -gt 0) {
    $notes.Add("check failed, see log file")
  }

  return [ordered]@{
    name             = $Name
    required         = $Required
    status           = $status
    exit_code        = [int]$exitCode
    duration_seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 2)
    command          = $commandLine
    notes            = ($notes -join "; ")
    metrics          = $metrics
    log_file         = [System.IO.Path]::GetFileName($logPath)
  }
}

$tagSlug = ($Tag -replace "[^0-9A-Za-z._-]", "_")
$generatedAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$commitSha = (git rev-parse HEAD).Trim()
$repository = if ($env:GITHUB_REPOSITORY) { $env:GITHUB_REPOSITORY } else { "local" }
$runUrl = $null
if ($env:GITHUB_SERVER_URL -and $env:GITHUB_REPOSITORY -and $env:GITHUB_RUN_ID) {
  $runUrl = "{0}/{1}/actions/runs/{2}" -f $env:GITHUB_SERVER_URL, $env:GITHUB_REPOSITORY, $env:GITHUB_RUN_ID
}

$checks = @()
$checks += Invoke-ReportCheck -Name "security_contract_suite" -Command "cargo" -Args @("test", "--test", "message_contracts", "--locked") -Required $true
$checks += Invoke-ReportCheck -Name "bridge_reconnect_regression" -Command "cargo" -Args @("test", "--features", "discord-bridge", "--bin", "discord_bot", "--locked", "bridge_supervisor_reconnects_after_disconnect") -Required $true
$checks += Invoke-ReportCheck -Name "bridge_channel_map_regression" -Command "cargo" -Args @("test", "--features", "discord-bridge", "--bin", "discord_bot", "--locked", "parse_channel_map_filters_and_normalizes_entries") -Required $true
$checks += Invoke-ReportCheck -Name "bridge_self_source_match_regression" -Command "cargo" -Args @("test", "--features", "discord-bridge", "--bin", "discord_bot", "--locked", "self_source_filter_matches_only_non_empty_identical_source") -Required $true
$checks += Invoke-ReportCheck -Name "bridge_self_source_type_regression" -Command "cargo" -Args @("test", "--features", "discord-bridge", "--bin", "discord_bot", "--locked", "self_source_filter_ignores_non_string_src_values") -Required $true
$checks += Invoke-ReportCheck -Name "dependency_audit" -Command "cargo" -Args @("audit", "--json") -Required $false -AuditMode

$requiredChecks = @($checks | Where-Object { $_.required -eq $true })
$requiredFailed = @($requiredChecks | Where-Object { $_.status -ne "pass" })
$optionalWarn = @($checks | Where-Object { $_.required -eq $false -and $_.status -eq "warn" })
$optionalFailed = @($checks | Where-Object { $_.required -eq $false -and $_.status -eq "fail" })

$overallStatus = if ($requiredFailed.Count -eq 0) { "pass" } else { "fail" }

$summary = [ordered]@{
  overall_status         = $overallStatus
  required_checks_total  = $requiredChecks.Count
  required_checks_passed = ($requiredChecks.Count - $requiredFailed.Count)
  required_checks_failed = $requiredFailed.Count
  optional_checks_warn   = $optionalWarn.Count
  optional_checks_failed = $optionalFailed.Count
}

$report = [ordered]@{
  metadata = [ordered]@{
    repository       = $repository
    tag              = $Tag
    git_commit       = $commitSha
    generated_at_utc = $generatedAt
    workflow_run_url = $runUrl
  }
  summary  = $summary
  checks   = $checks
}

$jsonPath = Join-Path $ReportDir ("chatify-security-report-{0}.json" -f $tagSlug)
$mdPath = Join-Path $ReportDir ("chatify-security-report-{0}.md" -f $tagSlug)

$report | ConvertTo-Json -Depth 12 | Set-Content -Path $jsonPath -Encoding UTF8

$mdLines = New-Object System.Collections.Generic.List[string]
$mdLines.Add("# Chatify Security Test Report")
$mdLines.Add("")
$mdLines.Add(('- Tag: `{0}`' -f $Tag))
$mdLines.Add(('- Commit: `{0}`' -f $commitSha))
$mdLines.Add(('- Generated (UTC): `{0}`' -f $generatedAt))
if ($runUrl) {
  $mdLines.Add(("- Workflow run: {0}" -f $runUrl))
}
$mdLines.Add("")
$mdLines.Add("## Summary")
$mdLines.Add("")
$mdLines.Add("| Metric | Value |")
$mdLines.Add("| --- | --- |")
$mdLines.Add(("| Overall status | {0} |" -f $summary.overall_status.ToUpperInvariant()))
$mdLines.Add(("| Required checks passed | {0}/{1} |" -f $summary.required_checks_passed, $summary.required_checks_total))
$mdLines.Add(("| Optional warnings | {0} |" -f $summary.optional_checks_warn))
$mdLines.Add(("| Optional failures | {0} |" -f $summary.optional_checks_failed))
$mdLines.Add("")
$mdLines.Add("## Check Results")
$mdLines.Add("")
$mdLines.Add("| Check | Required | Status | Duration (s) | Exit code | Notes |")
$mdLines.Add("| --- | --- | --- | ---: | ---: | --- |")
foreach ($check in $checks) {
  $notesCell = if ([string]::IsNullOrWhiteSpace([string]$check.notes)) { "-" } else { ([string]$check.notes).Replace("|", "\\|") }
  $requiredLabel = if ($check.required) { "yes" } else { "no" }
  $durationText = [string]::Format(
    [System.Globalization.CultureInfo]::InvariantCulture,
    "{0:0.00}",
    [double]$check.duration_seconds
  )
  $row = (
    "| {0} | {1} | {2} | {3} | {4} | {5} |" -f
    $check.name,
    $requiredLabel,
    $check.status.ToUpperInvariant(),
    $durationText,
    $check.exit_code,
    $notesCell
  )
  $mdLines.Add($row)
}
$mdLines.Add("")
$mdLines.Add("## Log Files")
$mdLines.Add("")
foreach ($check in $checks) {
  $mdLines.Add(('- `{0}`' -f $check.log_file))
}

$mdLines | Set-Content -Path $mdPath -Encoding UTF8

Write-Host ("Security report written: {0}" -f $jsonPath)
Write-Host ("Security report written: {0}" -f $mdPath)

if ($env:GITHUB_OUTPUT) {
  "report_json=$jsonPath" | Add-Content -Path $env:GITHUB_OUTPUT
  "report_md=$mdPath" | Add-Content -Path $env:GITHUB_OUTPUT
  "overall_status=$overallStatus" | Add-Content -Path $env:GITHUB_OUTPUT
}

if ($FailOnRequiredFailure -and $overallStatus -ne "pass") {
  throw "Required security checks failed. See security report for details."
}
