#!/usr/bin/env bash
# Sovereign Application Runtime — local distro verification.
#
# Mirrors the CI verify-distros job (see .github/workflows/ci.yml)
# for an operator who wants to confirm the musl binary works on
# their laptop's Docker without spinning up a real Hetzner CX22.
#
# Usage:
#   cargo build --release --target x86_64-unknown-linux-musl --bin sovereign
#   scripts/verify-distros.sh [--binary target/x86_64-unknown-linux-musl/release/sovereign]
#
# Exit code 0 = all 3 distros green. Non-zero = first failure.

set -euo pipefail

BINARY="${1:-target/x86_64-unknown-linux-musl/release/sovereign}"
[ -f "$BINARY" ] || { echo "binary not found: $BINARY" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || { echo "docker is required" >&2; exit 1; }

DISTROS=(
    "ubuntu:22.04 Ubuntu 22.04"
    "debian:12 Debian 12"
    "alpine:3.20 Alpine 3.20"
)

PASS=0
FAIL=0
FAILED_DISTROS=()

for entry in "${DISTROS[@]}"; do
    image="${entry% *}"
    label="${entry#* }"
    printf "\n\033[1;34m═══ %s (%s) ═══\033[0m\n" "$label" "$image"
    if docker run --rm \
        -v "$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")":/sovereign:ro \
        "$image" \
        /bin/sh -c '
            set -e
            echo "→ --version"
            /sovereign --version
            echo "→ --help (first 5 lines)"
            /sovereign --help | head -5
            echo "→ --help --format json (sample)"
            /sovereign --help --format json 2>/dev/null | head -c 200 || true
            echo
            echo "→ --version (static-link check)"
            if command -v ldd >/dev/null 2>&1; then
                ldd /sovereign 2>&1 | head -3 || true
            fi
        '; then
        printf "\033[32m✓ %s: PASS\033[0m\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "\033[31m✗ %s: FAIL\033[0m\n" "$label"
        FAIL=$((FAIL + 1))
        FAILED_DISTROS+=("$label")
    fi
done

printf "\n\033[1;34m═══ summary ═══\033[0m\n"
printf "  passed: %d\n" "$PASS"
printf "  failed: %d\n" "$FAIL"
if [ "$FAIL" -gt 0 ]; then
    printf "  failed distros: %s\n" "${FAILED_DISTROS[*]}"
    exit 1
fi
printf "\033[32m  all distros green\033[0m\n"
