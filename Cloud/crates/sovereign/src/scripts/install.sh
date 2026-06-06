#!/usr/bin/env bash
# Sovereign Application Runtime — install script.
# Full spec: docs/tech-stack.md §12, docs/operations/release-process.md.
# The 9-step contract:
#   1. Detect platform
#   2. Read the requested version
#   3. Download the binary (cosign-pinned, SHA-256-verified)
#   4. Verify SHA-256
#   5. Verify cosign signature
#   6. Install to /usr/local/bin/sovereign (or $INSTALL_DIR)
#   7. Install the systemd unit (Linux; --no-systemd to skip)
#   8. Wait for the service to be ready
#   9. Print the 5-line "what next"
#
# CLI flags (in addition to env vars):
#   --version <v>           Install a specific version (default: latest)
#   --install-dir <path>    Override the install dir (default: /usr/local/bin
#                           or $HOME/.local/bin if /usr/local/bin is not writable)
#   --no-systemd            Skip the systemd unit install (for containers,
#                           air-gapped installs, or non-systemd init systems)
#   --no-caddy-config       Skip the Caddy config drop-in
#   --dry-run               Print the plan, do not download or install anything
#   --releases-base <url>   Override the release URL (for mirror / staging)
#   -h | --help             Show this help
set -euo pipefail

# Defaults
VERSION="${SOVEREIGN_VERSION:-latest}"
RELEASES_BASE="${SOVEREIGN_RELEASES_BASE:-https://releases.sovereignruntime.dev}"
COSIGN_PUBKEY_URL="https://sovereignruntime.dev/.well-known/cosign.pub"
INSTALL_DIR_OVERRIDE=""
SKIP_SYSTEMD=0
SKIP_CADDY=0
DRY_RUN=0

# Parse args
while [ $# -gt 0 ]; do
    case "$1" in
        --version)       VERSION="$2"; shift 2 ;;
        --install-dir)   INSTALL_DIR_OVERRIDE="$2"; shift 2 ;;
        --no-systemd)    SKIP_SYSTEMD=1; shift ;;
        --no-caddy-config) SKIP_CADDY=1; shift ;;
        --dry-run)       DRY_RUN=1; shift ;;
        --releases-base) RELEASES_BASE="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,40p' "$0" | sed 's/^# //; s/^#//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

die() { echo "Error: $*" >&2; exit 1; }
info() { echo "→ $*"; }
ok()   { echo "✓ $*"; }

[ "$(id -u)" -eq 0 ] && die "do NOT run as root; the systemd install will sudo when needed"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1  || die "tar is required"

# 1. Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Linux)  PLATFORM_OS=linux ;;
    Darwin) PLATFORM_OS=darwin ;;
    FreeBSD) PLATFORM_OS=freebsd ;;
    *) die "unsupported OS: $OS (supported: linux, darwin, freebsd)" ;;
