#!/usr/bin/env bash
# Sovereign Application Runtime — Hetzner CX22 verification script.
#
# The recipe lives in docs/operations/cx22-verify.md. This script is
# the paste-and-run form. It is idempotent (re-running does not
# clobber existing apps; it just re-prints the status).
#
# Usage:
#   curl -sSf https://install.sovereignruntime.dev | sh
#   sudo install -m 0755 /dev/stdin /usr/local/bin/cx22-verify.sh \
#     < <(curl -sSf https://raw.githubusercontent.com/.../scripts/cx22-quickstart.sh)
#   cx22-verify.sh [--name hello] [--port 8000] [--skip-install]
#
# Exit code 0 = all 5 phases green. Non-zero = first failed phase.
#
# Time budget on a CX22: 90 seconds for a fresh install, 5 minutes
# for a first-time operator. The doctor + backup + auto-rollback
# tests (F8a/F8b/F9) are run separately; this script is the headline
# 5-command quickstart.

set -euo pipefail

# Defaults
APP_NAME="${APP_NAME:-hello}"
APP_PORT="${APP_PORT:-8000}"
SKIP_INSTALL=0
LOG_DIR="/var/log/sovereign"
LOG_FILE="${LOG_DIR}/cx22-verify.log"

# Parse args
while [ $# -gt 0 ]; do
    case "$1" in
        --name) APP_NAME="$2"; shift 2 ;;
        --port) APP_PORT="$2"; shift 2 ;;
        --skip-install) SKIP_INSTALL=1; shift ;;
        -h|--help)
            sed -n '2,30p' "$0" | sed 's/^# //; s/^#//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# Ensure log dir
mkdir -p "$LOG_DIR" 2>/dev/null || LOG_FILE="$HOME/cx22-verify.log"

# Tee everything to the log
exec > >(tee -a "$LOG_FILE") 2>&1

info()  { echo -e "\033[36m→ $*\033[0m"; }
ok()    { echo -e "\033[32m✓ $*\033[0m"; }
die()   { echo -e "\033[31m✗ $*\033[0m" >&2; exit 1; }
banner(){ echo -e "\n\033[1;34m═══ $* ═══\033[0m"; }

# Sanity
[ "$(id -u)" -eq 0 ] && die "do NOT run as root; the install script will sudo when needed"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v jq  >/dev/null 2>&1 || info "(jq not installed; some JSON output will be raw)"

banner "Phase 0: preflight"
uname -a
[ -d /sys/class/dmi/id ] && grep -q 'Hetzner' /sys/class/dmi/id/sys_vendor 2>/dev/null \
    && info "Hetzner hardware detected" \
    || info "(not a Hetzner host; running on $(uname -m) $(uname -o) — fine, the script is portable)"
command -v docker >/dev/null 2>&1 || info "docker not installed; F4 deploy will fail until you run 'apt install -y docker.io'"

# 1. Install sovereign
if [ "$SKIP_INSTALL" -eq 0 ] && ! command -v sovereign >/dev/null 2>&1; then
    banner "Phase 1/5: install sovereign"
    info "curl | sh the install script (SHA-256 + cosign-verified)"
    curl -sSf https://install.sovereignruntime.dev | sh || die "install failed"
    ok "sovereign installed: $(sovereign --version 2>&1)"
else
    banner "Phase 1/5: install sovereign (skipped)"
    info "sovereign already present: $(sovereign --version 2>&1)"
fi

# 2. Write a sample app
banner "Phase 2/5: write a sample app"
APP_DIR="/srv/${APP_NAME}"
mkdir -p "$APP_DIR"
cd "$APP_DIR"
cat > main.py <<EOF
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"hello from sovereign on a $(uname -n) (CX22 verify $(date -Iseconds))\n")
HTTPServer(("0.0.0.0", ${APP_PORT}), H).serve_forever()
EOF
: > requirements.txt
ok "wrote $APP_DIR/main.py (port ${APP_PORT})"

# 3. Init + login + (optional) secret
banner "Phase 3/5: sovereign init + login"
[ -f app.yaml ] || sovereign init --framework generic --name "$APP_NAME"
ok "app.yaml present"
if [ ! -f /var/lib/sovereign/master.key ] && [ ! -f "$HOME/.local/share/sovereign/master.key" ]; then
    info "no master key found; running 'sovereign login'"
    if [ -n "${SOVEREIGN_PASSPHRASE:-}" ]; then
        sovereign login --no-input || die "login failed"
    else
        info "you will be prompted for a passphrase (V0: accepted but not used for KDF)"
        sovereign login || die "login failed"
    fi
else
    info "master key already present; skipping login"
fi
ok "V0 single-tenant bootstrap complete"

# 4. Deploy
banner "Phase 4/5: sovereign deploy"
sovereign deploy --app "$APP_NAME" --port "$APP_PORT" || die "deploy failed"
ok "deploy complete; check sovereign.lock for the receipt"

# 5. Verify
banner "Phase 5/5: verify"
HOSTNAME_SHORT="$(hostname -s)"
URL="http://${HOSTNAME_SHORT}.sovereignruntime.dev/"
info "GET $URL"
sleep 2  # give Caddy a moment to pick up the new route
if curl -sSf --max-time 10 "$URL"; then
    ok "GET $URL returned 200"
else
    info "(curl failed; this is expected if DNS for *.sovereignruntime.dev is not pointed at this host's IP)"
    info "fall back to direct container health check:"
    if curl -sSf --max-time 5 "http://127.0.0.1:${APP_PORT}/"; then
        ok "direct health check (127.0.0.1:${APP_PORT}) returned 200"
    else
        die "neither the public URL nor the direct health check responded; see $LOG_FILE"
    fi
fi

# Doctor (the on-call's first command)
info "running doctor --level basic (the 18-check baseline)"
sovereign doctor --level basic
ok "doctor: HEALTHY (or check the output for any warns/fails)"

banner "Done"
ok "sovereign is live on this host. Next, you probably want to:"
cat <<EOF
  → sovereign status --app ${APP_NAME}        # see the deployment
  → sovereign doctor --level standard         # 40+ checks (V1)
  → sovereign backup create --app ${APP_NAME}  # F8a: snapshot the live DB
  → journalctl -u sovereign -f                # tail the logs
  → sovereign --help                          # the full command tree

Full log: $LOG_FILE
EOF
