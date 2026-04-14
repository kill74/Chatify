param(
  [switch]$PassThru
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$libraryPath = Join-Path $PSScriptRoot "release-targets.ps1"
if (-not (Test-Path -Path $libraryPath)) {
  throw "Missing release target library script: $libraryPath"
}

. $libraryPath

$targets = Assert-ChatifyReleaseTargets

Write-Host ("Validated {0} release target(s)." -f $targets.Count)
foreach ($target in $targets) {
  Write-Host ("- {0}/{1}" -f $target.Package, $target.Binary)
}

if ($PassThru) {
  $targets
}
