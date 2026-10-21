# ci-parity-check.ps1 — run the CI steps locally.
#
# `.github/workflows/keel.yml` was unfrozen on 2026-10-21, so this script is the LOCAL MIRROR of
# the applied hunks (see `docs/CI-WIRING-PROPOSAL.md` for the reasoning and the proof): it runs
# the DB-backed and tool steps against a live MySQL, exactly as the workflow does.
#
#   pwsh tools/ci-parity-check.ps1 -DatabaseUrl 'mysql://keel:keel@127.0.0.1:3307/keel'
#
# The scaffold-build step is NOT here: it edits the workspace's own Cargo.toml, so it belongs in a
# throwaway checkout (which is what CI has). Its two commands are in the proposal.

param(
    # The live fabric the matrix runs against. The pg half self-skips on a mysql:// URL and the
    # mysql half self-skips on a postgres:// URL, exactly as the CI job behaves.
    [string]$DatabaseUrl = 'mysql://keel:keel@127.0.0.1:3307/keel'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root

try {
    $env:DATABASE_URL = $DatabaseUrl
    # The CI job sets this too; the matrix reads it.
    $env:CARGO_INCREMENTAL = '0'

    Write-Host '== hunk 1: the shared modelstore conformance target =='
    cargo test -p kelson-postgres --features pg-tests --test modelstore_conformance --locked
    if ($LASTEXITCODE -ne 0) { throw 'the conformance target failed' }

    Write-Host '== row 10: the contracts-crate registry tests (already covered by --workspace) =='
    cargo test -p kelson-contracts --lib --locked
    if ($LASTEXITCODE -ne 0) { throw 'the contracts registry tests failed' }

    Write-Host '== hunk 2: the semver gate =='
    # A no-op until the repository has a kelson/platform-v* release tag to compare against; the
    # gate prints that reason and exits 0.
    cargo run -p xtask -- semver
    if ($LASTEXITCODE -ne 0) { throw 'the semver gate failed' }

    Write-Host ''
    Write-Host 'ci-parity-check: OK'
}
finally {
    Pop-Location
}
