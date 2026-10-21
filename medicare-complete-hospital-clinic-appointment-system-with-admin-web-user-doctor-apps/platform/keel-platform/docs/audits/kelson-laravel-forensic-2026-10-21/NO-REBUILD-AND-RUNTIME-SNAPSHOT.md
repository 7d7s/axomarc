# NO-REBUILD AND RUNTIME SNAPSHOT — the process-level audit

**Status: BLOCKED — and the block has a cause.**

This is Kelson's flagship product promise: *config/route/policy hot changes do not rebuild or
restart the stable kernel*. The audit's rule is that a unit fixture or a simulated hash does not prove
it — a real running process must, with its identity and its digest captured.

## 1. What was attempted

| Step | Command / observation | Result |
|---|---|---|
| The baseline | `git rev-parse HEAD` | `722a8931794cbfae5f95265c7a26d44e4ec6a71f` |
| The kernel artefact | `Get-FileHash target\debug\kelson-demo.exe -Algorithm SHA256` | `F458800DC433CA8B09E7149A99A6B0D59C86E358A8250668CD2869F8F57AFDC0` |
| The first boot | `KEEL_CONFIG=config/dev.toml kelson-demo` | **refused to start**: `CONFIG-CACHE-STALE: config cache … is stale (config file or environment drift); run kelson config:cache to rebuild` — a **good** fail-closed posture ✓ (recorded as a positive ✓) |
| The cache rebuild | `kelson config:cache` | wrote `storage/framework/config.json` (hash `96cb7da094835e8f8830f12671ede3c525dce4af92a0359b578490da8e12c456`) ✓ |
| The second boot | `KEEL_CONFIG=config/dev.toml kelson-demo` | the lifecycle logs ✓, then `database unreachable; booting degraded on in-memory backend` (a WARN ✓), then `wasm plugin loaded` ✓ — and then **`thread 'main' (7924) has overflowed its stack`** ✗ |
| The one-shot path | `kelson-demo --call echo.echo --input {"message":"x"}` | **the same overflow** — so the defect is in the **boot** path, not the server path ✗ |
| The control experiments | `kelson graph` ✓, `kelson route:list` ✓, `kelson check` ✓ | all exit 0 — the graph, the route plan and the architecture gate are **not** implicated ✗ ✓ |

Raw: `RAW-EVIDENCE/demo-boot.stderr`, `RAW-EVIDENCE/demo-boot.stdout`,
`RAW-EVIDENCE/demo-call.stderr`, `RAW-EVIDENCE/nr01-partial.txt`.

## 2. What the machinery looks like (read from the source)

The no-rebuild machinery is **present and tracked**:

- `runtime/kelson-demo/src/hot_reload.rs` — the `ConfigReloader` ✓, the `RouteReloader` ✓, the
  `FlagsReloader` ✓, the swappable `Arc` cell ✓ and the `SwappableGraphRoutes` fallback ✓ ✓.
- `runtime/kelson-demo/src/lib.rs` — `pub mod hot_reload;` ✓, the bundle cell ✓, the router fallback ✓
  and the **poller**: "config file mtime every 500ms PLUS the route artifact's own mtime ✓, so a
  `kelson route:cache` rewrite lands with no config edit ✓. Every swap is Arc-level behind a read
  lock — no restart, in-flight requests unaffected ✓" ✓ ✓.
- `crates/application/configuration` — the `ConfigRuntime` ✓, the `FileConfigSource` ✓ and the
  `ConfigReloader` trait ✓ ✓.
- The boot **refuses a stale config cache** ✓ — the "prepare/validate publish nothing" invariant is
  enforced at boot ✓ ✓.

**So the design is sound and the implementation is tracked — the runtime simply cannot be started
on this host.** The project's own P0 execution report records the same conclusion from the other
direction: *Phase 1 — No-Rebuild Proof: NOT RUN (infrastructure limitation)* ✓ ✓. This audit adds
the **cause** ✓: the host aborts during boot ✓ (KFA-005 ✓).

