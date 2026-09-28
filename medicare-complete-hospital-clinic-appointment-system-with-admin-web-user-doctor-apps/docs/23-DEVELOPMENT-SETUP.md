# 23 — Development Setup (Local Dev, Docker, Tooling, Conventions)

> From `git clone` to "I made my first API call" in 15 minutes. Repeatable for every dev, on macOS, Linux, and Windows (WSL2).

---

## 1. Prerequisites

| Tool | Version | Why |
|---|---|---|
| **PHP** | 8.3+ | Laravel 11 requires it |
| **Composer** | 2.7+ | PHP deps |
| **Node.js** | 20 LTS | Web (Next.js) |
| **pnpm** | 9+ | Faster than npm |
| **MySQL** | 8.0+ | Primary DB |
| **Redis** | 7+ | Cache + queue + Reverb |
| **Docker + Docker Compose** | latest | For the "no local install" path |
| **Git** | 2.40+ | |
| **Flutter** | 3.24+ | Mobile (separate repos) |
| **Android Studio / Xcode** | latest | For mobile dev |

Recommended but optional:
- **TablePlus** or **DBeaver** — DB GUI
- **Postman** or **Insomnia** — API testing
- **iTerm2** / **Windows Terminal** — better terminal
- **Oh My Zsh** / **Starship** — shell niceties

---

## 2. Quick Start (Docker, 5 min)

If you don't want to install PHP/MySQL/Redis locally:

```bash
git clone https://github.com/medicare/medicare-api.git
cd medicare-api
cp .env.example .env
docker compose up -d
docker compose exec app composer install
docker compose exec app php artisan key:generate
docker compose exec app php artisan install:resume \
    --db-host=db \
    --db-name=medicare \
    --db-user=medicare \
    --db-pass=medicare \
    --admin-email=admin@medicare.test \
    --admin-password=password \
    --tenancy=single \
    --app-url=http://localhost:8000
```

Then open `http://localhost:8000` → log in as `admin@medicare.test` / `password`.

### 2.1 The `docker-compose.yml`

```yaml
version: '3.9'

services:
  app:
    build: .
    volumes:
      - .:/var/www/html
    ports:
      - "8000:8000"
    depends_on:
      - db
      - redis
    environment:
      DB_HOST: db
      DB_DATABASE: medicare
      DB_USERNAME: medicare
      DB_PASSWORD: medicare
      REDIS_HOST: redis
      APP_URL: http://localhost:8000
    command: >
      sh -c "composer install &&
             php artisan migrate &&
             php artisan serve --host=0.0.0.0 --port=8000"

  db:
    image: mysql:8
    environment:
      MYSQL_ROOT_PASSWORD: root
      MYSQL_DATABASE: medicare
      MYSQL_USER: medicare
      MYSQL_PASSWORD: medicare
    ports:
      - "3306:3306"
    volumes:
      - db_data:/var/lib/mysql

  redis:
    image: redis:7
    ports:
      - "6379:6379"

  reverb:
    build: .
    command: php artisan reverb:start --host=0.0.0.0 --port=8080
    ports:
      - "8080:8080"
    depends_on:
      - redis

  mailhog:
    image: mailhog/mailhog
    ports:
      - "1025:1025"   # SMTP
      - "8025:8025"   # Web UI

  horizon:
    build: .
    command: php artisan horizon
    depends_on:
      - app
      - redis

volumes:
  db_data:
```

Open:
- API: `http://localhost:8000`
- WebSocket: `localhost:8080`
- MailHog (catch all emails): `http://localhost:8025`
- Horizon: `http://localhost:8000/horizon` (in local)

---

## 3. Quick Start (Native, 15 min)

### 3.1 Install PHP 8.3

**macOS** (Homebrew):
```bash
brew install php@8.3
brew link php@8.3 --force
```

**Ubuntu**:
```bash
sudo add-apt-repository ppa:ondrej/php
sudo apt update
sudo apt install php8.3 php8.3-{cli,mbstring,intl,bcmath,gd,zip,redis,mysql,xml,curl,fpm}
```

**Windows (WSL2)**: same as Ubuntu.

### 3.2 Install MySQL 8

```bash
# macOS
brew install mysql@8.0
brew services start mysql@8.0
mysql_secure_installation

# Ubuntu
sudo apt install mysql-server-8.0
sudo systemctl start mysql
sudo mysql_secure_installation
```

### 3.3 Install Redis 7

```bash
brew install redis       # macOS
sudo apt install redis   # Ubuntu
brew services start redis
```

### 3.4 Install Composer

```bash
curl -sS https://getcomposer.org/installer | php
sudo mv composer.phar /usr/local/bin/composer
```

### 3.5 Clone & Install

```bash
git clone https://github.com/medicare/medicare-api.git
cd medicare-api
cp .env.example .env
composer install
php artisan key:generate
```

### 3.6 Configure .env

```dotenv
APP_NAME=Medicare
APP_ENV=local
APP_DEBUG=true
APP_URL=http://localhost:8000

DB_CONNECTION=mysql
DB_HOST=127.0.0.1
DB_PORT=3306
DB_DATABASE=medicare
DB_USERNAME=root
DB_PASSWORD=

REDIS_HOST=127.0.0.1
REDIS_PORT=6379
REDIS_PASSWORD=null

CACHE_STORE=redis
QUEUE_CONNECTION=redis
SESSION_DRIVER=redis

BROADCAST_CONNECTION=reverb
REVERB_APP_ID=medicare
REVERB_APP_KEY=medicare-key
REVERB_APP_SECRET=medicare-secret
REVERB_HOST=0.0.0.0
REVERB_PORT=8080

MAIL_MAILER=smtp
MAIL_HOST=127.0.0.1
MAIL_PORT=1025          # MailHog
MAIL_FROM_ADDRESS=no-reply@medicare.test
MAIL_FROM_NAME="Medicare Local"
```

### 3.7 Create DB & Migrate

```bash
mysql -u root -e "CREATE DATABASE medicare;"
php artisan migrate --seed
php artisan install:resume ...   # or skip and use --seed instead
```

### 3.8 Start Services

You need 3 terminals:

**Terminal 1 — API:**
```bash
php artisan serve
```

**Terminal 2 — Reverb:**
```bash
php artisan reverb:start
```

**Terminal 3 — Queue worker:**
```bash
php artisan queue:work
```

