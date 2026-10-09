# keel-framework

The Keel umbrella crate. One dependency instead of two dozen.

```toml
[dependencies]
keel-framework = "0.1"
```

## What it is

`keel-framework` re-exports the published, consumer-facing surface of Keel.
Application modules depend on this crate rather than reaching into the
framework's internal crates directly.

```rust
use keel_framework::{
    ApplicationHost, PlatformConfig, DbPool, PolicyEngine, SessionStore,
};
```

It contains **no logic and no new external dependencies** — it is a pure
re-export surface. Pulling it in does not change your dependency budget.

## What it deliberately does not expose

Internal infrastructure stays out of the umbrella so that the public surface
stays reviewable: the metrics/telemetry pipeline, tracing, buffer and export
layers, and the integrity internals. Reach for those crates individually when
you need them.

## Choosing a database fabric

PostgreSQL and MySQL drivers are both re-exported. Your application selects
one through `KEEL_DATABASE_URL` / config; you do not need both.

## License

MIT OR Apache-2.0.
