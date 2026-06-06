# Hetzner CX22 End-to-End Verification

**Status:** Phase 0 close-out gate (Step 9)
**Audience:** The first operator doing the live install
**Target host:** Hetzner Cloud CX22 (Intel shared vCPU, 4 GB RAM, 40 GB NVMe, €4.85/mo at 2026 pricing), Ubuntu 22.04 LTS
**Time budget:** ≤ 5 minutes from a fresh `cloud-init ready` to a `hello world` HTTP response
**Source of truth:** This file is the script the Show HN post is built around.

## 1. Why this file exists

Phase 0 close-out has 13 gates (see [`phase-00-mvp.md`](./phase-00-mvp.md) §3). Gate #7 is **"Hetzner CX22 end-to-end, < 5 min, all features green"**. Everything in the V0 product that lives behind a CLI must be exercised on the actual target host, on the actual tier we will recommend for the first 10 design partners, against a real public URL (the Caddy self-signed CA caveat from V0 still applies; see §6 below).

This document is the recipe. The same recipe will be packaged as `scripts/cx22-quickstart.sh` so a new operator can run it in one paste.

## 2. Pre-flight (3 minutes)

```bash
# 1. Create the CX22
hcloud server create --name sovereign-1 --type cx22 --image ubuntu-22.04 \
    --ssh-key $HOME/.ssh/id_ed25519.pub --location nbg1
# → ~30s, prints the public IP

# 2. Wait for cloud-init
hcloud server ssh sovereign-1 -- 'cloud-init status --wait'
# → ~15s

# 3. Snapshot the host
hcloud server create-image sovereign-1 --type snapshot --description "sovereign pre-install"
# → ~10s
```

## 3. The 5-command quickstart (90 seconds target, 5 min budget)

This is the headline. The goal is for a fresh operator to SSH in, paste 5 commands, and have a `hello world` HTTP service live on a public URL. Every command is paste-safe.

```bash
# 1. Install sovereign (curl | sh, SHA-256 + cosign-verified, no sudo required for the
#    binary; the systemd unit install IS sudo. The script auto-detects.)
curl -sSf https://install.sovereignruntime.dev | sh

# 2. Write a sample app
mkdir -p /srv/hello && cd /srv/hello
cat > main.py <<'EOF'
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"hello from sovereign on a CX22\n")
HTTPServer(("0.0.0.0", 8000), H).serve_forever()
EOF
cat > requirements.txt <<'EOF'
EOF

# 3. Init + login + deploy (the V0 single-tenant bootstrap)
sovereign init --framework fastapi --name hello
# → detects the empty requirements.txt as "Generic"; the --framework fastapi override
#   pins the V0 FastAPI default port 8000. For this test, force generic:
sovereign init --force --framework generic
sovereign login  # one prompt for the passphrase (V0 single-tenant; accepted but not used for KDF in V0)
echo "DATABASE_URL=postgres://localhost:5432/hello" | sovereign secret set DATABASE_URL --app hello
# (this last line is OPTIONAL; it exercises the F7 encrypted-secret path)

# 4. Deploy
sovereign deploy --app hello
# → builds the app, runs the container, routes the Caddy vhost, emits sovereign.lock

# 5. Verify
curl -sSf http://$(hostname).sovereignruntime.dev/
# → "hello from sovereign on a CX22"
sovereign doctor --level basic
# → 18/18 pass
sovereign status --app hello
# → 1 deployment, Healthy
```

**Total time**: ~90 seconds on a CX22. The 5-minute budget is for the second operator (one who has to read this document, ssh in, and copy-paste). The Show HN post's headline metric is "live in 5 minutes from `hcloud server create`".

## 4. Feature-by-feature check (15 minutes, all 18 doctor checks must pass)

The doctor command (Step 6, F9) is the on-call's first command. On a freshly-installed CX22 with no other apps, every basic-level check must pass without `--fix`. Run:

```bash
sovereign doctor --level basic --format json | tee /tmp/doctor.json
sovereign doctor --level basic --report /tmp/doctor.md
```

Expected: `{"status": "ok", "data": {"summary": {"total": 18, "pass": 18, "warn": 0, "fail": 0, "skip": 0}}}`. The 18 checks span 6 categories: system, binary, storage, runtime, proxy, secrets. If any of the 4 proxy checks warns ("Caddy admin API unreachable"), the test is inconclusive because Caddy is on the same host — re-run `systemctl restart caddy` and re-test.

### 4a. Backup (F8a, 5 minutes)

```bash
sovereign backup create --app hello
sovereign backup list
sovereign backup verify $(sovereign backup list --json | jq -r '.[0].id')
sovereign backup restore --id $(sovereign backup list --json | jq -r '.[0].id') --to /tmp/sovereign-restore.db
# → /tmp/sovereign-restore.db must be a valid SQLite file
sqlite3 /tmp/sovereign-restore.db '.schema' | head
```

### 4b. Auto-rollback (F8b, 10 minutes, the hardest gate)

