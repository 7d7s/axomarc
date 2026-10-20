# kelson-session-redis

Redis-backed `SessionStore` for Kelson (GET / SET EX / DEL / SCAN).

- Keys are namespaced `keel:session:<session id>`; the prefix is
  configurable with `with_prefix`.
- The stored row is the session JSON; the key carries a TTL derived from the
  session policy (the shorter of the idle and the absolute remaining time), so
  **Redis** expires a session, not the application.
- `count()` counts with a SCAN over the store's own prefix, so several
  applications can share one Redis database.

Integration tests run behind `--features redis-tests` with `REDIS_URL` set;
without it they skip.

```text
REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-session-redis --features redis-tests
```

`tests/live_redis.rs` covers the store against a real server: the session
round trip (bag intact), two connections sharing one store (the property a
multi-instance deployment depends on), the stored key carrying its TTL, and
`count()` seeing only its own namespace.
