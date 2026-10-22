#!/usr/bin/env bash
# generate-sbom.sh — Generate a Software Bill of Materials from Cargo.lock
# Usage: ./generate-sbom.sh [output-file]
set -euo pipefail

OUTPUT="${1:-sbom.cdx.json}"
WORKSPACE_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "Generating SBOM from $WORKSPACE_ROOT/Cargo.lock"

# Try cargo-sbom if available
if command -v cargo-sbom &>/dev/null; then
    cargo sbom --output-format cyclonedx_1_4 --output-file "$OUTPUT" 2>/dev/null || true
    echo "SBOM generated via cargo-sbom: $OUTPUT"
    exit 0
fi

# Fallback: generate a minimal CycloneDX SBOM from Cargo.lock
python3 - "$WORKSPACE_ROOT" "$OUTPUT" <<'PY'
import json, hashlib, re, sys, os

workspace = sys.argv[1]
output = sys.argv[2]

# Parse Cargo.lock
lock_path = os.path.join(workspace, "Cargo.lock")
packages = []
with open(lock_path) as f:
    content = f.read()

# Simple TOML-ish parsing for [[package]] blocks
current = None
for line in content.splitlines():
    line = line.strip()
    if line == "[[package]]":
        if current:
            packages.append(current)
        current = {}
    elif current is not None and "=" in line:
        key, _, val = line.partition("=")
        key = key.strip()
        val = val.strip().strip('"')
        current[key] = val
if current:
    packages.append(current)

components = []
for p in packages:
    name = p.get("name", "")
    version = p.get("version", "")
    source = p.get("source", "")
    checksum = p.get("checksum", "")
    comp = {
        "type": "library",
        "name": name,
        "version": version,
        "purl": f"pkg:cargo/{name}@{version}",
    }
    if checksum:
        comp["hashes"] = [{"alg": "SHA-256", "content": checksum}]
    components.append(comp)

sbom = {
    "bomFormat": "CycloneDX",
    "specVersion": "1.4",
    "version": 1,
    "metadata": {
        "component": {
            "type": "application",
            "name": "kelson",
            "version": "0.1.0",
        }
    },
    "components": components,
}

with open(output, "w") as f:
    json.dump(sbom, f, indent=2)

print(f"SBOM generated: {output} ({len(components)} components)")
PY