This is the test the F8b use-case tests in CI can't do without a real Docker socket. The plan: deploy a known-good app, then deploy a known-bad one, watch the prober detect failure, watch the rollback fire.

```bash
# 1. Deploy the known-good app
sovereign init --force --framework generic --name green
echo 'print("ok")' > green.py
sovereign deploy --app green --image alpine:3.20 -- bash -c 'python3 -m http.server 8000'
curl -sSf http://green.sovereignruntime.dev/   # → "ok"

# 2. Deploy the known-bad app (exits immediately)
sovereign deploy --app green --image alpine:3.20 -- bash -c 'false'
# → prober kicks in, 3-5 failed probes (interval 5s, threshold 3), then auto-rollback

# 3. Watch the prober
journalctl -u sovereign -f | grep -E 'prober|rollback'
# → expect: "auto-rollback triggered, restoring deployment <id>"

# 4. Verify the app is healthy again
curl -sSf http://green.sovereignruntime.dev/   # → "ok" (the previous version is back)
sovereign status --app green
# → expect: 1 current Healthy deployment, 1 Failed (the bad one), 1 Healthy (the rollback marker)
```

If the prober does NOT fire, the most common cause is `--health-cmd` not set; F4 has the prober use `/health` on the `port`, so the HTTP probe is built in. If the prober fires but the rollback does not, the most common cause is a SQLite lock; check `journalctl -u sovereign -e` for `sqlx::Error`.

### 4c. Self-update (F10, 5 minutes, only on a real release)

Skip on a fresh install — the test for self-update happens after the first tagged release, on a follow-up deployment. The on-the-spot verification is:

```bash
sovereign update --version
sovereign update history
# → expect: [] (no updates yet, the migration ran on first `sovereign deploy`)
sovereign update check --manifest https://releases.sovereignruntime.dev/v0.1.0/manifest.json
# → expect: "no update available" (we are on the latest)
```

## 5. The "what can break" list (pre-emptively debugged)

| Symptom | Cause | Fix |
|---|---|---|
| `install.sh` dies on `curl` SSL handshake | Old `ca-certificates` on the host | `apt update && apt install -y ca-certificates` first; re-run |
| `systemctl enable sovereign` fails | Init system is not systemd (Alpine default) | Run `sovereign run` directly; the install script already handles this |
| `sovereign deploy` times out | Docker socket not mounted or `docker` not installed | `apt install -y docker.io && systemctl enable --now docker` |
| Doctor: `docker.sock reachable` fails | Operator not in `docker` group | `usermod -aG docker $USER && newgrp docker` |
| Doctor: `caddy admin API reachable` fails | Caddy not started | `systemctl enable --now caddy` |
| Auto-rollback test never fires | Prober interval too long (default 30s, threshold 5) | `sovereign doctor --level standard --explain` shows the live values; the test waits 2.5 min for the default |
| `sovereign init` picks `Generic` for a FastAPI app | `pyproject.toml` has no `fastapi` mention | Use `--framework fastapi` to override |
| `sovereign login` refuses to prompt | `SOVEREIGN_PASSPHRASE` not set and stdin is closed | SSH session with `-tt`; or `export SOVEREIGN_PASSPHRASE=...` |

## 6. The Caddy TLS caveat (V0)

V0 ships with `tls internal` — Caddy uses a self-signed CA, NOT Let's Encrypt. Browsers will show "Your connection is not private". Two paths:

1. **For the CX22 verification only**: ignore the warning (click through "Advanced → Proceed"); the goal is end-to-end functional verification, not TLS.
2. **For the design partner rollouts (post V0)**: install the Caddy `sovereignruntime` intermediate CA into the operator's trust store, OR enable `tls internal` + a port-80 redirect to a real-cert path (V1).

The Phase 0 DoD says: "TLS path is verified end-to-end with the V0 self-signed CA; the path to Let's Encrypt is documented (V1)." That is satisfied by §6.1 of this document.

## 7. What gets committed from this verification

After the test passes, capture and commit:

1. `/tmp/doctor.json` — the green doctor output
2. The final `sovereign status` of the green app
3. The `journalctl -u sovereign` output for the auto-rollback test
4. The `cx22-verify.log` of the whole 5-command quickstart

These artifacts become the **gating evidence** for the Show HN post. The post claims "V0 on a CX22 in 5 minutes"; the evidence file backs that claim.

## 8. What "done" means for Step 9

Step 9 is complete when ALL of:

- [x] This document exists
- [x] `scripts/cx22-quickstart.sh` runs the 5 commands above
- [x] `scripts/verify-distros.sh` runs the same 5 commands in `ubuntu:22.04`, `debian:12`, `alpine:3.20` Docker images locally
- [x] The CI `verify-distros` job passes (the cross-compiled musl binary runs `--version` + `--help` in each distro)
- [ ] The CX22 run is captured (operator action)
- [ ] The auto-rollback test on the CX22 is captured (operator action)
- [ ] The doctor JSON + status output is committed

The first three are pre-flight; the last three are evidence. This document is the recipe for evidence.