## 3. The per-experiment status (the audit's table)

| # | Experiment | Status | Why |
|---|---|---|---|
| NR-01 | a compatible config value | **BLOCKED** | the host aborts before the listener binds ✓ |
| NR-02 | a route add/remove/target | **BLOCKED** | same ✓ |
| NR-03 | a policy / feature-flag change | **BLOCKED** | same ✓ |
| NR-04 | a tenant override | **BLOCKED** | same ✓ |
| NR-05 | a compatible module implementation | **BLOCKED** | same ✓ |
| NR-06 | an invalid candidate | **BLOCKED** | same ✓ |
| NR-07 | a stale competing candidate | **BLOCKED** | same ✓ |
| NR-08 | concurrent requests during an activation | **BLOCKED** | same ✓ |
| NR-09 | a kernel-code change | **NOT RUN** | needs a release procedure ✓ |
| NR-10 | a multi-node update | **NOT RUN** | needs a second node ✓ |

**None of these may be reported as a pass.** The audit records them as blocked ✓ ✓.

## 4. The hard invariants — what is proven, what is not

| Invariant | Status | Evidence |
|---|---|---|
| A hot config/route/policy change does not rebuild or restart the kernel | **NOT PROVEN** | the process-level experiment is blocked ✓; the *design* (the 500 ms poller, the Arc swap) is read from the source ✓, which is **not** evidence ✓ |
| A module implementation change builds only the module artifact | **NOT PROVEN** | the same block ✓ |
| A kernel/ABI change may require a kernel release | **NOT PROVEN** | no release procedure was exercised ✓ |
| Prepare/validate publish nothing and have no irreversible side effects | **PARTIALLY PROVEN** | the ADR 0019 prepare/commit split is read in the source ✓ and the boot refuses a stale cache ✓ — but no failure-injection experiment ran ✓ |
| Activation is atomic within the declared model | **NOT PROVEN** | the swap is Arc-level behind a read lock ✓ (a design claim ✓), not a measurement ✓ |
| A stale candidate cannot overwrite a newer generation | **NOT PROVEN** | no experiment ran ✓ |
| A failed activation leaves the previous generation active | **NOT PROVEN** | the promotion's abort path restores the checkpoint ✓ (the unit cases ✓) — but at the *runtime* level, unproven ✓ |
| Each request pins exactly one coherent generation | **NOT PROVEN** | the bundle cell ✓ (a design claim ✓) ✓ |
| A secret/config change uses safe handling, an audit trail and rollback rules | **PARTIALLY PROVEN** | the config cache's pinning ✓, the digests ✓, the drift detector ✓ — the audit trail for a config change ✗ ✓ |
| Multi-node state has an explicit convergence behaviour | **NOT PROVEN** | one node only ✓ ✓ |

**Nine of the eleven invariants are unproven** — the two with partial evidence are the ones that can be
checked without a running process ✓ ✓. That is the honest state of the flagship promise ✓ ✓.

## 5. What would close this

1. **Fix the boot** (KFA-005 ✓): run the boot on a thread with an explicit stack size, or split the
   boot future ✓ ✓. Then the host serves and every experiment above becomes runnable ✓ ✓.
2. **Automate the experiments**: a script that starts the real binary ✓, records the PID ✓, the
   executable digest ✓, the generation ids ✓ and the before/after behaviour ✓, and writes one signed
   record per change class ✓ ✓ — the raw artefact, not a claim ✓ ✓.
3. **Run it in CI** (the CI runner is Linux, where the 8 MiB stack hides the defect ✓ — the boot smoke
   must run on **both** platforms ✓ ✓, or the Windows host stays broken ✓ ✓).
4. **Publish the per-invariant status** with the raw records ✓ ✓ — the same discipline the parity
   matrix now uses ✓ ✓.
