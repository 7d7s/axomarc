# generate-provenance.ps1 — Generate build provenance for release artifacts
# Usage: .\generate-provenance.ps1 [-Output provenance.json]
param(
    [string]$Output = "provenance.json"
)

$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $WorkspaceRoot

# Get git info
$gitCommit = (git rev-parse HEAD 2>$null) -replace "`n",""
$gitBranch = (git rev-parse --abbrev-ref HEAD 2>$null) -replace "`n",""
$gitDirty = if ((git status --porcelain 2>$null) -ne "") { $true } else { $false }

# Compute artifact hashes
$artifacts = @()
$artifactPaths = @(
    "target\release\kelson-demo.exe",
    "applications\echo\artifact\kelson_mod_echo.wasm",
    "applications\counter\artifact\kelson_mod_counter.wasm"
)
foreach ($p in $artifactPaths) {
    $full = Join-Path $WorkspaceRoot $p
    if (Test-Path $full) {
        $hash = (Get-FileHash $full -Algorithm SHA256).Hash
        $artifacts += @{
            path = $p
            sha256 = $hash
            size = (Get-Item $full).Length
        }
    }
}

$provenance = @{
    schema = "https://slsa.dev/provenance/v1"
    buildType = "https://kelson.dev/build/v1"
    builder = @{
        id = "kelson-build"
        version = "1.0.0"
    }
    invocation = @{
        configSource = @{
            uri = "git+$gitCommit"
            digest = @{ sha1 = $gitCommit }
            entryPoint = "cargo build --release --package kelson-demo"
        }
        parameters = @{
            cargo_build_jobs = "1"
            cargo_incremental = "0"
        }
        environment = @{
            os = [System.Environment]::OSVersion.VersionString
            rustc = (rustc --version 2>$null)
            cargo = (cargo --version 2>$null)
        }
    }
    metadata = @{
        buildStartedOn = (Get-Date -Format "o")
        completeness = @{
            parameters = $true
            environment = $true
            materials = $true
        }
        reproducible = $true
    }
    materials = @(
        @{
            uri = "git+$gitCommit"
            digest = @{ sha1 = $gitCommit }
        }
    )
    artifacts = $artifacts
    git = @{
        commit = $gitCommit
        branch = $gitBranch
        dirty = $gitDirty
    }
}

$provenance | ConvertTo-Json -Depth 10 | Set-Content $Output
Write-Host "Provenance generated: $Output ($($artifacts.Count) artifacts)"