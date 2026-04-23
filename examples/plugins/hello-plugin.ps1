Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$PluginOp = "manifest"
$PluginCommand = ""

for ($i = 0; $i -lt $args.Count; $i++) {
  switch ($args[$i]) {
    "--chatify-plugin-op" {
      if ($i + 1 -ge $args.Count) {
        throw "missing value for --chatify-plugin-op"
      }
      $PluginOp = [string]$args[$i + 1]
      $i++
    }
    "--chatify-plugin-command" {
      if ($i + 1 -ge $args.Count) {
        throw "missing value for --chatify-plugin-command"
      }
      $PluginCommand = [string]$args[$i + 1]
      $i++
    }
  }
}

function Write-JsonResponse {
  param([Parameter(Mandatory = $true)][hashtable]$Value)

  $Value | ConvertTo-Json -Depth 8 -Compress
}

function Read-JsonPayload {
  $raw = [Console]::In.ReadToEnd()
  if ([string]::IsNullOrWhiteSpace($raw)) {
    return @{}
  }

  return $raw | ConvertFrom-Json
}

switch ($PluginOp) {
  "manifest" {
    Write-JsonResponse @{
      api_version = "1"
      name = "hello-plugin"
      message_hook = $false
      commands = @(
        @{
          name = "hello"
          description = "Say hello from an external plugin"
        }
      )
    }
  }

  "slash" {
    $payload = Read-JsonPayload
    $user = if ($payload.user) { [string]$payload.user } else { "someone" }
    $argsList = @($payload.args)
    $target = if ($argsList.Count -gt 0 -and -not [string]::IsNullOrWhiteSpace([string]$argsList[0])) {
      [string]$argsList[0]
    }
    else {
      "Chatify"
    }

    if ($PluginCommand -ne "hello") {
      Write-JsonResponse @{
        api_version = "1"
        error = "unsupported command '$PluginCommand'"
      }
      return
    }

    Write-JsonResponse @{
      api_version = "1"
      messages = @(
        @{
          target = "channel"
          text = "Hello $target, from $user's external Chatify plugin."
        }
      )
    }
  }

  "message_hook" {
    Write-JsonResponse @{
      api_version = "1"
      blocked = $false
      messages = @()
    }
  }

  default {
    Write-JsonResponse @{
      api_version = "1"
      error = "unsupported plugin op '$PluginOp'"
    }
  }
}
