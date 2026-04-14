Set-StrictMode -Version Latest

function ConvertFrom-JsonCompat {
  param(
    [Parameter(Mandatory = $true)]
    [string]$JsonText,
    [ValidateRange(2, 1000)]
    [int]$Depth = 100
  )

  if ($PSVersionTable.PSVersion.Major -ge 6) {
    return ($JsonText | ConvertFrom-Json -Depth $Depth)
  }

  return ($JsonText | ConvertFrom-Json)
}

function Get-ChatifyReleaseTargets {
  return @(
    [pscustomobject]@{ Package = "clifford-server"; Binary = "chatify-server" },
    [pscustomobject]@{ Package = "clifford-client"; Binary = "chatify-client" }
  )
}

function Get-CargoMetadata {
  param([string]$CargoCommand = "cargo")

  $metadataLines = & $CargoCommand metadata --format-version 1 --no-deps
  $metadataJson = ($metadataLines -join "`n").Trim()
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($metadataJson)) {
    throw "Failed to read workspace metadata via '$CargoCommand metadata'."
  }

  try {
    return (ConvertFrom-JsonCompat -JsonText $metadataJson -Depth 100)
  }
  catch {
    throw "Failed to parse Cargo metadata JSON."
  }
}

function Assert-CargoBinaryTarget {
  param(
    [Parameter(Mandatory = $true)]
    [object]$Metadata,
    [Parameter(Mandatory = $true)]
    [string]$Package,
    [Parameter(Mandatory = $true)]
    [string]$Binary
  )

  $packageObj = @($Metadata.packages | Where-Object { $_.name -eq $Package } | Select-Object -First 1)
  if (-not $packageObj) {
    $knownPackages = @($Metadata.packages | ForEach-Object { $_.name }) -join ", "
    throw ("Package '{0}' was not found in this workspace. Known packages: {1}" -f $Package, $knownPackages)
  }

  $binTargets = @($packageObj.targets | Where-Object { $_.kind -contains "bin" })
  $binTarget = @($binTargets | Where-Object { $_.name -eq $Binary } | Select-Object -First 1)
  if (-not $binTarget) {
    $availableBins = if ($binTargets.Count -gt 0) {
      ($binTargets | ForEach-Object { $_.name }) -join ", "
    }
    else {
      "<none>"
    }
    throw ("Binary target '{0}' was not found in package '{1}'. Available bin targets: {2}" -f $Binary, $Package, $availableBins)
  }
}

function Assert-ChatifyReleaseTargets {
  param(
    [object[]]$Targets = (Get-ChatifyReleaseTargets),
    [string]$CargoCommand = "cargo"
  )

  if (-not (Get-Command $CargoCommand -ErrorAction SilentlyContinue)) {
    throw "Required command '$CargoCommand' was not found in PATH."
  }

  $metadata = Get-CargoMetadata -CargoCommand $CargoCommand
  foreach ($target in $Targets) {
    if ($null -eq $target -or [string]::IsNullOrWhiteSpace([string]$target.Package) -or [string]::IsNullOrWhiteSpace([string]$target.Binary)) {
      throw "Invalid release target entry. Each target must include non-empty Package and Binary properties."
    }

    Assert-CargoBinaryTarget -Metadata $metadata -Package $target.Package -Binary $target.Binary
  }

  return $Targets
}
