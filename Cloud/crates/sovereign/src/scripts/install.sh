#!/usr/bin/env bash
# Sovereign Application Runtime — install script.
# Full spec: docs/tech-stack.md §12.
# The 9-step contract:
#   1. Detect platform
#   2. Read the requested version
#   3. Download the binary (cosign-pinned, SHA-256-verified)
#   4. Verify SHA-256
#   5. Verify cosign signature
#   6. Install to /usr/local/bin/sovereign
#   7. Install the systemd unit (Linux)
#   8. Wait for the service to be ready
#   9. Print the 5-line "what next"
set -euo pipefail

VERSION="${SOVEREIGN_VERSION:-latest}"
RELEASES_BASE="https://releases.sovereignruntime.dev"
COSIGN_PUBKEY_URL="https://sovereignruntime.dev/.well-known/cosign.pub"

die() { echo "Error: $*" >&2; exit 1; }
info() { echo "→ $*"; }

# 1. Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Linux)  PLATFORM_OS=linux ;;
    Darwin) PLATFORM_OS=darwin ;;
    FreeBSD) PLATFORM_OS=freebsd ;;
    *) die "unsupported OS: $OS" ;;
esac
case "$ARCH" in
    x86_64)  PLATFORM_ARCH=amd64 ;;
    aarch64|arm64) PLATFORM_ARCH=arm64 ;;
    *) die "unsupported architecture: $ARCH" ;;
esac

# Detect glibc vs musl (Linux only)
LIBC_FLAVOR="musl"
if [ "$PLATFORM_OS" = "linux" ]; then
    if command -v ldd >/dev/null 2>&1; then
        if ldd --version 2>&1 | head -1 | grep -qi glibc; then
            # Use musl if glibc >= 2.31 (covers Ubuntu 20.04+, Debian 11+, RHEL 8+);
            # fall back to glibc binary for older distros.
            GLIBC_VER="$(ldd --version 2>&1 | head -1 | grep -oE '[0-9]+\.[0-9]+' | head -1)"
            GLIBC_MAJOR="$(echo "$GLIBC_VER" | cut -d. -f1)"
            GLIBC_MINOR="$(echo "$GLIBC_VER" | cut -d. -f2)"
            if [ "${GLIBC_MAJOR:-0}" -lt 2 ] || { [ "${GLIBC_MAJOR:-0}" -eq 2 ] && [ "${GLIBC_MINOR:-0}" -lt 31 ]; }; then
                LIBC_FLAVOR="gnu"
            fi
        fi
    fi
fi

case "$PLATFORM_OS" in
    linux)  TARGET="${ARCH}-unknown-linux-${LIBC_FLAVOR}" ;;
    darwin) TARGET="${PLATFORM_ARCH}-apple-darwin" ;;
    freebsd) TARGET="${ARCH}-unknown-freebsd" ;;
esac

info "Detected: ${OS} ${ARCH} (target=${TARGET})"

# 2. Resolve version
if [ "$VERSION" = "latest" ]; then
    info "Resolving latest version..."
    VERSION="$(curl -sSfL "${RELEASES_BASE}/stable.json" | grep -oE '"version":[[:space:]]*"[^"]+"' | head -1 | cut -d'"' -f4)"
    [ -n "$VERSION" ] || die "could not resolve latest version"
    info "Latest version: $VERSION"
fi

TARBALL="sovereign-${VERSION}-${TARGET}.tar.xz"
CHECKSUMS="sha256sums.txt"
URL="${RELEASES_BASE}/${VERSION}/${TARBALL}"

# 3. Download
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
info "Downloading $URL"
curl -sSfL --tlsv1.2 --proto =https -o "$WORKDIR/$TARBALL" "$URL" \
    || die "cannot reach $RELEASES_BASE. Check your network or use the package path."

# 4. Verify SHA-256
info "Verifying SHA-256..."
curl -sSfL --tlsv1.2 --proto =https -o "$WORKDIR/$CHECKSUMS" \
    "${RELEASES_BASE}/${VERSION}/${CHECKSUMS}" \
    || die "cannot fetch checksums"
( cd "$WORKDIR" && sha256sum -c <(grep "$TARBALL" "$CHECKSUMS") ) \
    || die "SHA-256 mismatch. Refusing to install. This is a release-blocking bug; please report to security@sovereignruntime.dev."

# 5. Verify cosign signature (skipped in offline mode)
if command -v cosign >/dev/null 2>&1; then
    info "Verifying cosign signature..."
    curl -sSfL --tlsv1.2 --proto =https -o "$WORKDIR/$TARBALL.cosign.bundle" \
        "${RELEASES_BASE}/${VERSION}/${TARBALL}.cosign.bundle" 2>/dev/null \
        && cosign verify-blob --key "$COSIGN_PUBKEY_URL" \
            --bundle "$WORKDIR/$TARBALL.cosign.bundle" \
            "$WORKDIR/$TARBALL" \
            || die "signature verification failed. The binary may have been tampered with."
else
    info "(cosign not installed; skipping signature check — install cosign for full verification)"
fi

# 6. Install the binary
info "Extracting..."
tar -xJf "$WORKDIR/$TARBALL" -C "$WORKDIR"

INSTALL_DIR="/usr/local/bin"
if [ ! -w "$INSTALL_DIR" ]; then
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    info "/usr/local/bin not writable; installing to $INSTALL_DIR"
fi

install -m 0755 "$WORKDIR/sovereign" "$INSTALL_DIR/sovereign"
info "Installed to $INSTALL_DIR/sovereign"

# 7. Install systemd unit (Linux only)
if [ "$PLATFORM_OS" = "linux" ] && command -v systemctl >/dev/null 2>&1; then
    info "Installing systemd unit..."
    sudo install -m 0644 "$WORKDIR/sovereign.service" /etc/systemd/system/sovereign.service
    sudo systemctl daemon-reload
    sudo systemctl enable --now sovereign.service
    info "Service started."
else
    info "(systemd not available; run 'sovereign run' to start the control plane)"
fi

# 8. Wait for ready
if command -v systemctl >/dev/null 2>&1; then
    info "Waiting for the service to be ready..."
    for _ in $(seq 1 30); do
        if systemctl is-active --quiet sovereign 2>/dev/null; then
            if "$INSTALL_DIR/sovereign" --version >/dev/null 2>&1; then
                break
            fi
        fi
        sleep 1
    done
fi

# 9. Print the "what next"
cat <<'EOF'

✓ sovereign is installed. Next, you probably want to:
  → sovereign init                          # detect your framework, write app.yaml
  → sovereign deploy                        # build + ship your app
  → sovereign doctor                        # diagnose the host
  → sovereign --help                        # full command tree
  → cat /var/log/sovereign/sovereign.log    # see the logs

For the docs, see https://sovereignruntime.dev/docs/
EOF
