# generate-sbom.ps1 — Generate a Software Bill of Materials from Cargo.lock
# Usage: .\generate-sbom.ps1 [-Output sbom.cdx.json]
param(
    [string]$Output = "sbom.cdx.json"
)

$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$LockPath = Join-Path $WorkspaceRoot "Cargo.lock"

Write-Host "Generating SBOM from $LockPath"

# Parse Cargo.lock
$packages = @()
$current = $null
foreach ($line in (Get-Content $LockPath)) {
    $trimmed = $line.Trim()
    if ($trimmed -eq "[[package]]") {
        if ($current) { $packages += $current }
        $current = @{}
    } elseif ($current -ne $null -and $trimmed -match '^(\w+)\s*=\s*"(.*)"$') {
        $current[$matches[1]] = $matches[2]
    }
}
if ($current) { $packages += $current }

$components = @()
foreach ($p in $packages) {
    $comp = @{
        type = "library"
        name = $p["name"]
        version = $p["version"]
        purl = "pkg:cargo/$($p['name'])@$($p['version'])"
    }
    if ($p["checksum"]) {
        $comp.hashes = @(@{ alg = "SHA-256"; content = $p["checksum"] })
    }
    $components += $comp
}

$sbom = @{
    bomFormat = "CycloneDX"
    specVersion = "1.4"
    version = 1
    metadata = @{
        component = @{
            type = "application"
            name = "kelson"
            version = "0.1.0"
        }
    }
    components = $components
}

$sbom | ConvertTo-Json -Depth 10 | Set-Content $Output
Write-Host "SBOM generated: $Output ($($components.Count) components)"