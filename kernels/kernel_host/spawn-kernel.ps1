Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$composeFile = Join-Path $scriptDir "compose.copilot.yaml"
$envFile = Join-Path $scriptDir ".env"
$exampleEnvFile = Join-Path $scriptDir ".env.example"
$projectName = "agentspace-kernel"

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

function Invoke-Cleanup {
    docker compose -p $projectName -f $composeFile down --remove-orphans | Out-Null
}

Invoke-Cleanup

try {
    Write-Error "Building kernel image..."
    docker compose -p $projectName -f $composeFile build kernel

    if ($args.Length -eq 0) {
        throw "Usage: .\spawn-kernel.ps1 <message>"
    }

    $message = $args[0]
    Write-Error "Running kernel..."
    docker compose -p $projectName -f $composeFile run --rm kernel $message
} finally {
    Invoke-Cleanup
}
