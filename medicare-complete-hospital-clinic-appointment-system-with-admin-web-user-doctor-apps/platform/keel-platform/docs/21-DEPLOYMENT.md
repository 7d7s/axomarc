# 21 — Deployment (Forge, Nginx, CI/CD, Environments, Rollback)

> From `git push` to live in 5 minutes. Production-grade, repeatable, observable.

---

## 1. Environments

| Env | URL | DB | Hosted on |
|---|---|---|---|
| **Local** | `localhost:8000` | Docker MySQL | Developer machine |
| **CI** | ephemeral | SQLite in-memory | GitHub Actions |
| **Staging** | `staging-api.medicare.app` | `medicare_staging` | Hetzner CX21 (€30/mo) |
| **Production** | `api.medicare.app` (apex) + per-tenant subdomains | `medicare_prod` | Hetzner CCX23 (€60/mo) × 2 (active + standby) |

### 1.1 Per-Env Resources

| | Staging | Production |
|---|---|---|
| API server | 1× Hetzner CX21 | 2× Hetzner CCX23 (active-active behind LB) |
| Reverb server | 1× Hetzner CX11 | 2× Hetzner CX11 |
| Database | Hetzner managed MySQL 8 | Hetzner managed MySQL 8 (with replica) |
| Redis | Hetzner managed Redis 7 | Hetzner managed Redis 7 |
| Object storage | Cloudflare R2 | AWS S3 (EU) |
| SMTP | Mailgun sandbox | Mailgun production |
| Sentry | `medicare-staging` | `medicare-prod` |
| Firebase | `medicare-staging` | `medicare-prod` |
| Razorpay/Stripe | Test mode | Live mode |
| Backup retention | 7 days | 30 days online + 1 year Glacier |

---

## 2. The 4 Repositories

| Repo | Tech | Deploys to | Tool |
|---|---|---|---|
| `medicare-api` | Laravel 11 | Hetzner via Forge | Forge |
| `medicare-web` | Next.js 15 | Vercel | Vercel (or Hetzner) |
| `medicare-user-app` | Flutter | Play Store + App Store | Fastlane + Codemagic |
| `medicare-doctor-app` | Flutter | Play Store + App Store | Fastlane + Codemagic |

### 2.1 Why 4 Repos

- Each has different release cadence (API can ship daily; mobile apps are weekly).
- Different access control (different teams own different repos).
- Different CI requirements (Laravel vs Next.js vs Flutter).

### 2.2 Repo Topologies

- All on GitHub.
- `main` branch is always deployable.
- Feature branches → PR → review → squash merge.
- Tags trigger releases for mobile apps.

---

## 3. CI (GitHub Actions)

### 3.1 API (`medicare-api`)

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    services:
      mysql:
        image: mysql:8
        env:
          MYSQL_ROOT_PASSWORD: root
          MYSQL_DATABASE: medicare_test
        ports: ['3306:3306']
        options: --health-cmd="mysqladmin ping" --health-interval=5s
      redis:
        image: redis:7
        ports: ['6379:6379']
    steps:
      - uses: actions/checkout@v4
      - uses: shivammathur/setup-php@v2
        with: { php-version: '8.3', extensions: mbstring, intl, bcmath, pdo_mysql, redis }
      - run: composer install --prefer-dist --no-progress
      - run: php artisan key:generate
      - run: php artisan migrate --env=testing
      - name: Lint
        run: ./vendor/bin/pint --test
      - name: Static analysis
        run: ./vendor/bin/phpstan analyse --memory-limit=2G --level=8
      - name: Security audit
        run: composer audit
      - name: Tests
        run: php artisan test --parallel --coverage-clover=coverage.xml
        env:
          DB_CONNECTION: mysql
          DB_HOST: 127.0.0.1
          DB_DATABASE: medicare_test
          REDIS_HOST: 127.0.0.1
      - name: Upload coverage
        if: matrix.php == '8.3'
        uses: codecov/codecov-action@v4
```

### 3.2 Web (`medicare-web`)

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v3
        with: { version: 9 }
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: pnpm }
      - run: pnpm install --frozen-lockfile
      - run: pnpm turbo run lint
      - run: pnpm turbo run typecheck
      - run: pnpm turbo run test
      - run: pnpm turbo run build
```

### 3.3 Mobile (`medicare-user-app`, `medicare-doctor-app`)

Codemagic CI:
- Runs on every push.
- Unit tests (`flutter test`).
- Build for Android (debug + release APK + AAB).
- Build for iOS (no signing for dev; signed for release).
- Code coverage uploaded to Codecov.

### 3.4 Shared Quality Gates

Every repo:
- Linter passes
- Type check passes
- Tests pass (with required coverage threshold)
- Security scan clean
- No secrets in diff (`gitleaks`)

