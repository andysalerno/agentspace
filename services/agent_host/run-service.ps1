Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$composeFile = Join-Path $scriptDir "compose.yaml"
$envFile = Join-Path $scriptDir ".env"
$exampleEnvFile = Join-Path $scriptDir ".env.example"
$projectName = "agentspace-agent-host"
$command = if ($args.Length -gt 0) { $args[0] } else { "start" }

if (-not (Test-Path $envFile)) {
    Copy-Item -LiteralPath $exampleEnvFile -Destination $envFile
}

switch ($command) {
    "start" {
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
