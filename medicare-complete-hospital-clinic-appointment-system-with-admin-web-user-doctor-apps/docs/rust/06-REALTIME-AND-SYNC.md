# 06 — Realtime & Sync Design

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> V1 constraint: no message broker. Fan-out uses an in-process WebSocket hub plus Postgres LISTEN/NOTIFY for cross-node propagation.

---

## 1. Components

| Component | Role |
|---|---|
| Hub (kernel-notify) | In-process channel registry; topic → subscriber set; per-subscriber StreamFilter (from `05-SECURITY-ISOLATION.md` §4) |
| PG bridge | `LISTEN kernel_deltas` on every API node; on NOTIFY, hub matches local subscribers only |
| Socket gateway | axum WS endpoint; auth handshake; subscribe/unsubscribe protocol; heartbeat |
| Outbox worker | Drains `outbox` → webhooks/push/SMS/email; backoff + dead-letter |
| Cursor APIs | Durable catch-up (see `04-API-CONTRACTS.md` §12) |

Flow: `module handler commits tx (delta+outbox)` → `NOTIFY` → each node's hub filters → `WS frame to allowed subs` and/or `outbox row picked up by worker → provider`.

---

## 2. Socket Lifecycle

1. Client calls `POST /api/v1/auth/socket-token` → short-lived (60s) one-time socket token bound to the session.
2. `GET /realtime/v1/ws?token=…` → upgrade; server resolves Subject + tenant from token; connection record kept (conn_id, subject, node_id, connected_at).
3. Client sends `sub` frames: `{channel: "queue", scope: "doctor:<id>" | "node:<node_id>"}`.
4. Server validates each subscription against capability snapshot; rejected subs get `sub_denied` frame with error code.
5. Heartbeat every 25s (`ping/pong`); missed ×2 → close; server pushes `snapshot_version` so clients know when to re-bootstrap.
6. On capability snapshot invalidation (grant change), hub re-validates all live subscriptions for affected subjects; drops what no longer passes; sends `resub` hint frame.

---

## 3. Channels (V1)

| Channel | Payload source | Typical clients |
|---|---|---|
| `queue:{node_id}` | queue_token deltas, appointment transitions | waiting-room display, staff app |
| `calling:{appointment_id}` | call event + token state | patient app (live doctor-call) |
| `today:{doctor_user_id}` | appointment/queue deltas for doctor's day sheet | doctor app |
| `board:{node_id}` | aggregate counters (waiting, in-consult, done) | dashboards |
| `rx:{patient_ref}` | prescription issued events | patient app |
| `deltas:{streams}` | generic delta nudge (seq only) | mobile sync clients |

Frames are delta-shaped (`{stream, seq, aggregate, aggregate_id, action, state}`) so clients reuse one apply path for WS and cursor sync.

---

## 4. Nudge-not-Payload for Mobile Push

- FCM/APNs carries **data-only nudges**: `{tenant, streams, cursor_hint}`; never PHI, never aggregate payload.
- App wakes, calls `/sync/deltas` with its cursors → scoped, auditable, resumable. This keeps push providers out of the PHI path and works through doze/permissions.

---

## 5. Webhooks (tenant-facing)

- Source: outbox; batching window 1s or 64 deltas, whichever first.
- Signing: `X-Medicare-Signature: t=<ts>,v1=<hmac-sha256(secret, ts.body)>`; 5-min replay window.
- Retries: 1m, 5m, 30m, 2h, 12h → dead-letter; dashboard + manual replay endpoint.
- Per-endpoint circuit breaker: sustained 5xx → pause endpoint, alert tenant admin; other endpoints unaffected.

---

## 6. Backpressure & Failure Modes

| Failure | Behavior |
|---|---|
| Slow WS consumer | Per-connection bounded send buffer (1024 frames); overflow → close with `resume_with_cursor` frame; client resyncs via API |
| Node crash | sessions reconnect (socket token re-issue); missed frames recovered via cursors; hub state is all derived, nothing to restore |
| PG NOTIFY lost (standby promotion, reconnect gap) | hub periodically emits `cursor_probe`; clients compare seqs → request gap fill |
| Outbox worker down | rows accumulate (metric: outbox depth alert); delivery resumes; dedupe_key prevents duplicate side-effects at providers where supported |
| Storm (mass check-ins) | hub coalesces board/counter channels (send latest aggregate state per 500ms window); raw delta channels never coalesce |

---

## 7. Scale Path

1. **V1 single node** — hub in-process; NOTIFY unused but wired.
2. **V1 multi-node** — N API nodes behind LB; sticky sessions not required (any node can LISTEN); WS connection count per node capped (config), horizontal scale by adding nodes.
3. **V2 option** — swap NOTIFY for NATS/Redis Streams by replacing one bridge trait implementation; hub and filters unchanged (this is why fanout filtering lives in the hub, not in SQL triggers).

---

## 8. Offline Sync Protocol (client contract)

```
boot (fresh)     → /sync/bootstrap        → snapshot + cursors
wakeup/nudge     → /sync/deltas?after=…   → apply deltas by (aggregate_id, version)
apply rule       → skip if version <= local; else replace aggregate state
conflict policy  → server-authoritative; local writes go through API with Idempotency-Key
backgrounded >7d → may fall past retention → must re-bootstrap that stream
```

Clients persist `cursors` + last-known `snapshot_version` per stream. Doctor app with flaky clinic Wi-Fi is the reference scenario (PRD scenario 10).