esac
case "$ARCH" in
    x86_64)  PLATFORM_ARCH=amd64 ;;
    aarch64|arm64) PLATFORM_ARCH=arm64 ;;
    *) die "unsupported architecture: $ARCH (supported: x86_64, aarch64/arm64)" ;;
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
    info "Resolving latest version from $RELEASES_BASE/stable.json ..."
    VERSION="$(curl -sSfL --tlsv1.2 --proto =https "${RELEASES_BASE}/stable.json" \
        | grep -oE '"version":[[:space:]]*"[^"]+"' | head -1 | cut -d'"' -f4)" \
        || die "could not resolve latest version. Set --version explicitly or check $RELEASES_BASE/stable.json"
    [ -n "$VERSION" ] || die "stable.json did not include a version field"
    info "Latest version: $VERSION"
fi

TARBALL="sovereign-${VERSION}-${TARGET}.tar.xz"
CHECKSUMS="sha256sums.txt"
URL="${RELEASES_BASE}/${VERSION}/${TARBALL}"

# Dry-run: print the plan and exit
if [ "$DRY_RUN" -eq 1 ]; then
    cat <<EOF
Plan (dry run):
  download:     $URL
  verify with:  sha256sums.txt + cosign (if installed)
  install to:   ${INSTALL_DIR_OVERRIDE:-/usr/local/bin (or \$HOME/.local/bin)}
  systemd:      $([ "$SKIP_SYSTEMD" -eq 0 ] && echo "install + enable" || echo "skipped (--no-systemd)")
  caddy config: $([ "$SKIP_CADDY" -eq 0 ] && echo "install (if /etc/caddy exists)" || echo "skipped (--no-caddy-config)")

Re-run without --dry-run to proceed.
EOF
    exit 0
fi

# 3. Download
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
info "Downloading $URL"
curl -sSfL --tlsv1.2 --proto =https -o "$WORKDIR/$TARBALL" "$URL" \
    || die "cannot reach $URL. Check your network, or use --releases-base to point at a mirror."

# 4. Verify SHA-256
info "Verifying SHA-256..."
curl -sSfL --tlsv1.2 --proto =https -o "$WORKDIR/$CHECKSUMS" \
    "${RELEASES_BASE}/${VERSION}/$CHECKSUMS" \
    || die "cannot fetch checksums from ${RELEASES_BASE}/${VERSION}/$CHECKSUMS"
( cd "$WORKDIR" && sha256sum -c <(grep "$TARBALL" "$CHECKSUMS") ) \
    || die "SHA-256 mismatch. Refusing to install. This is a release-blocking bug; please report to security@sovereignruntime.dev."

# 5. Verify cosign signature (skipped if cosign not installed or in offline mode)
if command -v cosign >/dev/null 2>&1; then
    info "Verifying cosign signature..."
    if curl -sSfL --tlsv1.2 --proto =https -o "$WORKDIR/$TARBALL.cosign.bundle" \
        "${RELEASES_BASE}/${VERSION}/${TARBALL}.cosign.bundle" 2>/dev/null; then
        cosign verify-blob --key "$COSIGN_PUBKEY_URL" \
            --bundle "$WORKDIR/$TARBALL.cosign.bundle" \
            "$WORKDIR/$TARBALL" \
            || die "signature verification failed. The binary may have been tampered with."
        ok "cosign signature verified"
    else
        info "(no .cosign.bundle found for this artifact; skipping signature check)"
    fi
else
    info "(cosign not installed; skipping signature check — install cosign for full verification: https://docs.sigstore.dev/cosign/installation/)"
fi

# 6. Install the binary
info "Extracting..."
tar -xJf "$WORKDIR/$TARBALL" -C "$WORKDIR"

if [ -n "$INSTALL_DIR_OVERRIDE" ]; then
    INSTALL_DIR="$INSTALL_DIR_OVERRIDE"
elif [ ! -w "/usr/local/bin" ]; then
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    info "/usr/local/bin not writable; installing to $INSTALL_DIR (add it to PATH if not already)"
else
    INSTALL_DIR="/usr/local/bin"
fi
mkdir -p "$INSTALL_DIR"

install -m 0755 "$WORKDIR/sovereign" "$INSTALL_DIR/sovereign"
ok "Installed to $INSTALL_DIR/sovereign"

# 7. Install systemd unit (Linux only, not --no-systemd)
if [ "$PLATFORM_OS" = "linux" ] && [ "$SKIP_SYSTEMD" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
    info "Installing systemd unit..."
    sudo install -m 0644 "$WORKDIR/sovereign.service" /etc/systemd/system/sovereign.service
    sudo systemctl daemon-reload
    # Idempotent: only enable --now if not already active.
    if ! systemctl is-enabled --quiet sovereign 2>/dev/null; then
        sudo systemctl enable sovereign.service
    fi
    if ! systemctl is-active --quiet sovereign 2>/dev/null; then
        sudo systemctl start sovereign.service
    fi
    ok "Service installed and $(systemctl is-active sovereign)"
else
    if [ "$SKIP_SYSTEMD" -eq 1 ]; then
        info "(--no-systemd set; skipping systemd install. Run 'sovereign run' to start the control plane.)"
    else
        info "(systemd not available; run 'sovereign run' to start the control plane)"
    fi
fi

# 7b. Install the Caddy config (best-effort, not --no-caddy-config)
if [ "$PLATFORM_OS" = "linux" ] && [ "$SKIP_CADDY" -eq 0 ] && command -v caddy >/dev/null 2>&1; then
    if [ -d /etc/caddy ]; then
        info "Installing Caddy config..."
        sudo install -m 0644 "$WORKDIR/Caddyfile" /etc/caddy/Caddyfile.sovereign
        ok "Caddy config installed at /etc/caddy/Caddyfile.sovereign. Merge it into /etc/caddy/Caddyfile or 'sudo systemctl reload caddy' after editing."
    fi
else
    if [ "$SKIP_CADDY" -eq 1 ]; then
        info "(--no-caddy-config set; skipping Caddy config install.)"
    else
        info "(caddy not installed; skipping Caddy config install. 'sovereign deploy' requires Caddy — see docs/operations-runbook.md §3.)"
    fi
fi

# 8. Wait for ready (if systemd is in play)
if [ "$PLATFORM_OS" = "linux" ] && [ "$SKIP_SYSTEMD" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
    info "Waiting for the service to be ready..."
    for _ in $(seq 1 30); do
        if systemctl is-active --quiet sovereign 2>/dev/null; then
            if "$INSTALL_DIR/sovereign" --version >/dev/null 2>&1; then
                ok "Service ready"
                break
            fi
        fi
        sleep 1
    done
fi

# 9. Print the "what next"
cat <<EOF

✓ sovereign $VERSION is installed. Next, you probably want to:
  → cd /srv/myapp                          # or wherever your app source lives
  → sovereign init                          # detect your framework, write app.yaml
  → sovereign login                         # provision the age master key (V0 single-tenant)
  → sovereign deploy                        # build + ship your app
  → sovereign doctor                        # diagnose the host
  → sovereign --help                        # full command tree
  → cat /var/log/sovereign/sovereign.log    # see the logs

For the docs, see https://sovereignruntime.dev/docs/
EOF
