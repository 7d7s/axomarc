# ARCHITECTURE BOUNDARIES

The dependency matrix is **declared, enforced and green** ✓ — this is one of Kelson's genuine
strengths, and the audit found no prohibited crossing ✓.

## 1. The declared matrix

`tools/kelson-analyzer/src/lib.rs:128` — `Class::allowed_deps` ✓. The classes are derived from each
member's **path** (`crates/<class>/…` ✓, `adapters/` ✓, `runtime/` ✓, `tools/` ✓, `testing/` ✓,
`extensions/` ✓, `modules/` ✓) ✓.

| Class | May depend on |
|---|---|
| Foundation | Foundation |
| Security | Foundation, Security |
| Execution | Foundation, Security, Execution |
| Application | Foundation, Security, Execution, Application, **Module** (ADR 0020: an application module is built on the module SDK — the direction stays one-way ✓) |
| Web | Foundation, Security, Execution, Application, Services, Web (D-77: the upload edge reaches object storage ✓) |
| Services | Foundation, Security, Execution, Application, Services |
| Async | Foundation, Security, Execution, Application, Async |
| Observability | Foundation, Observability |
| Enterprise | Foundation, Security, Execution, Application, Web, Services, Async, Observability, Enterprise |
| Testing | Foundation, Security, Execution, Application, Web, Services, Async, Observability, Data, Testing |
| Ai | Foundation, Security, Execution, Application, Web, Services, Async, Ai |
| Data | Foundation, Security, Execution, Application, Services, Async, Data |
| Adapters | Foundation, Security, Services, Async, Adapters (an adapter depends on its port's owner; the security layer never depends on an adapter ✓) |
| Runtime | Foundation, Security, Execution, Application, Web, Services, Async, Observability, Data, Runtime, Extension (D-77: the runtime host instantiates and dispatches WASM plugin handlers ✓) |

## 2. The evidence that it holds

| Check | Command | Result |
|---|---|---|
| The gate | `cargo xtask dep-rules` | **clean** |
| The same engine from the CLI | `kelson check` | "dependency matrix + budgets clean (84 members)" ✓; "graph compiles; middleware catalog resolves every route" ✓ |
| In CI | the `platform` and `keel-platform` jobs run `cargo run -p xtask -- dep-rules` ✓ | enforced on every push ✓ |

**No prohibited crossing was found** ✓ — including the two the audit's own history found: the
rate-limit port that once sat in the web layer while its adapter lived in the adapters layer (F-20 ✓
— moved to the foundation ✓) ✓.

## 3. The boundaries that the audit did **not** prove

The matrix governs **crate-to-crate** dependencies ✓. It does not govern:

| Boundary | Status | Note |
|---|---|---|
| The kernel vs. the module seam | **PARTIALLY PROVEN** | the ADR 0019 prepare/commit split is read in the source ✓; the guest's capability set is sidecar-verified ✓; but the **wasmtime miscompilation advisories** (KFA-001 ✓) attack exactly this boundary ✓ — the matrix cannot see a compiler defect ✓ ✓ |
| The **runtime state** vs. the request state | **NOT PROVEN** | the context carries the principal/tenant/capabilities ✓ and the tenant scope is per-request ✓; no case proves a long-lived worker's state cannot leak between jobs **in the presence of the panic boundary** ✓ (the F-25 worker's panic → a failed attempt ✓ → the state is dropped ✓ ✓ — that is proven ✓ ✓) |
| The **global mutable state** | **PROVEN for the audited paths** | the `QueryTap` is process-global **by design** ✓ and the testkit serializes around it ✓; the global state count is small ✓ but no rule forbids a new global ✓ ✗ |
| The **deployment orchestration** vs. the request path | **PROVEN** | the promotion runs in the console ✓, the lock is a file on the release volume ✓, the request path never touches it ✓ ✓ |
| The **secrets** vs. the logs | **PROVEN per surface** | the `Secret` type is redacted ✓; the per-surface coverage is not systematic ✓ (SEC-09 ✓) |

## 4. The architecture findings this audit adds

| # | Finding | Where |
|---|---|---|
| 1 | The **synchronous `StoragePort`** makes the layer boundary a *blocking* boundary: the web layer's async ingest calls a sync port that does file IO on the executor thread ✓ | KFA-015 ✓ |
| 2 | The **adapters layer** may depend on `Services` and `Async` ✓ — which is how the rate-limit adapter reaches the cache port ✓ — but it may **not** depend on `Web` ✓; the audit found no violation ✓ and the matrix is the proof ✓ | — ✓ |
| 3 | The **module SDK**'s 15 `unsafe` sites sit behind the module boundary ✓ — the one place where a memory-safety defect becomes a sandbox escape ✓, and it is **not** fuzzed** ✗ (no target exists ✓) | KFA-001 ✓ + the coverage ledger ✓ |

## 5. The conclusion

**The architecture is the strongest part of Kelson and it is enforced** ✓ ✓. The audit's
architecture findings are not layer violations ✗ — they are *runtime* facts about a correct layering
✓: a sync port that blocks ✓, a compiler defect that crosses the sandbox ✓, and a runtime that
cannot start ✓ ✓.

That is the audit's clearest single message: **Kelson's structure is right; its proof and its
supply chain are not** ✗ ✗ — which is exactly where the roadmap's Stage 0 and Stage 1 spend their
effort ✓ ✓.
