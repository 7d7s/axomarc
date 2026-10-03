# Dependency Policy v2 (D-62)

Supersedes the v1 "budget 20" note. Frozen by D-62 (STAGE-0-SPEC §15): a
justification table replaces the soft count budget; hard cap 25 direct
externals, enforced by the analyzer (keel check -> ANALYZER-BUDGET).

## Budget
<= 25 direct external dependencies at the workspace level.
Current count: 25.

## Admission criteria
1. Maintenance: active releases, responsive security handling.
2. Weight: prefer small, focused crates; avoid meta-frameworks.
3. License: MIT / Apache-2.0 / BSD. Copyleft requires a governance decision.
4. Native code: needs explicit approval (compile-time + supply-chain cost).
5. Every addition gets a row in the table below BEFORE it lands in Cargo.toml.

## Justification table (direct externals)

| Crate | Role | License | Notes (MSRV / audit / compile / unsafe / size) |
|---|---|---|---|
| axum | HTTP transport + middleware | MIT | Rust Foundation project; no unsafe hot path |
| tokio | Async runtime | MIT | Standard runtime; features capped to macros/rt/signal/time |
| tower | Service/Layer traits | MIT | Small; pulled transitively anyway |
| tower-http | CORS (+trace) middleware | MIT | Only cors/trace features enabled |
| serde | Serialization | MIT/Apache-2.0 | Universal standard |
| serde_json | JSON values | MIT/Apache-2.0 | Universal standard |
| sqlx | SQL fabric: Postgres pool + migrate; MySQL pool + migrate (Wave 9 B-31, `mysql` feature — same external, no new crate) | MIT/Apache-2.0 | Default features OFF; runtime-tokio + rustls only; the mysql feature rides the existing dependency (cap stays 25) |
| uuid | Typed ids (v7) | MIT/Apache-2.0 | v7 + serde features only |
| time | Clock/dates | MIT/Apache-2.0 | serde/formatting/parsing/macros features |
| thiserror | Error derives | MIT/Apache-2.0 | Tiny proc-macro |
| tracing | Structured events | MIT | Standard instrumentation |
| tracing-subscriber | Log init (json/pretty) | MIT | json + env-filter features |
| moka | Bounded cache; S5: bounded registries (dedupe windows, label cardinality, exception groups) | MIT/Apache-2.0 | future + sync features; used by keel-cache-memory adapter, keel-pipeline (dedupe), keel-metrics (label bound), keel-observability (exception groups) |
| clap | CLI parsing | MIT/Apache-2.0 | derive feature |
| toml | Config manifests | MIT/Apache-2.0 | Small parser |
| argon2 | Password hashing | MIT/Apache-2.0 | Pure Rust; PHC format |
| anyhow | CLI error plumbing | MIT/Apache-2.0 | Tools only |
| rand | Key/token material, OTP, salts | MIT/Apache-2.0 | 0.8 line (matches argon2 salt usage); CSPRNG via getrandom; first justified D-62 add |
| sha2 | Token hashing (sha256); S5: audit hash chain, WAL checksums, fingerprinting | MIT/Apache-2.0 | Pure Rust, RustCrypto; first justified D-62 add; used by keel-tokens, keel-session, keel-integrity (D-74 audit chain), keel-buffer (WAL record checksums), keel-telemetry/keel-pipeline (fingerprints), keel-http, keel-feature-flags, keel-webhooks |
| reqwest | Outbound HTTP client (webhook delivery, live SMS transport) | MIT/Apache-2.0 | default-features OFF; json + rustls-tls + blocking only; used by keel-http-client + adapters |
| tokio-stream | Broadcast -> SSE stream bridging | MIT | sync feature only; tiny tokio companion crate; used by keel-realtime |
| lettre | SMTP mail delivery | MIT | default-features OFF; builder + smtp-transport + rustls-tls; pure-Rust TLS; used by keel-mail-smtp adapter |
| aes-gcm | At-rest secret encryption (AES-256-GCM envelopes, L27) | MIT/Apache-2.0 | Pure Rust, RustCrypto; audited cipher line next to sha2; used by keel-tokens only |
| redis | Optional bounded-cache/queue driver (adapters) | MIT/Apache-2.0 | socket feature only; used by the keel-cache-redis adapter; not on the default path |
| wasmtime | Sandboxed WASM plugin runtime (Wave 9 B-30) | Apache-2.0 WITH LLVM-exception | D-62 amendment recorded by docs/19 Wave 9 B-30; 34.x line, MSRV 1.85 = workspace MSRV; default features OFF (cranelift/runtime/wat/std only: no component-model, no cache, no profiling); fuel metering + store-limits sandbox, fail-closed import surface; reaches the 25-crate cap (25/25) |

## Rules
- Core (crates/foundation/core) uses only the pinned minimal set (serde, serde_json, time, uuid,
  thiserror) + internal crates.
- Adapters may add exactly one driver each (e.g. sqlx for postgres).
- Upgrades that change MSRV or add features require an ADR.
- Dev-dependencies count against the same cap when they are new externals.