---

## 4. CD — API (Forge + Deploy Script)

### 4.1 Laravel Forge Setup

- Server: Hetzner CX21/CCX23, Ubuntu 24.04 LTS, PHP 8.3, MySQL 8, Redis 7.
- Forge provisions in ~5 minutes.
- Auto-installs Nginx, PHP-FPM, Supervisor, Certbot.
- Daily backups to S3.

### 4.2 Forge Deployment

Forge has a `Deploy Script` field. Our script:

```bash
# forge deploy script
cd /home/forge/medicare-api

# Enable maintenance mode
php artisan down --secret="1630542a-246b-4ddf-bc83-7032c1100e08"

# Pull latest
git pull origin main

# Install deps
composer install --no-dev --no-interaction --prefer-dist --optimize-autoloader

# Run migrations
php artisan migrate --force

# Clear caches
php artisan config:cache
php artisan route:cache
php artisan event:cache
php artisan view:cache

# Restart workers
php artisan queue:restart
php artisan horizon:terminate

# Reload PHP-FPM (for opcache)
sudo -S service php8.3-fpm reload

# Bring back up
php artisan up

# Done
echo "Deployment complete: $(date)"
```

Deployments are triggered by:
- GitHub webhook on `main` push (auto-deploy to staging).
- Manual click in Forge UI for production (after smoke test on staging).

### 4.3 Zero-Downtime Considerations

