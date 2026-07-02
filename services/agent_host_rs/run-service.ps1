Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$composeFile = Join-Path $scriptDir "compose.yaml"
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptDir)
$kernelComposeFile = Join-Path $repoRoot "kernels\\kernel_host\\compose.copilot.yaml"
$envFile = Join-Path $scriptDir ".env"
$exampleEnvFile = Join-Path $scriptDir ".env.example"
$projectName = "agentspace-agent-host"
$command = if ($args.Length -gt 0) { $args[0] } else { "start" }

function Get-AgentSpaceVersion {
    if ($env:AGENTSPACE_VERSION) {
        return $env:AGENTSPACE_VERSION
    }

    $timestamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
    try {
        $sha = (git rev-parse --short=12 HEAD 2>$null).Trim()
    } catch {
        $sha = ""
    }
    if ([string]::IsNullOrWhiteSpace($sha)) {
        $sha = "unknown"
    }
    return "$sha-$timestamp"
}

if (-not (Test-Path $envFile)) {
    Copy-Item -LiteralPath $exampleEnvFile -Destination $envFile
}

$env:AGENTSPACE_VERSION = Get-AgentSpaceVersion

switch ($command) {
    "start" {
        docker compose -p "agentspace-kernel" -f $kernelComposeFile build kernel
        docker compose -p $projectName -f $composeFile down --remove-orphans
        docker compose -p $projectName -f $composeFile build
        docker compose -p $projectName -f $composeFile up -d
    }
    "stop" {
        docker compose -p $projectName -f $composeFile down --remove-orphans
    }
    "logs" {
        docker compose -p $projectName -f $composeFile logs -f
    }
    "status" {
        docker compose -p $projectName -f $composeFile ps
    }
    default {
        throw "Usage: .\run-service.ps1 [start|stop|logs|status]"
    }
}
