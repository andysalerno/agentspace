Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$composeFile = Join-Path $scriptDir "compose.copilot.yaml"
$envFile = Join-Path $scriptDir ".env"
$exampleEnvFile = Join-Path $scriptDir ".env.example"
$projectName = "agentspace-kernel"

if (-not (Test-Path $envFile)) {
    Copy-Item -LiteralPath $exampleEnvFile -Destination $envFile
}

function Invoke-Cleanup {
    docker compose -p $projectName -f $composeFile down --remove-orphans | Out-Null
}

Invoke-Cleanup

try {
    Write-Error "Building kernel image..."
    docker compose -p $projectName -f $composeFile build kernel setup

    if ($args.Length -eq 0) {
        throw "Usage: .\spawn-kernel.ps1 <message|setup>"
    }

    if ($args[0] -eq "setup") {
        Write-Error "Starting interactive copilot session (run /login to authenticate)..."
        docker compose -p $projectName -f $composeFile run --rm setup
    } else {
        $message = $args[0]
        Write-Error "Running kernel..."
        docker compose -p $projectName -f $composeFile run --rm kernel $message
    }
} finally {
    Invoke-Cleanup
}