Or use `php artisan dev` (Laravel 11's built-in dev command) which uses Concurrently to run all 3:
```bash
composer require laravel/dev
php artisan dev
```

This starts: server, queue, reverb, vite (for any front-end assets), logs all in one window.

### 3.9 Verify

```bash
curl http://localhost:8000/health
# → 200 OK

# Open admin (in the web repo):
cd ../medicare-web
pnpm install
pnpm dev
# → http://localhost:3000 (admin) or :3001 (patient-web)
```

---

## 4. The 4 Repos Setup

Each repo has its own README with specifics. Here's the overview.

### 4.1 `medicare-api` (Laravel)

```bash
git clone https://github.com/medicare/medicare-api.git
cd medicare-api
composer install
cp .env.example .env
php artisan key:generate
php artisan migrate --seed
php artisan dev   # starts everything
```

### 4.2 `medicare-web` (Next.js)

```bash
git clone https://github.com/medicare/medicare-web.git
cd medicare-web
pnpm install
cp apps/admin/.env.example apps/admin/.env.local
cp apps/patient-web/.env.example apps/patient-web/.env.local
pnpm dev
```

The monorepo has:
- `apps/admin` — clinic back-office
- `apps/patient-web` — public-facing site
- `packages/ui` — shared components
- `packages/api-client` — typed API client
- `packages/config` — tsconfig, eslint, etc.

### 4.3 `medicare-user-app` (Flutter)

```bash
git clone https://github.com/medicare/medicare-user-app.git
cd medicare-user-app
flutter pub get
cp .env.example .env
flutter run            # on connected device/emulator
```

### 4.4 `medicare-doctor-app` (Flutter)

Same as user-app but different repo.

---

## 5. Code Style

### 5.1 PHP

- **PSR-12** + Laravel conventions.
- `laravel/pint` for formatting (auto-fix on save).
- PHPStan level 8 for static analysis.
- EditorConfig for whitespace.

Run before commit:
```bash
./vendor/bin/pint
./vendor/bin/phpstan analyse
```

### 5.2 TypeScript / React

- **TypeScript strict** mode.
- ESLint + Prettier.
- Auto-format on save (in editor).
- Run before commit:
  ```bash
  pnpm turbo run lint
  pnpm turbo run typecheck
  ```

### 5.3 Dart / Flutter

- `flutter_lints` (Flutter team's recommended rules).
- `dart format` on save.
- Run before commit:
  ```bash
  dart format .
  flutter analyze
  ```

---

## 6. Git Workflow

### 6.1 Branches

- `main` — always deployable, protected.
- `feat/xxx`, `fix/xxx`, `chore/xxx` — feature branches.
- `release/v1.2.3` — release branches (cut from main, hotfixes only).
- Personal: `yourname/xxx`.

### 6.2 Commit Messages

Conventional Commits:

```
feat(appointments): add bulk cancel
fix(payments): handle Stripe webhook signature
chore(deps): bump Laravel to 11.5
docs(auth): document OTP rate limits
test(appointments): add state machine coverage
refactor(billing): extract tax calculator
```

### 6.3 PR Workflow

1. Branch from `main`.
2. Commit in small logical chunks.
3. Push, open PR.
4. CI runs (lint, typecheck, tests, security).
5. At least 1 approval (2 for `medicare-api` or schema changes).
6. Squash merge to `main`.
7. Auto-deploy to staging.
8. Manual promotion to production via Forge.

### 6.4 Branch Protection

- `main` requires:
  - 1+ approval
  - All CI checks pass
  - No direct pushes
  - Linear history (squash only)

---

## 7. Database Workflow

### 7.1 New Migration

```bash
php artisan make:migration add_xyz_to_appointments_table --table=appointments
```

Edit the file. Add an index if a new FK. Write the `down()` method.

### 7.2 New Seeder

```bash
php artisan make:seeder MedicineSeeder
```

### 7.3 Reset Local DB

```bash
php artisan migrate:fresh --seed
```

This drops all tables and re-runs everything. **Destructive** — never run in staging/prod.

### 7.4 Tinker for One-Offs

```bash
php artisan tinker
> User::find(1)->update(['email_verified_at' => now()]);
> Appointment::where('status', 'pending')->count();
```

---

## 8. Testing

### 8.1 Test Stack

- **PHPUnit** (or **Pest** — we use Pest for new tests, PHPUnit for legacy).
- **Mockery** for mocks.
- **Laravel Dusk** for browser E2E.
- **Playwright** for the web admin E2E.
- **BrowserStack** for real device testing of mobile.

### 8.2 Running Tests

```bash
# All tests
php artisan test

# Parallel
php artisan test --parallel

# Specific file
php artisan test --filter=AppointmentStateMachineTest

# Specific test
php artisan test --filter=test_cannot_transition_from_completed

# With coverage
php artisan test --coverage --min=80
```

### 8.3 Test Structure

```
tests/
├── Unit/                      # Pure unit tests
│   ├── Modules/
│   │   ├── Billing/
│   │   │   ├── MoneyTest.php
│   │   │   └── TaxCalculatorTest.php
│   │   └── ...
├── Feature/                   # HTTP + DB
│   ├── Modules/
│   │   ├── Auth/
│   │   ├── Appointments/
│   │   └── ...
├── Integration/               # Cross-module
│   ├── BookingFlowTest.php
│   ├── WebhookFlowTest.php
│   └── ...
├── Browser/                   # Dusk (deprecated; using Playwright)
└── E2E/                       # Full E2E (Playwright)
```

### 8.4 Factories

Every model has a factory. We use `laravel/pail` for cleaner factory syntax:

```php
// database/factories/PatientFactory.php
class PatientFactory extends Factory {
    public function definition(): array {
        return [
            'mrn' => 'MRN-' . now()->year . '-' . str_pad((string) $this->faker->unique()->numberBetween(1, 999999), 6, '0', STR_PAD_LEFT),
            'first_name' => $this->faker->firstName(),
            'last_name' => $this->faker->lastName(),
            'gender' => $this->faker->randomElement(['m', 'f']),
            'dob' => $this->faker->dateTimeBetween('-80 years', '-1 year')->format('Y-m-d'),
            'tenant_id' => Tenant::factory(),
        ];
    }
    
    public function minor(): self {
        return $this->state(['dob' => $this->faker->dateTimeBetween('-17 years', '-1 year')->format('Y-m-d')]);
    }
}
```

### 8.5 Mocking External Services

- **Firebase**: `FirebaseVerifier::shouldReceive('verifyIdToken')->andReturn(...)`.
- **Razorpay**: `Http::fake([...]);` to intercept HTTP calls.
- **Reverb**: `Event::fake([...])` to skip broadcasting in tests.
- **Queue**: `Bus::fake(); Queue::fake();` to assert jobs dispatched without running them.

---

## 9. Debugging

### 9.1 Telescope (dev only)

`laravel/telescope` is installed in `local` env. Access at `/telescope`.

- Requests
- Commands
- Scheduled tasks
- Jobs
- Events
- Gates
- Cache
- Redis
- Queries (with EXPLAIN!)

### 9.2 Ray (paid, recommended)

`spatie/ray` for beautiful debugging. `$ray->show($variable)`.

### 9.3 Logs

```bash
tail -f storage/logs/laravel.log
```

Or use the integrated log viewer in Telescope.

### 9.4 DB Query Log

```php
DB::enableQueryLog();
// ... do stuff
dd(DB::getQueryLog());
```

### 9.5 Dump Server

```bash
php artisan dump-server
```

Then `dump($variable)` in code → output appears in terminal instead of breaking the page.

---

## 10. IDE Setup

### 10.1 VS Code (Recommended)

Extensions:
- PHP Intelephense
- Laravel Extension Pack
- Tailwind CSS IntelliSense
- ESLint, Prettier
- Dart, Flutter
- GitLens
- Thunder Client (API testing)
- Error Lens

Settings (`.vscode/settings.json`):
```json
{
  "editor.formatOnSave": true,
  "php-cs-fixer.onsave": true,
  "[php]": { "editor.defaultFormatter": "open-southeners.laravel-pint" },
  "[typescript]": { "editor.defaultFormatter": "esbenp.prettier-vscode" },
  "[dart]": { "editor.defaultFormatter": "Dart-Code.dart-code" }
}
```

### 10.2 PhpStorm

- Install Laravel plugin
- Configure PHP interpreter (Docker or local)
- Enable Laravel Idea (paid but worth it)

### 10.3 Android Studio / Xcode

For mobile only.

---

## 11. Useful Commands

```bash
# Generate IDE helper files (Laravel IDE Helper)
php artisan ide-helper:generate
php artisan ide-helper:models --nowrite

# Check N+1 queries
php artisan db:show --counts
php artisan db:table appointments

# Optimize autoloader
composer dump-autoload -o

# Clear all caches
php artisan optimize:clear

# List all routes
php artisan route:list

# List all scheduled tasks
php artisan schedule:list

# Run a queued job synchronously
php artisan tinker
> dispatch(new \App\Modules\Appointments\Jobs\MarkNoShowJob())->onQueue('sync');
```

### 11.1 The `dev` Command

`php artisan dev` (Laravel 11+) runs:
- `php artisan serve`
- `php artisan queue:work`
- `php artisan reverb:start`
- `php artisan pail --watch` (log tailer)
- `php artisan vite` (for any inline assets)

All in one terminal with color-coded output.

---

## 12. Troubleshooting

### 12.1 "Class not found"

```bash
composer dump-autoload
php artisan optimize:clear
```

### 12.2 "Permission denied" on storage

```bash
chmod -R 775 storage bootstrap/cache
chown -R $USER:www-data storage bootstrap/cache
```

### 12.3 "SQLSTATE[HY000] [2002] No connection"

DB not running. Start it:
```bash
brew services start mysql
sudo systemctl start mysql
```

### 12.4 "Predis vs phpredis"

Laravel supports both. For perf use phpredis extension. For dev convenience use Predis (pure PHP, no extension needed).

```dotenv
REDIS_CLIENT=phpredis   # or predis
```

### 12.5 "Reverb won't start"

Check port 8080 is free:
```bash
lsof -i :8080
```

If used, change `REVERB_PORT` in `.env`.

### 12.6 "Mobile app can't connect to API"

- CORS: add `http://localhost:3000` to `config/cors.php` allowed origins.
- Network: if using a physical device, use your machine's LAN IP, not `localhost`.
- TLS: if API is HTTPS with self-signed cert, add cert to device.

### 12.7 "Tenant scope is too aggressive in tests"

Tests run without a tenant by default. Wrap them in `Tenant::set(Tenant::factory()->create())` or use the `WithTenant` trait.

```php
use App\Modules\Tenancy\Testing\WithTenant;

class AppointmentTest extends TestCase {
    use WithTenant;
    
    protected string $seedTenant = true;
}
```

---

## 13. Onboarding a New Dev (Day 1 Checklist)

```markdown
## Day 1

- [ ] GitHub access (added to org)
- [ ] VPN access (if needed)
- [ ] Clone all 4 repos
- [ ] Run docker compose up -d
- [ ] Run installer
- [ ] Log in to admin
- [ ] Book a test appointment
- [ ] Open Flutter app, see test appointment
- [ ] Read 00-ARCHITECTURE.md
- [ ] Read your module's doc
- [ ] Pair with a senior on a small PR
- [ ] Deploy to staging together

## Week 1

- [ ] Read 20-SECURITY-COMPLIANCE.md
- [ ] Read 17-REALTIME.md
- [ ] Read 19-INTEGRATIONS.md
- [ ] First solo PR merged
- [ ] Attend architecture review
- [ ] Set up on-call rotation
```

---

## 14. Acceptance Criteria (V1)

- [ ] New dev can clone + run + log in within 15 min.
- [ ] Docker compose path requires zero local installs.
- [ ] All 4 repos have working local dev setups.
- [ ] All tests pass on a fresh clone.
- [ ] CI catches style + type errors before review.
- [ ] Lint, type check, and tests run on every PR.
- [ ] Coverage threshold enforced (80% for new code).
- [ ] No secrets in any committed file (gitleaks in CI).
- [ ] Documentation matches the actual code (linted docstrings).

---

## 15. V2 Development Setup

> V1 dev setup is 4 repos (api, web, user-app, doctor-app). V2 adds 3 more (ai-service, lab-gateway, fhir-gateway) plus 7 external services to run locally (LiveKit, coturn, MinIO, Meilisearch, Ollama, mock lab partner, mock ABDM).

### 15.1 V2 dev stack additions

| Component | Tech | Why | Install |
|---|---|---|---|
| `medicare-ai-service` | Python 3.12, FastAPI, vLLM | LLM, embeddings, STT | Local venv + poetry |
| `medicare-lab-gateway` | Java 21, Spring Boot | Lab partner adapters | Docker compose |
| `medicare-fhir-gateway` | Node 22, HAPI-FHIR | FHIR endpoints | Docker compose |
| LiveKit | Go | Telemedicine video | Docker compose |
| coturn | C | TURN/STUN | Docker compose |
| MinIO | Go | S3-compatible storage | Docker compose |
| Meilisearch | Rust | Search | Docker compose |
| Ollama | Rust | Local LLM (qwen2.5:7b, llama3.1:8b) | Native install |
| Whisper | Python | STT | pip install |
| mock-lab-partner | Node | Mock 1mg/Quest webhook | Docker compose |
| mock-abdm | Node | Mock ABDM gateway | Docker compose |
| mock-payer | Node | Mock insurance payer | Docker compose |
| Redis (V1, V2) | C | Cache, queue, pub/sub | Docker compose |
| PostgreSQL (V2) | C | Optional migration from MySQL for pgvector | Docker compose |
| MailHog | Go | Local SMTP | Docker compose |
| Stripe CLI | Go | Webhook forwarding | Native install |
| Razorpay CLI | Go | Webhook forwarding | Native install |

### 15.2 V2 docker-compose.yaml (additions)

```yaml
# docker-compose.v2.yaml
services:
  # V1 services
  mysql: ...
  redis: ...
  mailhog: ...
  reverb: ...
  stripe-cli: ...

  # V2 additions
  ai-service:
    build: ./medicare-ai-service
    ports: ['8000:8000']
    volumes: ['./medicare-ai-service:/app']
    environment:
      OPENAI_API_KEY: ${OPENAI_API_KEY:-sk-fake}
      ANTHROPIC_API_KEY: ${ANTHROPIC_API_KEY:-sk-ant-fake}
      DEFAULT_LLM: ollama  # uses local Ollama by default
    depends_on: [ollama]

  ollama:
    image: ollama/ollama:latest
    ports: ['11434:11434']
    volumes: ['ollama-data:/root/.ollama']
    # First run: ollama pull qwen2.5:7b llama3.1:8b whisper-large-v3

  lab-gateway:
    build: ./medicare-lab-gateway
    ports: ['8080:8080']
    environment:
      SPRING_PROFILES_ACTIVE: dev

  fhir-gateway:
    build: ./medicare-fhir-gateway
    ports: ['8081:8080']
    depends_on: [postgres]

  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: medicare_pg
      POSTGRES_USER: medicare
      POSTGRES_PASSWORD: medicare
    ports: ['5432:5432']
    volumes: ['postgres-data:/var/lib/postgresql/data']

  livekit:
    image: livekit/livekit-server:latest
    ports: ['7880:7880', '7881:7881/udp', '7882:7882/udp']
    command: --dev --bind 0.0.0.0

  coturn:
    image: coturn/coturn:latest
    ports: ['3478:3478/udp', '3478:3478/tcp']
    command: >
      -n
      --realm=turn.local
      --static-auth-secret=dev-secret
      --listening-port=3478
      --no-tls
      --no-dtls

  minio:
    image: minio/minio:latest
    ports: ['9000:9000', '9001:9001']
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin
    command: server /data --console-address ":9001"

  meilisearch:
    image: getmeili/meilisearch:v1.10
    ports: ['7700:7700']
    environment:
      MEILI_MASTER_KEY: medicare-dev-key

  mock-lab-partner:
    build: ./mock-servers/lab-partner
    ports: ['9001:3000']
    environment:
      WEBHOOK_TARGET: http://nginx:80/webhooks/lab-partner

  mock-abdm:
    build: ./mock-servers/abdm
    ports: ['9002:3000']

  mock-payer:
    build: ./mock-servers/payer
    ports: ['9003:3000']

volumes:
  postgres-data:
  ollama-data:
```

### 15.3 V2 dev scripts

```bash
# bin/dev (V2)
#!/usr/bin/env bash
set -euo pipefail

# Detect platform
case "$(uname -s)" in
  Linux*)  PLATFORM=linux ;;
  Darwin*) PLATFORM=macos ;;
  *)       PLATFORM=windows ;;
esac

# Start core services
docker compose -f docker-compose.yaml -f docker-compose.v2.yaml up -d

# Wait for services
./scripts/wait-for.sh mysql:3306
./scripts/wait-for.sh redis:6379
./scripts/wait-for.sh ai-service:8000
./scripts/wait-for.sh livekit:7880

# Run migrations
php artisan migrate --seed

# Generate API docs
php artisan l5-swagger:generate

# Start API server
php artisan serve --host=0.0.0.0 --port=8000 &

# Start queue worker
php artisan queue:work --queue=default,broadcasts,exports &

# Start Reverb
php artisan reverb:start --port=8080 &

# Start scheduler (in dev, run as a loop)
while true; do php artisan schedule:run; sleep 60; done &

# Start AI service
cd medicare-ai-service && poetry run uvicorn main:app --reload --port 8000 &

# Start Next.js
cd medicare-web && pnpm dev &

# Tails
tail -f storage/logs/laravel.log
```

### 15.4 V2 onboarding (extended)

V1 onboarding is 3 days. V2 adds 2 more days:

#### Day 1 (V1, unchanged)
- Setup, run, log in.

#### Day 2 (V1, unchanged)
- First PR, modules overview.

#### Day 3 (V1, unchanged)
- Architecture, security.

#### Day 4 (V2 NEW)
- Read `14-MODULE-V2-STUBS.md`.
- Read `00-ARCHITECTURE.md` §14 (V2 architecture).
- Run V2 services locally: `docker compose -f docker-compose.v2.yaml up`.
- Send a test LLM call: `curl -X POST http://localhost:8000/v1/chat/completions -d '{"messages": [{"role": "user", "content": "Hello"}]}'`.
- Spin up LiveKit: `docker compose up livekit`; join a test room from two browser tabs.
- Mock a lab partner webhook: `curl -X POST http://localhost:9001/webhook -d '{"order_id": "test", "status": "completed"}'`.

#### Day 5 (V2 NEW)
- Read `19-INTEGRATIONS.md` §15 (V2 integrations).
- Make a Stripe CLI webhook: `stripe listen --forward-to localhost:8000/webhooks/stripe`.
- Set up MailHog, see emails at `http://localhost:8025`.
- Connect a debugger to the Python AI service (VS Code `launch.json` preset).
- First V2 PR: enable a V2 feature flag for a test tenant, verify the migration runs.

### 15.5 V2 dev tools

| Tool | Purpose | Install |
|---|---|---|
| `pgcli` | PostgreSQL CLI with autocomplete | `brew install pgcli` |
| `redis-cli` | Redis CLI | `apt install redis-tools` |
| `k9s` | K8s dashboard for local k3d | `brew install k9s` |
| `argocd-cli` | ArgoCD CLI for local GitOps | `brew install argocd` |
| `stripe-cli` | Stripe webhook forwarding | `brew install stripe/stripe-cli/stripe` |
| `mailhog` | Local SMTP catcher | Docker |
| `ngrok` | Expose localhost for webhook testing | `brew install ngrok` |
| `direnv` | Per-directory env vars | `brew install direnv` |
| `aws-vault` | AWS credential management | `brew install aws-vault` |
| `kafkacat` | Inspect Kafka events (V3) | `brew install kafkacat` |
| `lens` | K8s IDE | Native install |

### 15.6 V2 .env additions

```bash
# V1 (unchanged)
APP_NAME=Medicare
APP_ENV=local
APP_KEY=base64:...
APP_URL=http://localhost:8000
MEDICARE_TENANCY=false
# ... (all V1 vars)

# V2 additions
# AI
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
GOOGLE_AI_KEY=...
DEFAULT_LLM_PROVIDER=ollama
OLLAMA_HOST=http://localhost:11434
MEDICARE_AI_SERVICE_URL=http://localhost:8000
MEDICARE_AI_SERVICE_TOKEN=dev-shared-secret

# Telemedicine
LIVEKIT_API_KEY=APIdevkey
LIVEKIT_API_SECRET=devsecret
MEDICARE_LIVEKIT_URL=ws://localhost:7880
MEDICARE_COTURN_URL=turn:localhost:3478
MEDICARE_COTURN_SECRET=dev-secret

# Storage (multi-backend)
MEDICARE_STORAGE_DRIVER=s3
S3_ENDPOINT=http://localhost:9000
S3_KEY=minioadmin
S3_SECRET=minioadmin
S3_BUCKET=medicare-dev
S3_REGION=us-east-1
S3_USE_PATH_STYLE=true

# Search
MEDICARE_SEARCH_DRIVER=meilisearch
MEILISEARCH_HOST=http://localhost:7700
MEILISEARCH_KEY=medicare-dev-key

# V2 plugins (V2 enabled, V1 disabled)
FEATURE_RECURRING_APPOINTMENTS=true
FEATURE_GROUP_APPOINTMENTS=true
FEATURE_FAMILY_BOOKING=true
FEATURE_WHATSAPP=false
FEATURE_TELEMEDICINE=true
FEATURE_AI_SYMPTOM_CHECKER=true
FEATURE_PATHOLOGY_LAB=true
FEATURE_DRUG_INTERACTION_DB=true
FEATURE_IN_APP_CHAT=true
FEATURE_INSURANCE_PREAUTH=false
FEATURE_CUSTOM_DOMAIN=false

# Mock services
MOCK_LAB_PARTNER_URL=http://localhost:9001
MOCK_ABDM_URL=http://localhost:9002
MOCK_PAYER_URL=http://localhost:9003
```

### 15.7 V2 test data seeder

`database/seeders/DevSeeder.php` creates:
- 1 tenant (default).
- 1 super-admin (`admin@medicare.test` / `password`).
- 3 cities, 5 clinics, 8 doctors, 5 specializations, 10 services.
- 50 patients with vitals and history.
- 100 appointments (past, today, future).
- 30 prescriptions, 20 invoices, 15 transactions.
- V2: 5 lab orders, 3 insurance claims, 2 video rooms, 1 subscription (trialing).
- 2 plugins installed (Drug Interaction DB, In-App Chat).

### 15.8 V2 CI additions

```yaml
# .github/workflows/ci-v2.yaml
jobs:
  test-ai-service:
    runs-on: ubuntu-latest
    services:
      ollama:
        image: ollama/ollama
        ports: ['11434:11434']
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: actions/setup-python@v5
        with: { python-version: '3.12' }
      - run: pip install poetry && poetry install
      - run: poetry run pytest --cov
        env:
          DEFAULT_LLM_PROVIDER: ollama

  test-lab-gateway:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: actions/setup-java@v4
        with: { distribution: temurin, java-version: '21' }
      - run: ./gradlew test

  integration-v2:
    runs-on: ubuntu-latest
    services:
      mysql: ...
      redis: ...
      livekit: ...
      ollama: ...
    steps:
      - uses: actions/checkout@v4
      - run: docker compose -f docker-compose.yaml -f docker-compose.v2.yaml up -d
      - run: ./scripts/wait-for-all.sh
      - run: php artisan test --testsuite=Feature --filter=V2

  e2e-v2:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: pnpm install
      - run: pnpm playwright test --project=v2

  security-v2:
    runs-on: ubuntu-latest
    steps:
      - run: pip install bandit
      - run: bandit -r medicare-ai-service
      - run: ./gradlew dependencyCheckAnalyze
      - run: trivy fs --security-checks vuln,secret,misconfig .

  cost-check:
    runs-on: ubuntu-latest
    steps:
      - run: infracost breakdown --path=terraform
```

### 15.9 V2 acceptance criteria

- [ ] New V2 dev can clone, run all V2 services, log in within 30 min.
- [ ] Docker compose v2 starts all 15 services in <5 min.
- [ ] Ollama model pull is pre-cached in CI.
- [ ] Dev seeder produces a realistic dataset.
- [ ] V2 tests run in CI in <10 min total.
- [ ] Coverage threshold maintained at 80% for new V2 code.
- [ ] No secrets in any committed file (gitleaks + trufflehog).
- [ ] V1 acceptance criteria still pass.

---

## 16. Test Matrix by Module

> Per §19.16 acceptance criteria ("each flow has a test (Pest feature test)"), this is the **module-to-test-file map** for all 59 V1 + V1.1 + V1.2 + V2 + V2.1 + V3 + V4 + V5+ modules. The test files live in `app-modules/{slug}/tests/Feature/{Foo}Test.php`; integration tests live in `tests/Integration/{Foo}Test.php`; E2E Playwright tests live in `tests/E2E/{slug}.spec.ts`; performance k6 scripts live in `tests/Performance/{slug}.k6.js`. Every module has at least one Feature test, one Integration test (where applicable), and one Playwright E2E (where applicable).

### 16.1 V1 modules (57)

| Module | Feature test | Integration test | E2E | Performance | Notes |
|---|---|---|---|---|---|
| Auth | `tests/Feature/AuthTest.php` | `tests/Integration/AuthTest.php` | `tests/E2E/auth.spec.ts` | — | Firebase OTP + email/password |
| RBAC | `tests/Feature/RbacTest.php` | `tests/Integration/TenantIsolationTest.php` | — | — | Spatie Permissions |
| Tenancy | `tests/Feature/TenancyTest.php` | `tests/Integration/TenantIsolationTest.php` | — | — | Row-level isolation |
| Cities / Clinics / Departments / Specs / Services | `tests/Feature/ClinicStructureTest.php` | `tests/Integration/ClinicStructureTest.php` | — | — | |
| Doctors + Schedules + Slots | `tests/Feature/DoctorTest.php` | `tests/Integration/DoctorTest.php` | `tests/E2E/doctor-profile.spec.ts` | `tests/Performance/slot-gen.k6.js` | SlotGenerator: 60 days < 1s |
| Patients + Family + Vitals + Files | `tests/Feature/PatientTest.php` | `tests/Integration/PatientTest.php` | `tests/E2E/patient-booking.spec.ts` | — | Auto-MRN |
| Appointments + Calendar + Check-in + Queue | `tests/Feature/AppointmentTest.php` (250+ cases) | `tests/Integration/AppointmentTest.php` (100+ cases) | `tests/E2E/appointment-booking.spec.ts` | `tests/Performance/appointment-concurrency.k6.js` | State machine: 9 states, 30 transitions |
| Prescriptions + Medicines | `tests/Feature/PrescriptionTest.php` | `tests/Integration/PrescriptionTest.php` | — | — | |
| Invoices + Transactions + Refunds | `tests/Feature/InvoiceTest.php` | `tests/Integration/InvoiceTest.php` | `tests/E2E/payment.spec.ts` | — | |
| Payments (Razorpay + Stripe + Paystack) | `tests/Feature/PaymentTest.php` | `tests/Integration/PaymentWebhookTest.php` | `tests/E2E/payment.spec.ts` | `tests/Performance/payment-webhook.k6.js` | Mocked gateway webhooks |
| Coupons + Taxes | `tests/Feature/DiscountTest.php` | — | — | — | |
| Notifications (FCM + Email + Templates) | `tests/Feature/NotificationTest.php` | `tests/Integration/NotificationTest.php` | `tests/E2E/notification.spec.ts` | — | MailHog + FCM mock |
| Banners + Slides + Reviews + Currencies + Languages | `tests/Feature/EngagementTest.php` | — | — | — | |
| Settings + Reports + Dashboard | `tests/Feature/SettingsTest.php` | — | — | — | |
| Cron jobs | `tests/Feature/ScheduleTest.php` | — | — | — | Reminders, no-show |
| Reverb broadcasting | `tests/Feature/ReverbTest.php` | `tests/Integration/ReverbTest.php` | `tests/E2E/realtime.spec.ts` | `tests/Performance/reverb-cluster.k6.js` | 10k connections p99 < 100ms |

### 16.2 V1.1 lane (12 modules)

| Module | Feature test | Integration test | E2E | Performance | Runbook |
|---|---|---|---|---|---|
| `whatsapp-business` | `app-modules/whatsapp-business/tests/Feature/{SendTemplate,Inbound,24hWindow,QualityRating}Test.php` | `tests/Integration/WhatsappWebhookTest.php` | `tests/E2E/whatsapp.spec.ts` | — | `runbooks/whatsapp-business.md` |
| `recurring-appointments` | `app-modules/recurring-appointments/tests/Feature/{RruleParse,Expander,Exdate,Rdate,Dst,Idempotency}Test.php` | `tests/Integration/RecurringAppointmentTest.php` | `tests/E2E/recurring.spec.ts` | `tests/Performance/recurring-expander.k6.js` | `runbooks/recurring-appointments.md` |
| `group-appointments` | `app-modules/group-appointments/tests/Feature/{Capacity,RowLock,Outcomes,GroupVideo}Test.php` | `tests/Integration/GroupAppointmentTest.php` | `tests/E2E/group-booking.spec.ts` | — | `runbooks/group-appointments.md` |
| `family-booking` | `app-modules/family-booking/tests/Feature/{Dependent,Otp,Consent,MinorAging}Test.php` | `tests/Integration/FamilyTest.php` | `tests/E2E/family-booking.spec.ts` | — | `runbooks/family-booking.md` |
| `multi-currency-payout` | `app-modules/multi-currency-payout/tests/Feature/{Currency,Fx,Gateway,TaxWithholding}Test.php` | `tests/Integration/PayoutRunTest.php` | — | — | `runbooks/multi-currency-payout.md` |
| `geo-search` | `app-modules/geo-search/tests/Feature/{Postgis,Distance,TilePrecompute,Fallback}Test.php` | `tests/Integration/GeoSearchTest.php` | `tests/E2E/geo-search.spec.ts` | `tests/Performance/geo-search.k6.js` | `runbooks/geo-search.md` |
| `marketing-vouchers` | `app-modules/marketing-vouchers/tests/Feature/{Create,Redeem,Referral,Partner,Influencer,Scope,Limit,Fraud}Test.php` | `tests/Integration/VoucherTest.php` | — | `tests/Performance/voucher-redeem.k6.js` | `runbooks/marketing-vouchers.md` |
| `dynamic-refund-window` | `app-modules/dynamic-refund-window/tests/Feature/{CreatePolicy,Priority,SlidingScale,InitiatorRules,Credit,FreeRebook,Expiry,ForceMajeure}Test.php` | `tests/Integration/RefundWindowTest.php` | — | — | `runbooks/dynamic-refund-window.md` |
| `smart-waitlist` | `app-modules/smart-waitlist/tests/Feature/{Join,MatchScore,Offer,Claim,Decline,Cascade,QuietHours,Group,Recurring,Distance,HardConstraint}Test.php` | `tests/Integration/WaitlistCascadeTest.php` | — | `tests/Performance/waitlist-cascade.k6.js` | `runbooks/smart-waitlist.md` |
| `i18n-rtl` | `app-modules/i18n-rtl/tests/Feature/{LocaleSwitch,RtlLayout,Formatter,HijriCalendar,TranslationMemory,Glossary,AutoTranslate,VisualRegression}Test.php` | `tests/Integration/I18nTest.php` | `tests/Visual/rtl-snapshots/` (Playwright snapshots) | — | `runbooks/i18n-rtl.md` |
| `doctor-rotating-schedule` | `app-modules/doctor-rotating-schedule/tests/Feature/{CreateGroup,RegeneratePlan,Swap,Escalation,Fairness,CoverageGap,AfterHours,Timezone}Test.php` | `tests/Integration/RotationTest.php` | — | — | `runbooks/doctor-rotating-schedule.md` |
| `pwa-offline-mode` | `app-modules/pwa-offline/tests/Feature/{InstallPrompt,ServiceWorker,OfflineView,OfflineCheckIn,ConflictResolution,Push,Storage,VersionUpdate,Timezone}Test.php` | `tests/Integration/PwaSyncTest.php` | `tests/E2E/pwa-offline.spec.ts` | — | `runbooks/pwa-offline.md` |

### 16.3 V1.2 lane (8 modules)

| Module | Feature test | E2E | Runbook |
|---|---|---|---|
| `abdm-bridge` | `app-modules/abdm-bridge/tests/Feature/{AbhaLink,Consent,HiuPull,HipPush,CertLevels,Otp}Test.php` | — | `runbooks/abdm-bridge.md` |
| `fhir-r4-read` | `app-modules/fhir-r4-read/tests/Feature/{Patient,Practitioner,Encounter,Observation,MedicationRequest,Allergy,Condition,DiagnosticReport,Procedure,Immunization,Coverage,Capability,Bulk,Oauth}Test.php` | — | `runbooks/fhir-r4-read.md` |
| `razorpay-subscriptions` | `app-modules/razorpay-subscriptions/tests/Feature/{Card,UpiAutopay,eNach,PayLater,Webhook,Dunning,Pause,Proration}Test.php` | — | `runbooks/razorpay-subscriptions.md` |
| `paystack-subscriptions` | `app-modules/paystack-subscriptions/tests/Feature/{Card,Bank,MobileMoney,Webhook,Dunning,Region}Test.php` | — | `runbooks/paystack-subscriptions.md` |
| `stripe-terminal` | `app-modules/stripe-terminal/tests/Feature/{WisePad,Verifone,BBPOS,ServerDriven,Sdk,Offline,PartialCapture}Test.php` | — | `runbooks/stripe-terminal.md` |
| `twilio-sms` | `app-modules/twilio-sms/tests/Feature/{Verify,Messaging,Delivery,OptOut,OptIn,TwoWay,Regulatory,A2P10DLC,DltIndia}Test.php` | — | `runbooks/twilio-sms.md` |
| `local-payment-methods` | `app-modules/local-payment-methods/tests/Feature/{UpiIntent,UpiCollect,UpiAutopay,Paytm,NetBanking,Emi,Pix,Boleto,Oxxo,Gcash,MPesa,PaystackBank,PaystackQr}Test.php` | — | `runbooks/local-payment-methods.md` |
| `multi-clinic-dashboard` | `app-modules/multi-clinic-dashboard/tests/Feature/{ChainHierarchy,ChainAdmin,RegionAdmin,ChainAuditor,RevenueSplit,MaterializedView,ImpersonationBlock,PiiExport}Test.php` | — | `runbooks/multi-clinic-dashboard.md` |

### 16.4 V2 lane (11 modules)

| Module | Feature test | E2E | Runbook |
|---|---|---|---|
| `pathology-lab` | `app-modules/pathology-lab/tests/Feature/{Order,Partner,Loinc,SampleTracking,CriticalResult,Report}Test.php` | `tests/E2E/pathology.spec.ts` | `runbooks/pathology-lab.md` |
| `telemedicine-video` | `app-modules/telemedicine-video/tests/Feature/{Livekit,Daily,Jitsi,Recording,Consent,Quality}Test.php` | `tests/E2E/telemedicine.spec.ts` | `runbooks/telemedicine-video.md` |
| `ai-symptom-checker` | `app-modules/ai-symptom-checker/tests/Feature/{Session,RedFlag,Triage,Privacy}Test.php` | — | `runbooks/ai-symptom-checker.md` |
| `ai-clinical-scribe` | `app-modules/ai-clinical-scribe/tests/Feature/{Stt,Soap,Consent,Withdraw,SignOff}Test.php` | — | `runbooks/ai-clinical-scribe.md` |
| `in-app-chat` | `app-modules/in-app-chat/tests/Feature/{Message,Attachment,Voice,ChartPromotion,Encryption}Test.php` | — | `runbooks/in-app-chat.md` |
| `plugins-v1` | `app-modules/plugins-v1/tests/Feature/{Install,Lifecycle,Hook,Sandbox,Capability,Audit,KillSwitch,TenancyIsolation,Signature}Test.php` | — | `runbooks/plugins-v1.md` |
| `custom-domain` | `app-modules/custom-domain/tests/Feature/{Dns01,Caa,Acm,Hsts}Test.php` | — | `runbooks/custom-domain.md` |
| `multi-region-dr` | `app-modules/multi-region-dr/tests/Feature/{Failover,Rpo,Rto,Drill}Test.php` | — | `runbooks/multi-region-dr.md` |
| `drug-interaction-db` | `app-modules/drug-interaction/tests/Feature/{Rxnav,DrugBank,Who,TenantCurated,ContraindicatedHardStop,Performance}Test.php` | — | `runbooks/drug-interaction-db.md` |
| `e-prescription` | `app-modules/e-prescription/tests/Feature/{Abdm,Script,Eps,Nhs,Adha,SigningJws}Test.php` | — | `runbooks/e-prescription.md` |
| `insurance-preauth` | `app-modules/insurance-preauth/tests/Feature/{Eligibility270,Preauth278,Claim837,Era835,Nhcx,Denial,CarcRarc}Test.php` | — | `runbooks/insurance-preauth.md` |

### 16.5 V2.1 lane (6 modules)

| Module | Feature test | E2E | Runbook |
|---|---|---|---|
| `revenue-cycle-mgmt` | `app-modules/revenue-cycle-mgmt/tests/Feature/{Denial,Appeals,Dunning,PaymentPlan,Underpayment,ArAging}Test.php` | — | `runbooks/revenue-cycle-mgmt.md` |
| `saas-plan-marketplace` | `app-modules/saas-plan-marketplace/tests/Feature/{Developer,Plan,Subscription,Payout}Test.php` | — | `runbooks/saas-plan-marketplace.md` |
| `on-prem-ai-gateway` | `app-modules/on-prem-ai-gateway/tests/Feature/{Vllm,AirGapped,Hsm,TokenCount,PromptHash,NoLogging}Test.php` | — | `runbooks/on-prem-ai-gateway.md` |
| `soc2-type-ii` | `app-modules/soc2-type-ii/tests/Feature/{Control,ControlTest,ControlException,EvidenceRequest,Observation,Big4Audit}Test.php` | — | `runbooks/soc2-type-ii.md` |
| `byok` | `app-modules/byok/tests/Feature/{Aws,Gcp,Vault,Azure,DekCache,KeyVersion,RevokeTest}Test.php` | — | `runbooks/byok.md` |
| `field-level-encryption` | `app-modules/field-level-encryption/tests/Feature/{EncryptedFieldCast,SearchToken,DataClassification,DriftSweep,KeyRotation}Test.php` | — | `runbooks/field-level-encryption.md` |

### 16.6 V3 lane (10 modules)

| Module | Feature test | Runbook |
|---|---|---|
| `plugin-marketplace` | `app-modules/plugin-marketplace/tests/Feature/{Catalog,Rating,OneClickInstall,RevenueShare}Test.php` | `runbooks/plugin-marketplace.md` |
| `hl7-v2-adt` | `app-modules/hl7v2-adt/tests/Feature/{MllpListener,MirthTransform,HapiFhir,Quarantine,IheAtna,MTls}Test.php` | `runbooks/hl7-v2-adt.md` |
| `fhir-r4-write` | `app-modules/fhir-r4-write/tests/Feature/{SmartLaunch,DualWrite,Conflict,Idempotency}Test.php` | `runbooks/fhir-r4-write.md` |
| `dicom-viewer` | `app-modules/dicom-viewer/tests/Feature/{Ohif,WadoRs,Partner,PdfReport,Retention,ShortLivedJwt}Test.php` | `runbooks/dicom-viewer.md` |
| `custom-report-builder` | `app-modules/custom-report-builder/tests/Feature/{VisualBuilder,SqlSandbox,PgParseTree,Timeout,RowCap,Schedules}Test.php` | `runbooks/custom-report-builder.md` |
| `white-label-mobile` | `app-modules/white-label-mobile/tests/Feature/{TenantConfig,EasBuild,Submit,AssetOverride,Push}Test.php` | `runbooks/white-label-mobile.md` |
| `sso-oidc-saml` | `app-modules/sso-oidc/tests/Feature/{OidcPkce,SamlResponse,Scim,Jit,JustInTimeProvisioning}Test.php` | `runbooks/sso-oidc-saml.md` |
| `bed-management` | `app-modules/bed-management/tests/Feature/{Ward,Bed,Assignment,Occupancy,CleaningState,Discharge}Test.php` | `runbooks/bed-management.md` |
| `ot-room-scheduling` | `app-modules/ot-scheduling/tests/Feature/{Ot,Booking,EquipmentConflict,SurgicalNote,Utilization}Test.php` | `runbooks/ot-scheduling.md` |
| `equipment-booking` | `app-modules/equipment-booking/tests/Feature/{Slot,Booking,Udi,Recall,Fefo}Test.php` | `runbooks/equipment-booking.md` |

### 16.7 V4 lane (6 modules)

| Module | Feature test | Runbook |
|---|---|---|
| `lis-astm` | `app-modules/lis-astm/tests/Feature/{MirthSidecar,AstmE1394,Lis2A2,Astm1394-97,ResultImport,Quarantine}Test.php` | `runbooks/lis-astm.md` |
| `his-bridge` | `app-modules/his-bridge/tests/Feature/{Pix,Pdq,Xdsb,CCda,Epic,Cerner,Meditech,Bidirectional}Test.php` | `runbooks/his-bridge.md` |
| `er-triage` | `app-modules/er-triage/tests/Feature/{Esi1to5,BayAssignment,Pickup,CriticalAlert,Board}Test.php` | `runbooks/er-triage.md` |
| `nursing-station` | `app-modules/nursing-station/tests/Feature/{Handoff,Vitals,Mar,FiveRights,NarcoticWitness,Mews}Test.php` | `runbooks/nursing-station.md` |
| `centralized-pharmacy` | `app-modules/centralized-pharmacy/tests/Feature/{WardStock,ScheduleVault,Witness,IvAdmixture,Fefo}Test.php` | `runbooks/centralized-pharmacy.md` |
| `equipment-maintenance` | `app-modules/equipment-maintenance/tests/Feature/{BiomedicalAsset,PmSchedule,WorkOrder,FdaRecall}Test.php` | `runbooks/equipment-maintenance.md` |

### 16.8 V5+ lane (1 module)

| Module | Feature test | Runbook |
|---|---|---|
| `population-health` | `app-modules/population-health/tests/Feature/{Deidentification,SafeHarbor,CohortDsl,Hedis,Mips}Test.php` | `runbooks/population-health.md` |

**Total test surface:** ~250 V1 + ~80 V1.1 + ~50 V1.2 + ~60 V2 + ~30 V2.1 + ~50 V3 + ~30 V4 + ~5 V5+ ≈ **555+ test files**, ~5,000+ individual test cases.

---

## 17. Runbooks Index

> Per §19.16 acceptance criteria ("each flow has a runbook (in `runbooks/`)"), this is the **module-to-runbook map** for all 59 modules. Runbook stubs are committed under `runbooks/{slug}.md`. The stubs follow a 6-section template: **When to use · Detection · Diagnosis · Mitigation · Resolution · Post-mortem template**. On-call engineers are expected to read the relevant runbook **before** paging the module owner.

### 17.1 The runbook template (all 59 follow this)

```markdown
# Runbook: {module-name} ({slug})

## 1. When to use
- Alert: PagerDuty service `medicare-{slug}-prod` (or similar) page.
- Sentry: error rate > 0.5% over 5 min, or any SEV-1.
- Customer report: support ticket with "{module}" tag.

## 2. Detection
- Datadog dashboard: `https://app.datadoghq.com/dashboard/medicare-{slug}`.
- Key metrics: {module-specific}.
- Sentry project: `medicare-api` filter `module:{slug}`.

## 3. Diagnosis
1. Check the Datadog dashboard for anomalies.
2. Check Sentry for the error fingerprint.
3. SSH to the affected pod / instance: `kubectl exec -it {pod} -- bash`.
4. Check logs: `tail -f /var/log/medicare/{module}.log`.
5. Check DB: `php artisan tinker` → query the relevant tables.
6. Check external dependency: {module-specific}.

## 4. Mitigation
- Option A: Restart the service (`kubectl rollout restart deployment/{slug}`).
- Option B: Disable the feature flag (`php artisan tinker` → `tenants.update(['feature_{slug}' => false])`).
- Option C: Failover to DR region (if multi-region).
- Option D: Rollback to previous version.

## 5. Resolution
- Document the root cause in the incident channel.
- Apply the fix; deploy via standard pipeline.
- Re-enable the feature flag.
- Monitor for 30 min.

## 6. Post-mortem template
- Incident: INC-{date}-{number}
- Duration: {start} → {end}
- Detection: {how}
- Mitigation time: {minutes}
- Resolution time: {minutes}
- Customer impact: {N affected}
- Root cause: {5-why}
- Action items: {assignee, due date}
```

### 17.2 Per-module runbook links (59 modules)

#### V1 (57 V1 modules)
The 53 active V1 modules are covered by existing runbooks at `runbooks/{module}.md` (per `00-ARCHITECTURE §9` on-call runbook index). Runbooks cover the standard failure modes: DB connection loss, queue worker down, Reverb cluster, gateway webhook failures, FHIR / HL7 endpoint down, cron job failures.

#### V1.1 lane (12)
- `runbooks/whatsapp-business.md` — Meta Cloud API outage, quality rating drop, template rejection, 24h window expiry
- `runbooks/recurring-appointments.md` — RRULE expander stuck, DST transition failure, exdate / rdate corruption, idempotency-key collision
- `runbooks/group-appointments.md` — partial unique index violation, row-lock deadlock, group video room provisioning failure
- `runbooks/family-booking.md` — OTP delivery failure, dependent age-out, minor-mode transition, consent revocation
- `runbooks/multi-currency-payout.md` — FX rate stale, gateway transfer failure, tax form missing (TDS / 1099), payout run stuck
- `runbooks/geo-search.md` — PostGIS index corruption, Mapbox tile precompute failure, fallback to Haversine, geocoding timeout
- `runbooks/marketing-vouchers.md` — referral credit misuse, partner pool exhausted, voucher exhaustion storm, fraud detection hold
- `runbooks/dynamic-refund-window.md` — policy priority conflict, sliding scale edge case, force majeure override, credit expiry
- `runbooks/smart-waitlist.md` — cascade stuck, claim race condition, quiet hours misconfig, group cascade partial
- `runbooks/i18n-rtl.md` — locale switch 500, RTL visual regression, translation import failure, glossary conflict
- `runbooks/doctor-rotating-schedule.md` — coverage gap, escalation chain broken, swap deadlock, fairness score drift
- `runbooks/pwa-offline.md` — service worker install fail, IndexedDB quota, Background Sync replay loop, push delivery fail, version update stuck

#### V1.2 lane (8)
- `runbooks/abdm-bridge.md` — NHA gateway OAuth 2.0 expire, mTLS cert mismatch, ConsentArtifact corruption, ABDM cert M1/M2/M3 renewal
- `runbooks/fhir-r4-read.md` — HAPI-FHIR sidecar down, capability statement stale, SMART token expiry, bulk export timeout
- `runbooks/razorpay-subscriptions.md` — mandate auth fail, dunning retry exhausted, proration calc drift, pause limit hit (3 mo)
- `runbooks/paystack-subscriptions.md` — subscription code lost, mobile-money bank timeout, dunning exhausted, region-specific (NG/GH/ZA/KE) issues
- `runbooks/stripe-terminal.md` — reader disconnect, location offline, offline mode overflow, partial capture fail
- `runbooks/twilio-sms.md` — Verify API down, A2P 10DLC TCR rejection, DLT India template fail, opt-out storm, 2-way SMS webho ok fail
- `runbooks/local-payment-methods.md` — UPI Intent timeout, Pix webhook lag, M-Pesa STK push fail, OXXO voucher expiry
- `runbooks/multi-clinic-dashboard.md` — materialized view stale, chain_admin impersonation block, PII export co-sign missing, revenue distribution miscalc

#### V2 lane (11)
- `runbooks/pathology-lab.md` — lab partner webhook down, LOINC unmapped, critical result not paged, partner HMAC signature fail
- `runbooks/telemedicine-video.md` — LiveKit cluster down (fallback to Daily/Jitsi), TURN credential stale, recording upload fail, 2-party consent revoke
- `runbooks/ai-symptom-checker.md` — OpenAI / Anthropic rate limit, red-flag detection miss, privacy log leak, prompt injection
- `runbooks/ai-clinical-scribe.md` — STT service down, SOAP section timeout, withdraw-consent purge stuck, sign-off audit fail
- `runbooks/in-app-chat.md` — message persistence fail, attachment scan (ClamAV) timeout, chart promotion race, encryption-at-rest key rotation
- `runbooks/plugins-v1.md` — plugin exception isolated, capability denied storm, signature verify fail, kill switch false positive, tenancy leak attempt
- `runbooks/custom-domain.md` — DNS-01 verify fail, ACM cert renewal fail, CAA record reject, HSTS preload request fail
- `runbooks/multi-region-dr.md` — primary region down (failover to passive), replication lag > RPO 5 min, drill fail, DNS failover
- `runbooks/drug-interaction-db.md` — RxNav API down, DrugBank sync fail, hard-stop override attempt, p99 latency spike
- `runbooks/e-prescription.md` — DSC cert expire (IN), EPCS token (US), JWS ES256 sign fail, transmission to pharmacy fail
- `runbooks/insurance-preauth.md` — 270/271 timeout, NHCX gateway (IN) down, 278 denial auto-categorization fail, 835 ERA post drift

#### V2.1 lane (6)
- `runbooks/revenue-cycle-mgmt.md` — denial worklist stuck, dunning retry exhausted, payment plan broken, A/R aging recalc drift
- `runbooks/saas-plan-marketplace.md` — developer KYC fail, subscription stuck in `past_due`, payout run fail, revenue split miscalc, Stripe Connect transfer fail
- `runbooks/on-prem-ai-gateway.md` — vLLM down, HSM unreachable, prompt-hash mismatch, audit log leak, air-gapped license renewal
- `runbooks/soc2-type-ii.md` — control test overdue, exception discovered mid-audit, evidence request overdue, Big-4 firm audit prep
- `runbooks/byok.md` — KMS provider outage (AWS / GCP / Vault / Azure), DEK cache stale, key version old (30-day grace), emergency revoke fail
- `runbooks/field-level-encryption.md` — `EncryptedFieldCast` failure, search token drift, classification sweep fail, key rotation broken

#### V3 lane (10)
- `runbooks/plugin-marketplace.md` — catalog sync fail, rating / review spam, one-click install race, revenue share miscalc
- `runbooks/hl7-v2-adt.md` — MLLP listener down (port 6661), Mirth transform fail, HAPI-FHIR sync lag, quarantine overflow, mTLS cert mismatch, IHE ATNA audit fail
- `runbooks/fhir-r4-write.md` — SMART launch token fail, dual-write conflict storm, idempotency-key collision, FHIR resource validation fail
- `runbooks/dicom-viewer.md` — OHIF viewer down, WADO-RS partner endpoint fail, short-lived JWT exp, 7-year retention purge stuck
- `runbooks/custom-report-builder.md` — SQL sandbox escape attempt, pg_parse_tree validation fail, 30s statement_timeout, 1M row cap hit, schedule fail
- `runbooks/white-label-mobile.md` — EAS build fail, asset override cache, Submit rejection (App Store / Play), push cert mismatch
- `runbooks/sso-oidc-saml.md` — OIDC PKCE fail, SAML Response signature fail, SCIM v2 webhook fail, JIT provisioning race
- `runbooks/bed-management.md` — bed_assignments row-lock deadlock, cleaning state stuck, occupancy recalc drift, transport order fail
- `runbooks/ot-scheduling.md` — OT slot double-book, equipment conflict fail, surgical note lost, utilization recalc drift
- `runbooks/equipment-booking.md` — slot double-book, UDI lookup fail, FDA recall API sync fail, FEFO enforcement drift

#### V4 lane (6)
- `runbooks/lis-astm.md` — Mirth sidecar down, ASTM E1394 framing error, LIS-2A2 record fail, result import timeout
- `runbooks/his-bridge.md` — Epic / Cerner / Meditech endpoint down, PIX / PDQ query fail, XDS.b registry fail, C-CDA parse error
- `runbooks/er-triage.md` — ESI 1 (resuscitation) mis-sort, bay assignment fail, critical alert not paged, board refresh fail
- `runbooks/nursing-station.md` — handoff SBAR incomplete, MAR Five Rights hard-stop, narcotic witness missing, MEWS threshold miss
- `runbooks/centralized-pharmacy.md` — ward stock reorder fail, Schedule II–V vault access denied, witness requirement fail, IV admixture calculation drift
- `runbooks/equipment-maintenance.md` — PM schedule overdue, work order stuck, FDA recall API down, biomedical asset record drift

#### V5+ lane (1)
- `runbooks/population-health.md` — de-identification pipeline fail, cohort DSL syntax error, HEDIS measure recompute fail, MPS measure drift

### 17.3 Runbook standards

- **Owner:** every runbook has a `module_owner` field at the top; the on-call engineer pages the owner if the runbook does not resolve within 30 min.
- **Tested quarterly:** every runbook is exercised in a quarterly game-day drill (per `21-DEPLOYMENT §18.5` for DR); results are logged in the runbook header (last-tested date).
- **Linked from PagerDuty:** every PagerDuty service has a `runbook_url` field that points to the relevant runbook.
- **Linked from Sentry:** every Sentry alert rule has a `runbook_url` field.
- **Linked from status page:** every status page incident has a runbook link for the status-engineer.