- `php artisan down` returns 503 for ~3-5 seconds.
- For larger changes, use **load balancer drain**: take server out of LB pool, deploy, health check, put back.
- Database migrations are backward-compatible (add columns, don't drop).
- Rolling restart of PHP-FPM workers via `php artisan queue:restart`.

### 4.4 Rollback

Forge keeps the last 5 releases. To rollback:

```bash
# Via Forge UI: click "Rollback to previous release"
# Or manually:
cd /home/forge/medicare-api
git checkout HEAD~1
composer install --no-dev
php artisan migrate  # if down-migrations are needed
php artisan config:cache
php artisan queue:restart
```

Database rollbacks are manual and require care — see `15-DATABASE-SCHEMA.md` §9.4.

### 4.5 The .env on Production

- Never committed.
- Generated from AWS Secrets Manager at deploy time by a Forge pre-deploy script:

```bash
php artisan secrets:load --from=aws
```

Or stored directly in Forge's "Environment" UI (encrypted at rest).

---

## 5. CD — Web (Vercel)

### 5.1 Setup

- Vercel project per app: `medicare-admin`, `medicare-patient-web`.
- Or one monorepo with 2 apps (Turborepo detects).
- Build command: `pnpm turbo run build --filter=admin` (or similar).
- Output: `.next/` static + serverless.
- Domain: `admin.medicare.app`, `medicare.app` (patient).

### 5.2 Env Vars

In Vercel project settings:
- `NEXT_PUBLIC_API_URL`
- `NEXT_PUBLIC_REVERB_KEY`
- etc.

### 5.3 Deploy

- Push to `main` → Vercel builds → preview URL.
- Tag release → production deploy.

### 5.4 Alternative: Self-Host on Hetzner

If the customer requires on-prem (e.g., for data residency), deploy as a Docker container behind the same Nginx as the API:

```dockerfile
# Dockerfile
FROM node:20-alpine AS builder
WORKDIR /app
COPY . .
RUN pnpm install --frozen-lockfile
RUN pnpm turbo run build --filter=patient-web

FROM node:20-alpine AS runner
WORKDIR /app
COPY --from=builder /app/apps/patient-web/.next/standalone ./
COPY --from=builder /app/apps/patient-web/.next/static ./.next/static
COPY --from=builder /app/apps/patient-web/public ./public
EXPOSE 3000
CMD ["node", "server.js"]
```

---

## 6. CD — Mobile (Fastlane + Codemagic)

### 6.1 Codemagic (CI for Flutter)

- Triggers on tag push (`v1.2.3`).
- Runs tests.
- Builds:
  - Android: AAB for Play Store, APK for direct distribution
  - iOS: IPA for TestFlight + App Store
- Code-signed via Codemagic's signing infrastructure.

### 6.2 Fastlane (Release Management)

- `fastlane android release` → uploads to Play Console (internal track first, then staged rollout).
- `fastlane ios release` → uploads to App Store Connect (TestFlight first).
- Release notes auto-generated from commits since last tag.

### 6.3 Release Cadence

- **Internal builds**: every PR merged to main.
- **Beta builds**: weekly (Wednesdays), distributed via Firebase App Distribution + TestFlight.
- **Production releases**: every 2-4 weeks, with staged rollout (10% → 50% → 100% over 7 days).

### 6.4 Force Update

The app checks `settings.mobile_user.min_version` on launch. If below, the user sees a "Please update" screen with a link to the store.

---

## 7. Nginx Config (API)

```nginx
# /etc/nginx/sites-available/medicare-api
server {
    listen 80;
    server_name api.medicare.app *.medicare.app;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name api.medicare.app *.medicare.app;
    
    ssl_certificate /etc/letsencrypt/live/medicare.app/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/medicare.app/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;
    ssl_prefer_server_ciphers on;
    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 1d;
    
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains; preload" always;
    add_header X-Frame-Options "DENY" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    add_header Content-Security-Policy "default-src 'self'; frame-ancestors 'none'" always;
    
    # Real IP from Cloudflare (if used) or proxy
    set_real_ip_from 10.0.0.0/8;
    real_ip_header X-Forwarded-For;
    
    # Rate limit zone
    limit_req_zone $binary_remote_addr zone=api_limit:10m rate=60r/m;
    limit_req zone=api_limit burst=20 nodelay;
    
    # API
    location / {
        limit_req zone=api_limit burst=20 nodelay;
        try_files $uri $uri/ /index.php?$query_string;
        fastcgi_pass unix:/var/run/php/php8.3-fpm.sock;
        include fastcgi_params;
        fastcgi_param SCRIPT_FILENAME $document_root/index.php;
        fastcgi_read_timeout 60s;
    }
    
    # Reverb WebSocket
    location /app {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_read_timeout 86400s;
    }
    
    # Static files (user uploads, served via signed URLs from S3 in prod)
    location /storage {
        alias /home/forge/medicare-api/storage/app/public;
        expires 1y;
        add_header Cache-Control "public, immutable";
    }
    
    # Health check (no rate limit)
    location = /health {
        access_log off;
        try_files $uri /index.php?$query_string;
        fastcgi_pass unix:/var/run/php/php8.3-fpm.sock;
    }
    
    # Hide sensitive files
    location ~ /\.(?!well-known).* { deny all; }
}
```

---

## 8. Supervisor (Queue Workers + Reverb)

```ini
# /etc/supervisor/conf.d/medicare-queue.conf
[program:medicare-queue]
process_name=%(program_name)s_%(process_num)02d
command=php /home/forge/medicare-api/artisan queue:work redis --sleep=1 --tries=3 --max-time=3600 --backoff=30
numprocs=4
autostart=true
autorestart=true
stopwaitsecs=3600
user=forge
stdout_logfile=/home/forge/.forge/medicare-queue.log
stopwaitsecs=3600

# /etc/supervisor/conf.d/medicare-horizon.conf
[program:medicare-horizon]
process_name=%(program_name)s
command=php /home/forge/medicare-api/artisan horizon
numprocs=1
autostart=true
autorestart=true
user=forge
stdout_logfile=/home/forge/.forge/medicare-horizon.log

# /etc/supervisor/conf.d/medicare-reverb.conf
[program:medicare-reverb]
process_name=%(program_name)s
command=php /home/forge/medicare-api/artisan reverb:start --host=0.0.0.0 --port=8080
numprocs=1
autostart=true
autorestart=true
user=forge
stdout_logfile=/home/forge/.forge/medicare-reverb.log

# /etc/supervisor/conf.d/medicare-scheduler.conf
[program:medicare-scheduler]
process_name=%(program_name)s
command=php /home/forge/medicare-api/artisan schedule:work
numprocs=1
autostart=true
autorestart=true
user=forge
stdout_logfile=/home/forge/.forge/medicare-scheduler.log
```

---

## 9. The Scheduler (Cron)

Laravel's scheduler is the single entry point:

```php
// app/Console/Kernel.php
protected function schedule(Schedule $schedule): void {
    $schedule->command('queue:prune-failed --hours=48')->daily();
    $schedule->job(new ExpirePendingAppointmentsJob())->everyMinute();
    $schedule->job(new MarkNoShowJob())->everyFiveMinutes();
    $schedule->job(new AppointmentReminder24hJob())->hourly();
    $schedule->job(new AppointmentReminder2hJob())->everyFifteenMinutes();
    $schedule->job(new ReleaseExpiredHoldsJob())->everyMinute();
    $schedule->job(new InvoiceOverdueReminderJob())->dailyAt('09:00');
    $schedule->job(new SyncPendingPaymentsJob())->everyFiveMinutes();
    $schedule->job(new GenerateFutureSlotsJob())->daily();
    $schedule->job(new AuditIntegrityCheckJob())->daily();
    $schedule->job(new DataArchiveJob())->weekly();
    $schedule->job(new BackupPerTenantJob())->dailyAt('02:00');
}
```

`schedule:work` runs these in a loop. Supervisor keeps it alive.

---

## 10. Monitoring & Observability

### 10.1 Metrics (Prometheus)

`promphp/prometheus_client_php` exposes `/metrics` for Prometheus to scrape.

Custom metrics:
- `medicare_api_requests_total{route,method,status}`
- `medicare_api_request_duration_seconds{route}`
- `medicare_appointments_created_total`
- `medicare_payments_captured_total{gateway}`
- `medicare_queue_depth{queue}`
- `medicare_reverb_connections`
- `medicare_active_users_5m`

### 10.2 Dashboards (Grafana)

- API Performance (latency p50/p95/p99 by route)
- Error Rate (4xx/5xx over time)
- Queue Health (depth, processing time, failed jobs)
- Business KPIs (bookings/hour, revenue/hour, active doctors)
- Infrastructure (CPU, RAM, disk, network)

### 10.3 Logs

Centralized via `logtail` or `papertrail`:
- `storage/logs/laravel.log` → tailed
- `~/.forge/*.log` → tailed
- Nginx access + error → tailed

Sentry for exceptions.

### 10.4 Alerts (PagerDuty)

Already listed in 19-INTEGRATIONS §10. Same rules.

---

## 11. Pre-Deploy Checklist

```markdown
## Pre-Deploy Checklist (Production)

- [ ] All tests pass on staging
- [ ] OWASP ZAP scan clean
- [ ] PHPStan level 8 clean
- [ ] composer audit clean
- [ ] DB migrations reviewed for backward compat
- [ ] New env vars documented in 19-INTEGRATIONS §13
- [ ] Secrets updated in AWS Secrets Manager
- [ ] Sentry release tag set
- [ ] Changelog updated
- [ ] Customer-facing release notes drafted
- [ ] Rollback plan documented in PR
- [ ] On-call notified
- [ ] Deploy window chosen (low traffic)
```

---

## 12. Post-Deploy Verification

Within 15 minutes of deploy:
- [ ] Health check `/health/ready` returns 200
- [ ] Synthetic booking test (test patient books, test doctor sees it) passes
- [ ] Sentry error rate unchanged from baseline
- [ ] Queue depth stable
- [ ] Reverb accepts new connections
- [ ] Dashboard shows expected counts

If any fails → page on-call, consider rollback.

---

## 13. Feature Flags & Gradual Rollout

We use `config('medicare.features.X')` (env-based) for coarse-grained flags.

For finer control, we have a simple `FeatureFlag` table (or use Laravel Pennant):

```php
Feature::define('new-booking-flow', function (User $user) {
    return $user->id % 10 === 0;  // 10% of users
});
```

Used for:
- Canary releases (1% → 10% → 50% → 100%)
- Per-tenant beta opt-in
- A/B tests (V2)

---

## 14. Database Migration Strategy

### 14.1 Backward-Compatible Only

Never:
- Drop a column in the same deploy that removes code that uses it.
- Rename a column.
- Add a NOT NULL column without a default.

Always:
- Add nullable columns, backfill in a separate deploy, then add NOT NULL.
- Use `expand-contract` pattern (https://martinfowler.com/bliki/ParallelChange.html).

### 14.2 Long Migrations

For >1 minute on production data:
- `php artisan make:migration add_index_to_X --table=X`
- Use `ALTER TABLE ... ALGORITHM=INPLACE, LOCK=NONE` for MySQL (no table lock).
- Run during low traffic.
- Or use `pt-online-schema-change` for huge tables.

---

## 15. Disaster Recovery Drills

| Drill | Frequency | What we do |
|---|---|---|
| Server down | Monthly | Kill an API instance, verify LB drains and recovers |
| DB restore | Quarterly | Restore from backup to a sandbox, run smoke tests |
| Region failover | Annually | Simulate AWS region down, verify failover |
| Ransomware | Annually | "Encrypt" S3, restore from backup |
| Bad deploy | Quarterly | Deploy a known-broken commit, verify rollback works |

---

## 16. The Team's Runbook

(Pinned in #engineering Slack channel.)

```
INCIDENT: API down
1. Check status page (status.medicare.app)
2. Check BetterUptime alert
3. SSH to server: ssh forge@medicare-prod-1
4. Check: tail -f /home/forge/.forge/medicare-queue.log
5. Check: sudo systemctl status php8.3-fpm
6. Check: sudo systemctl status nginx
7. Check: top, df -h, free -h
8. Check Sentry for new errors
9. If deploy-related: forge rollback
10. Communicate in #incidents

INCIDENT: Payment webhook failure
1. Check processed_webhooks table for last successful
2. Check gateway dashboard (Razorpay, Stripe, Paystack)
3. Check our log for signature verification failures
4. If gateway down: notifications → support team
5. If our code: rollback or hotfix
6. Run sync job manually: php artisan payments:sync-pending

INCIDENT: Tenant data leak
1. IMMEDIATELY: revoke all tokens for affected tenant
2. Page security team
3. Snapshot DB for forensics
4. Pull audit_logs for the affected time range
5. Identify scope
6. Notify affected customers (data controllers)
7. File regulatory notifications within 72h
8. Post-mortem within 5 business days
```

---

## 17. Acceptance Criteria (V1)

- [ ] `git push` to main → auto-deploys to staging within 5 min.
- [ ] Manual click in Forge → deploys to production within 3 min.
- [ ] Health check `/health/ready` validates DB + Redis + Reverb.
- [ ] Rollback completes within 2 min and restores service.
- [ ] Queue workers survive deploys (no message loss).
- [ ] Migrations are backward-compatible (verified by CI).
- [ ] Monitoring dashboards show all key metrics.
- [ ] On-call gets paged for any Sev 1 within 1 minute.
- [ ] Disaster recovery drill is run quarterly.

---

## 18. V2 Deployment

> V1 ships a single Hetzner VPS + Vercel + Codemagic topology. V2 introduces **K8s/EKS**, **sidecar services** (AI, lab-gateway, FHIR, ICE), **multi-region DR**, and a **GitOps** deployment pipeline.

### 18.1 V2 target topology

```
                     ┌──────────────────────────┐
                     │  Cloudflare (CDN + WAF)  │
                     └────────────┬─────────────┘
                                  │
                     ┌────────────▼─────────────┐
                     │  Route53 (DNS + health)  │
                     └────────────┬─────────────┘
                                  │
        ┌─────────────────────────┼─────────────────────────┐
        ▼                         ▼                         ▼
┌──────────────┐          ┌──────────────┐          ┌──────────────┐
│  us-east-1   │          │  eu-west-1   │          │  ap-south-1  │
│  (active)    │          │  (active)    │          │  (active)    │
│              │          │              │          │              │
│  EKS cluster │          │  EKS cluster │          │  EKS cluster │
│  - medicare- │          │  - medicare- │          │  - medicare- │
│    api (×3)  │          │    api (×3)  │          │    api (×3)  │
│  - medicare- │          │  - realtime  │          │  - realtime  │
│    realtime  │          │    (×2)      │          │    (×2)      │
│    (×2)      │          │  - search    │          │  - search    │
│  - search    │          │              │          │              │
│  - fhir-gw   │          │  RDS MySQL   │          │  RDS MySQL   │
│  - lab-gw    │          │  ElastiCache │          │  ElastiCache │
│              │          │              │          │              │
│  RDS MySQL   │          │              │          │              │
│  ElastiCache │          │              │          │              │
│  S3 (region) │          │  S3 (region) │          │  S3 (region) │
└──────┬───────┘          └──────┬───────┘          └──────┬───────┘
       │                         │                         │
       │              ┌──────────┴──────────┐              │
       └──────────────┤  Cross-region read  ├──────────────┘
                      │  replicas (DR)      │
                      └─────────────────────┘

        ┌─────────────────────────────────────┐
        │  Global services (any region)       │
        │  - medicare-ai-service (GPU)        │
        │  - medicare-lab-gateway (Java)      │
        │  - coturn TURN/STUN (per region)    │
        │  - LiveKit Cloud (SaaS)             │
        └─────────────────────────────────────┘
```

### 18.2 EKS vs Hetzner

**For new V2 SaaS tenants:** EKS in the tenant's region.

**For V1 self-hosted tenants (perpetual license):** keep Hetzner + Forge, optionally migrate to K8s.

**Migration path:** `php artisan medicare:migrate-to-k8s` generates Helm charts, Terraform, and a runbook. Tenant can run on a hybrid (Hetzner DB + EKS app) during transition.

### 18.2a Plugin execution model

Per `14-MODULE-V2-STUBS.md` §3.3a execution model:

- **V2 (Plugin Core):** **in-process.** A plugin is a PHP class loaded via the `PluginManager` service, invoked inside the same PHP-FPM worker that handles the originating API request. Timeout 5 s, memory cap 32 MB, all exceptions caught (per `20-SECURITY-COMPLIANCE.md` §23.2.3). This keeps ops simple: one deploy, one scaling profile, no inter-process IPC.
- **V3 (Public Marketplace, Plugin Monetization):** **sidecar mode optional**, gated by `feature_plugin_sidecar`. A sidecar plugin runs in a separate PHP-FPM worker pool with no direct DB connection; it must call our internal API for all reads/writes. Stricter memory cap (16 MB), stricter egress filter (deny-all by default, allowlist per plugin), and mTLS between app and sidecar via SPIFFE IDs (per the existing sidecar services above).
- **Operational impact:** the V2 in-process mode has no extra pods. The V3 sidecar mode adds **1 pod per enabled plugin per tenant** (capped at 10 pods per tenant by default, raised on request for Pro and Enterprise tiers). Resource budget for the sidecar pool is added to the existing `medicare-api` HPA target.

### 18.3 Service deployment

Each sidecar service has its own:
- Dockerfile (multi-stage, distroless final).
- Helm chart (deployment, service, ingress, HPA, PDB, ServiceAccount, NetworkPolicy).
- Kustomize overlay per environment.
- ArgoCD application (GitOps).

```yaml
# helm/medicare-api/values.yaml
replicaCount: 3
image:
  repository: 123456789.dkr.ecr.us-east-1.amazonaws.com/medicare-api
  tag: 1.2.3
resources:
  requests: { cpu: 500m, memory: 1Gi }
  limits:   { cpu: 2,    memory: 4Gi }
autoscaling:
  minReplicas: 3
  maxReplicas: 20
  targetCPUUtilizationPercentage: 70
postgresql:
  host: medicare-rds.cluster-abc.us-east-1.rds.amazonaws.com
redis:
  host: medicare-elasticache.abc.cache.amazonaws.com
secrets:
  provider: aws-secrets-manager
```

### 18.4 GitOps with ArgoCD

```yaml
# argocd/medicare-api-prod.yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: medicare-api-prod
  namespace: argocd
spec:
  project: medicare
  source:
    repoURL: https://github.com/medicare/medicare-helm
    path: charts/medicare-api
    helm:
      valueFiles:
        - values-prod-us-east-1.yaml
  destination:
    server: https://kubernetes.default.svc
    namespace: medicare
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
  revisionHistoryLimit: 10
```

**Promotion flow:** dev → staging (auto on merge to main) → canary (5% in prod, watch SLOs for 1h) → full prod.

### 18.5 Multi-region DR — `multi-region-dr` (V2, MML 3)

**1. Overview.** V1 runs in a single region (e.g., `ap-south-1` for India, `us-east-1` for the US, `eu-west-1` for the EU). V2 promotes an **active-passive** warm-standby in a second region for the Enterprise tier. Per `14-V2-Stubs §10 Q1` decision: **active-passive** (not active-active) for V1–V4. Active-active is re-evaluated at V5+ once we have ≥ 3 regions in production and the conflict-resolution cost is justified. MML 3 GA: RPO 5 min, RTO 1 h, quarterly DR drill, region-scoped tenancy, encrypted-at-rest replica in a region the DR region cannot decrypt.

**2. Architecture.**
```
   Primary region (e.g., ap-south-1)               Secondary region (e.g., ap-southeast-1)
   ┌───────────────────────────────┐              ┌───────────────────────────────┐
   │ ALB → K8s API + workers       │              │ K8s (scaled-to-zero API tier,  │
   │ RDS primary (writes)          │──binlog─────▶│   warm workers)               │
   │ ElastiCache primary           │   async      │ RDS read replica (promotable)  │
   │ S3 (active bucket)            │   S3 CRR ──▶ │ S3 (replica bucket)            │
   │ KMS key (region-A, tenant)    │              │ KMS key (region-B, ciphertext │
   │                               │              │   from region-A; cannot       │
   │                               │              │   decrypt without region-A    │
   │                               │              │   key grant)                  │
   └───────────────────────────────┘              └───────────────────────────────┘
            ▲                                              ▲
            │         Route53 active-passive                │
            └──────────────────────────────────────────────┘
```

**3. Targets:**
- **RPO:** 5 min (continuous binlog shipping; S3 cross-region replication lag < 5 min; Redis async replication < 1 min).
- **RTO:** 1 h (Route53 DNS failover + RDS replica promotion + K8s rollout + KMS key grant for decrypt + S3 bucket policy swap).
- **DR drill:** quarterly, automated, measured; the most recent drill's RPO/RTO are reported in `/admin/ops/dr-status` and on the public status page.

**4. Tenant region-pinning.** A tenant's `tenants.region` is set at provisioning and is **immutable** for the lifetime of the tenant. The `tenants.region` is the primary region for all writes. Tenants cannot migrate regions without a re-provisioning flow (V3+ `region-migration`, TBD). The `TenantRouter` (per `03-Tenancy §17.5`) maps the tenant to its region at request time; reads from the secondary region are not allowed for that tenant (the secondary is for DR only, not for read scaling in V2).

**5. KMS + encryption-at-rest.** Each tenant has a per-tenant KMS key in their primary region (per `03-Tenancy §17.6`). The DR region's replica is encrypted **at the application layer** before being written to the DR region: the tenant's data is encrypted with a key that lives only in the primary region; the DR region stores only the ciphertext blob. On failover, the secondary region must be granted the decrypt capability via a `KmsKeyGrant` from the primary — this grant is held in cold storage (an S3 object in the primary region, replicated to a third "break-glass" region) and is applied by the failover runbook. This guarantees that **the secondary region cannot read tenant data without an explicit grant**, even if the secondary is compromised.

**6. Failover flow (the 1-hour runbook).**
```
T+0    : Route53 health check fails (3 consecutive 30s checks).
T+30s  : CloudWatch alarm fires; PagerDuty pages the on-call SRE.
T+2m   : SRE confirms; clicks "Promote DR" in the runbook console.
T+3m   : Lambda #1: update Route53 to point to DR ALB.
T+5m   : Lambda #2: promote RDS read replica (RDS API).
T+7m   : Lambda #3: apply KMS key grant from primary to DR.
T+10m  : Lambda #4: swap S3 bucket policy to point to DR bucket.
T+12m  : K8s: scale up API tier in DR (HPA from 0 → N).
T+15m  : Health check: synthetic tenant load passes.
T+30m  : Customer-facing status page updates "All systems normal".
T+1h   : RTO target met. Post-incident review opened.
```

**7. Failback.** Reverse the process. Any writes that happened in the DR region during the outage are exported as a logical-replication stream and replayed into the (now-recovered) primary. The replay is idempotent (uses ULIDs as the dedupe key).

**8. DR drill (quarterly).** A scheduled chaos-engineering event:
- `chaos-schedule` injects a region-level failure for the staging environment (a non-production replica).
- The same runbook runs against staging; RPO/RTO are measured automatically.
- A pass/fail report is written to `audit` with `action='dr.drill.completed'`.
- A failed drill opens a Sev-2 incident and a remediation sprint.

**9. What this module is NOT:**
- Not multi-region active-active. That is V5+ if/when the conflict-resolution cost is justified.
- Not read-scaling. The DR region does not serve reads. Read-scaling is a single-region read replica (V1).
- Not region migration. Tenants cannot move regions; a future `region-migration` module (TBD) would handle that.

**10. Acceptance Criteria.**
- [ ] RPO < 5 min, verified by a 4-hour chaos drill monthly.
- [ ] RTO < 1 h, verified by the quarterly DR drill.
- [ ] The secondary region cannot decrypt tenant data without an explicit `KmsKeyGrant`.
- [ ] Tenants pinned to a region cannot have their writes silently routed to a different region.
- [ ] The DR drill report is generated automatically and archived for 7 years (HIPAA).
- [ ] The status page updates within 30 min of a real failover (or simulated drill).
- [ ] Failback is idempotent: any writes that happened in DR are replayed cleanly into the recovered primary.

**11. Real-World Test Scenarios.**
- *Real outage:* `ap-south-1` has a network partition; Route53 fails over to `ap-southeast-1`; KMS grant applied; S3 swapped; 14 minutes total downtime (under the 1 h RTO); patients see a brief "Service recovering" banner; status page updated at T+18m.
- *Chaos drill:* in staging, the primary API tier is killed at 02:00 UTC; the runbook promotes DR; the SRE on-call follows the runbook manually; RPO measured at 2 m 18 s, RTO at 47 m; report archived.
- *Corrupt replica:* during a real failover, a row in the DR replica is corrupt (RDS replication lag spike); the application retries the read against a different replica; the audit row is written; the corrupt row is quarantined and a Sev-3 is opened.
- *Region migration attempt (must fail):* a malicious tenant admin tries to write a `tenants.region` change via the API; the API returns 403; the audit row is written with `action='tenants.region.change_attempted'`.
- *DR region read attempt:* a request lands in `ap-southeast-1` for an `ap-south-1` tenant (e.g., via a misconfigured DNS); the `TenantRouter` rejects the read; the audit row is written; the request is rerouted.

**12. Dev Workflow.** `php artisan dr:drill --region=ap-south-1 --target=ap-southeast-1 --dry-run` plans a drill without actually failing over. `php artisan dr:status` reports the last RPO/RTO measurement. `php artisan dr:grant-kms --tenant=...` manually applies a KMS grant (used in real failover; rarely needed in normal ops). Code: `app-modules/multi-region-dr/` (K8s manifests, Terraform, runbook scripts); tests: `tests/Dr/{Failover,Failback,RegionPin,KmsGrant}Test.php` and `tests/Chaos/region-partition.chaos.yml` (Gremlin/AWS FIS).

### 18.6 AI service deployment

- **GPU nodes:** `p4d.24xlarge` (8× A100 40GB) in `us-east-1` and `eu-west-1`.
- **vLLM** server with model weights on S3, loaded to GPU on cold start.
- **Auto-scale to zero** when no requests for 5 min (cost saving).
- **Model registry:** MLflow, with versioned prompts and weights.
- **A/B test:** 10% of traffic to new model version; measure quality metrics.
- **Inference endpoint:** `https://ai.medicare.app/v1/chat/completions` (OpenAI-compatible).

### 18.7 LiveKit + coturn deployment

- **LiveKit Cloud** for V2 primary; no self-hosted for now.
- **Coturn** self-hosted in each region (since LiveKit Cloud's TURN is in US only).
- Coturn deployed as StatefulSet in K8s with HostPort for UDP/3478.
- Auto-scales to 3 pods per region; sticky sessions not needed (coturn is stateless).

### 18.8 Cost & FinOps

- **Per-tenant cost tag** on every resource: `TenantId`, `PlanId`, `Region`.
- **Daily cost report** from AWS Cost Explorer to S3; loaded into the admin dashboard.
- **Anomaly alerts:** cost increase >20% day-over-day → Slack to FinOps channel.
- **Right-sizing:** weekly Trusted Advisor review; recommend downsizing underutilized instances.
- **Spot instances** for non-critical workloads (AI batch, ETL, log shipping).
- **Savings plans** for predictable workloads (1-year, no upfront).

### 18.9 V2 observability stack

- **Metrics:** Prometheus + Grafana (self-hosted in each region) OR Datadog (managed).
- **Logs:** Promtail → Loki (self-hosted) OR Datadog Logs.
- **Traces:** OpenTelemetry Collector → Tempo / Jaeger (self-hosted) OR Datadog APM.
- **Errors:** Sentry.
- **Synthetics:** Blackbox exporter (Prometheus) or Datadog Synthetics.
- **Dashboards:** Grafana JSON committed to `dashboards/` repo, deployed via GitOps.
- **Alerts:** Alertmanager → PagerDuty.

### 18.10 V2 CI/CD pipeline

```yaml
# .github/workflows/v2-deploy.yaml
name: V2 Deploy
on:
  push:
    branches: [main]
    paths: ['medicare-api/**', 'medicare-web/**', 'medicare-user-app/**', 'medicare-doctor-app/**']

jobs:
  test:
    uses: ./.github/workflows/test-reusable.yaml

  build:
    needs: test
    strategy:
      matrix: { service: [api, web, user-app, doctor-app, ai-service, lab-gateway, fhir-gateway] }
    steps:
      - uses: actions/checkout@v4
      - run: docker buildx build --push -t $ECR/medicare-${{ matrix.service }}:${{ github.sha }} --cache-from $ECR/medicare-${{ matrix.service }}:cache ./medicare-${{ matrix.service }}
      - run: cosign sign --key ${{ secrets.SIGSTORE_KEY }} $ECR/medicare-${{ matrix.service }}:${{ github.sha }}

  deploy-staging:
    needs: build
    environment: staging
    steps:
      - run: argocd app sync medicare-api-staging
      - run: argocd app sync medicare-web-staging
      - run: ./scripts/smoke-test.sh staging
      - run: ./scripts/canary-deploy.sh ai-service staging 10%

  deploy-prod-canary:
    needs: deploy-staging
    if: github.ref == 'refs/heads/main'
    environment: production
    steps:
      - run: ./scripts/canary-deploy.sh api prod 5%
      - run: sleep 1800  # 30 min watch
      - run: ./scripts/check-slo.sh api
      - run: ./scripts/canary-promote.sh api 50%
      - run: sleep 1800
      - run: ./scripts/canary-promote.sh api 100%
```

### 18.11 V2 pre-deploy checklist

- [ ] All tests pass (unit, feature, integration, E2E).
- [ ] SAST, SCA, secrets scan, license check pass.
- [ ] SBOM generated and signed.
- [ ] Container image signed (cosign) and pushed to ECR.
- [ ] DB migrations are backward-compatible (no DROP, no RENAME; only ADD).
- [ ] Feature flags updated; rollout plan reviewed.
- [ ] Runbook updated for any new behavior.
- [ ] On-call notified; SLO dashboards watched.
- [ ] Rollback plan documented and tested.
- [ ] Post-deploy verification (smoke tests) automated.

### 18.12 V2 acceptance criteria

- [ ] GitOps deploys a service in <10 min from merge.
- [ ] Canary deploys to 5% in <2 min, auto-promotes if SLOs hold.
- [ ] Rollback restores previous version in <3 min.
- [ ] DR drill completes in <1h with RPO verified <5min.
- [ ] All sidecar services have mTLS via SPIFFE.
- [ ] Per-tenant cost tag on every billable resource.
- [ ] Multi-region failover (planned) completes in <15 min.
- [ ] Multi-region failover (unplanned) completes in <30 min.
