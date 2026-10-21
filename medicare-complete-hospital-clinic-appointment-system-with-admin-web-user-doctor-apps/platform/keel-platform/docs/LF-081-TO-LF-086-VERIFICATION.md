# LF-081 … LF-086 — CLI, diagnostics, AI and framework DX

**Batch:** the six CLI/diagnostics/AI items of the Laravel Feature Atlas, verified against the framework
as it stands.

**Method (the project's rule):** every status below is backed by a **named, executed** test. A
documentation claim is never proof. Where an item is only partly covered, the missing cases are named.

**Verdicts:** **0 VERIFIED** · **5 PARTIALLY VERIFIED** · **1 NOT IMPLEMENTED**.

---

## 1. What was executed

| Suite | Result |
|---|---|
| `cargo test -p kelson-console --bin kelson` | **63 passed** — the CLI parsing ✓, the destructive-command gates ✓, the non-interactive posture ✓, the args quoting ✓, the redacted inspect output ✓, the queue/schema dumps ✓ and the scaffold guards ✓ |
| `cargo test -p kelson-codegen --lib` | **17 passed** — the generator kinds ✓, the deterministic emission ✓, the workspace member ✓, the overwrite refusal ✓ and the **scaffold-compiles** guard ✓ |
| `cargo test -p kelson-ai --lib` | **3 passed** — the message constructors ✓, the **mock provider** ✓ and the deterministic embedding ✓ |
| `cargo test -p kelson-ai-tools --lib` | **3 passed** — the tool registry ✓ and the tool descriptors generated from the command/query contracts ✓ |
| `cargo test -p kelson-ai-agents --lib` | **4 passed** — the **capability ceiling** ✓, the engine execution ✓, the **step budget** ✓ and the unknown-tool error ✓ |
| `cargo test -p kelson-ai-mcp --lib` | **3 passed** — the manifest shape ✓, the round trip ✓ and the tools following the platform graph ✓ |
| `cargo test -p kelson-demo --test http_integration` | **100 passed** — incl. the diagnostics edge ✓ (`dashboard_served_in_test_env_and_flag_gated_when_off` ✓, `healthz_is_liveness_and_readyz_reflects_degraded_state` ✓) |

**93 named cases** across the seven crates, plus the demo's diagnostics edge ✓.

---

## 2. Item by item

### LF-081 — Artisan console and scaffolding — **PARTIALLY VERIFIED**

- **Contract**: a command registry; input/output; signatures/options; interactive prompts; `make:*`
  generators; command scheduling; exit codes. Commands are discoverable, deterministic and composable;
  destructive commands require safeguards; generators produce compiling code.
- **Implementation**: the **console** ✓ — **65 commands** ✓ (`kelson-console` ✓ — `migrate:*` ✓,
  `queue:*` ✓, `config:*` ✓, `route:*` ✓, `schedule:*` ✓, `db:*` ✓, `cache:*` ✓, `flag:*` ✓,
  `event:list` ✓, `job:list` ✓, `mcp:tools` ✓, `make:module` ✓, `make:migration` ✓, `pkg:*` ✓,
  `promote` ✓, `rollback` ✓, `optimize` ✓, `doctor` ✓, `call` ✓ …), the **signatures** ✓ (clap ✓),
  the **exit codes** ✓ (`ExitCode::SUCCESS`/`FAILURE` ✓ propagated per command ✓), the **scheduling**
  ✓ (`schedule:run` ✓ / `schedule:work` ✓), and the **generators** ✓ (`kelson-codegen` ✓ — twelve
  generator kinds ✓, a deterministic emission ✓ and the workspace-member edit ✓).
- **Evidence**: the **discoverability** ✓ — 65 registered commands ✓, each with a documented name ✓
  and signature ✓; the **parsing** ✓ — `call_command_parses_with_documented_flags` ✓,
  `db_commands_parse_with_documented_names` ✓, `migrate_pair_commands_parse_with_documented_defaults` ✓,
  `queue_admin_commands_parse_with_documented_defaults` ✓, `promote_and_rollback_parse_with_documented_flags` ✓,
  `model_prune_cli_parses` ✓; the **exit codes** ✓ — `schedule_commands_exit_cleanly_when_nothing_is_registered` ✓
  (a clean exit ✓) and `doctor` ✓'s `FAILURE` path ✓; the **destructive safeguards** ✓ —
  **`the_destructive_commands_refuse_outside_dev_and_test`** ✓ (new ✓ — `migrate:fresh` ✓,
  `migrate:reset` ✓, `migrate:refresh` ✓ and `queue:clear` ✓ each refuse outside dev/test ✓ and name
  themselves in the refusal ✓) plus `migrate_plan_reaches_for_no_mutation_path` ✓ (the read-only
  plan ✓) and `batch_commands_validate_before_connecting` ✓ (the pre-flight ✓ before any connection ✓);
  the **non-interactive mode** ✓ — **`the_noninteractive_path_never_waits_on_a_prompt`** ✓ (new ✓ — a
  name given is used as-is ✓ and with no name and no terminal the console refuses with an actionable
  message ✓, never hanging ✓); the **args quoting** ✓ — **`the_args_quoting_keeps_a_value_intact`**
  ✓ (new ✓ — a JSON payload with spaces, quotes and braces survives the shell ✓ and is not re-split ✓);
  the **missing dependency** ✓ — `profile_cli_parses_and_missing_samply_fails_closed` ✓ (the missing
  profiler fails closed ✓) and `a_checkout_is_preferred_over_the_embedded_copy` ✓; the **generators
  produce compiling code** ✓ — `the_generated_authorize_matches_the_current_port_signature` ✓ (the
  guard that goes red when the template drifts ✓), `starter_template_renders_parses_and_compiles` ✓,
  `emits_three_files_with_substitutions` ✓, `writes_module_and_refuses_overwrite` ✓,
  `adds_workspace_member_idempotently` ✓, `emission_is_deterministic` ✓ and
  `generated_code_contains_no_placeholder_macros` ✓ — and now the **CI proves it for real** ✓ (the
  `scaffold-build` job ✓, applied with the user's approval ✓: a generated module is compiled and
  tested in CI ✓).
- **Gaps**: the **CLI snapshot** ✗ (no golden output of a command's stdout ✗ — the closest is
  `about_report_carries_the_golden_fields` ✓, which pins the *fields* ✓ but not a full output ✓); and
  the **interrupt** ✗ (a `Ctrl-C` mid-command is not covered ✗ — the console has no signal handling ✗).

### LF-082 — Prompts — **PARTIALLY VERIFIED**

- **Contract**: interactive terminal prompt primitives — text, select, multiselect, confirm, progress,
  validation and styling. Interactive and non-interactive modes behave correctly; validation and
  cancellation are predictable; output remains accessible in CI.
- **Implementation**: the **automation-friendly half** ✓ ships: the text prompt ✓ and the
  yes/no confirmation ✓ (`prompt_value` ✓ / `confirm` ✓) with the **non-interactive discipline** ✓
  (no TTY → no prompt ✓, the default or an explicit refusal ✓). The **rich interactive set** ✗ does
  not: no `select` ✗, no `multiselect` ✗, no `progress` bar ✗, no per-prompt **validation** ✗ and no
  **styling** ✗.
- **Evidence**: `the_noninteractive_path_never_waits_on_a_prompt` ✓ (the non-interactive mode ✓) and
  the console's other 62 cases ✓ (the output stays scriptable ✓ — the exit codes ✓ and the JSON/TOML
  switches ✓).
- **The atlas's own note is respected**: "Optional convenience; keep automation-friendly JSON/exit-code
  modes regardless of interactive UX" ✓ — the framework's console is exactly that ✓ (JSON where it
  makes sense ✓, exit codes everywhere ✓, no interactive dependency ✓), so the missing half is a
  **recorded divergence** ✓ rather than a defect ✓.
- **Gaps**: the **TTY behaviour** ✗ (no case drives a pseudo-terminal ✗), the **cancellation**
  ✗ (an aborted prompt ✗), the **invalid input** ✗ (the prompt does not validate ✗ — the name is
  validated later ✓ by the generator ✓), the **unicode** ✗ and the **terminal width** ✗ (the styling
  that would care ✗ does not exist ✓).

### LF-083 — AI SDK — **PARTIALLY VERIFIED**

- **Contract**: a unified API for model providers — prompts/responses, tools, structured output,
  streaming and orchestration. Provider settings, response schema, streaming, timeout, refusal and
  usage/cost behaviour are explicit; sensitive prompt data is controlled.
- **Implementation**: the **client** ✓ (`kelson-ai` ✓ — `AiMessage` with the roles ✓, `ToolCall` ✓,
  `ModelTool` with a JSON schema ✓, `CompletionRequest`/`CompletionResponse` ✓, the `Usage` record ✓
  — the usage/cost shape ✓, the `AiClient` trait ✓ and the **stub** provider ✓), the **tools** ✓
  (`kelson-ai-tools` ✓ — a tool registry ✓ whose descriptors are generated from the platform's own
  `Command`/`Query` contracts ✓) and the **agents** ✓ (`kelson-ai-agents` ✓ — the in-process runtime ✓
  with a **capability ceiling** ✓ and a **step budget** ✓).
- **Evidence**: the **mock provider** ✓ — `stub_plays_scripted_turns_then_echoes` ✓; the **tool call
  authorization** ✓ — `agent_cannot_exceed_its_capabilities` ✓ (an agent cannot reach a tool outside
  its grant ✓) and `unknown_tool_reports_error_without_panicking` ✓; the **budget enforcement** ✓ —
  `runaway_agent_hits_step_budget` ✓; the **tool execution** ✓ — `agent_tool_call_executes_through_engine` ✓;
  the **usage/cost** ✓ — `Usage` ✓ on the response ✓; the **response schema** ✓ — the tool
  descriptors carry a JSON schema ✓ (`tool_generated_from_command_contract` ✓,
  `tool_generated_from_query_contract` ✓); the **orchestration** ✓ — `registry_lookups_by_name` ✓ and
  the agents' runtime ✓.
- **Gaps** (all P3-scoped ✓ — the atlas rates this item **P3** and "optional and fast-moving" ✓):
  the **malformed structured output** ✗ (the model may return a payload that does not match the
  tool's schema ✗ — nothing checks it ✗); the **timeout/retry** ✗ (the client trait has no deadline
  ✓ and no retry ✓); the **prompt injection** ✗ (no case proves that injected text cannot steer the
  agent's tool choice ✗ — the capability ceiling bounds the blast radius ✓, but the case is absent ✗);
  the **streaming** ✗ (the client is request/response ✓ — no streaming ✓ and no cancellation ✓);
  and the **sensitive prompt data** ✗ (no redaction posture ✓ for what is sent to a provider ✓).
- **The atlas's own note is respected**: "Keep AI integrations outside trusted kernel, apply capability
  policy" ✓ — the AI crates sit **outside** the kernel ✓ and the agent's capability ceiling ✓ is the
  policy ✓.

### LF-084 — MCP server and tools — **PARTIALLY VERIFIED**

- **Contract**: expose application tools/resources/prompts using the MCP protocol — transport,
  registration and auth policy. Tool schemas validate, identities are authenticated, tenant scope and
  authorization are enforced, and tool calls are auditable.
- **Implementation**: the **manifest** ✓ (`kelson-ai-mcp` ✓ — `{ name, version, tools: [{ name,
  description, inputSchema }] }` ✓, generated from the platform graph ✓) and the **console's
  `mcp:tools`** ✓ (the manifest is inspectable ✓). The **transport** ✗ (there is no MCP wire
  server ✓ — the manifest is consumed in-process ✓; the crate says so ✓: "There is no parallel
  authorization story ✓ — an MCP tool call enters through the exact same `ToolSpec -> operation ->
  Engine` path ✓, so capability, scope and policy apply unchanged ✓. The manifest only describes
  what exists ✓ — it grants nothing ✓").
- **Evidence**: the **protocol shape** ✓ — `manifest_has_the_mcp_shape` ✓; the **round trip** ✓ —
  `manifest_round_trips_tool_names` ✓; the **tools follow the graph** ✓ —
  `tools_follow_the_platform_graph` ✓; the **authorization** ✓ — inherited by construction ✓ and
  proven by `agent_cannot_exceed_its_capabilities` ✓ (the same path ✓); the **versioning** ✓ — the
  manifest carries a version ✓ and round-trips it ✓.
- **Gaps**: the **invalid schema** ✗ (the manifest's permissive schema ✓ defers validation to the
  operation ✓ — correct by design ✓, but no case names it ✗); the **unauthorized tool** ✗ *as an MCP
  case* ✓ (the capability check is proven for the agent ✓, not through an MCP call ✓); the
  **cross-tenant call** ✗ (the engine's tenant scope ✓ applies ✓, but no MCP-level case names it ✓);
  the **replay** ✗, the **resource budget** ✗ and the **secret exfiltration** ✗ (no case ✓); and the
  **transport** ✗ (no wire protocol ✓ — a recorded divergence ✓).
- **The atlas's own gate is respected**: "Select only after core tool capability, auth, tenancy and
  observability contracts are secure" ✓ — those contracts are in place ✓ (the capability policy ✓,
  the tenancy ✓, the observability ✓), so the item is safe to keep ✓.

### LF-085 — Boost and agentic development — **NOT IMPLEMENTED**

Nothing ships: no Boost-style project-context tooling, no agent-assist layer. The atlas rates it
**P3** and says so itself: "Developer productivity tool rather than production runtime feature. Do
not count it in runtime behavioral parity unless comparison scope includes dev tooling" ✓ — so the
item is **out of scope for runtime parity** ✓, and the framework's own dev tooling (the codegen ✓,
the doctor ✓, the CI parity check ✓) is the nearest equivalent ✓. An absent feature has no executed
case ✓.

### LF-086 — Introspection, route list and diagnostics — **PARTIALLY VERIFIED**

- **Contract**: the CLI lists routes, config, events, providers, queues, migrations and runtime state
  where supported. Operators/devs can determine what is active, where it came from and why;
  privileged details are authenticated/redacted. Include the active generation, artifact digest and
  provenance in privileged inspect output.
- **Implementation**: the **CLI** ✓ — `route:list` ✓, `config:show` ✓, `config:dump` ✓, `env` ✓,
  `event:list` ✓, `job:list` ✓, `flag:list` ✓, `migrate:status` ✓, `schedule:list` ✓, `db:show` ✓,
  `db:inspect` ✓, `queue:monitor` ✓, `queue:failed` ✓, `mcp:tools` ✓, `pkg:list` ✓, `about` ✓,
  `doctor` ✓, `sdk:openapi` ✓ — and the **HTTP edge** ✓ (`/healthz` ✓, `/readyz` ✓, `/meta/boot` ✓,
  `/meta/flags` ✓, `/meta/routes` ✓, `/admin/modules` ✓).
- **Evidence**: the **snapshot output** ✓ — `about_report_carries_the_golden_fields` ✓ (the golden
  fields ✓) and `route_list_filters_carry_the_parity_columns` ✓; the **deterministic output** ✓ —
  `mysql_schema_dump_renders_normalized_sorted_ddl` ✓ (the DDL is normalized and sorted ✓) plus
  `queue_monitor_renders_depth_and_oldest_age` ✓ and `db_show_renders_server_metadata_and_inventory` ✓;
  the **secret redaction** ✓ — `config_show_renders_one_redacted_section` ✓ and
  `env_summary_reports_environment_secrets_and_cache_state` ✓ (the secrets are reported as *state* ✓,
  never as values ✓); the **authz** ✓ — `dashboard_served_in_test_env_and_flag_gated_when_off` ✓ (the
  diagnostics surface is flag-gated ✓) and the console's `default_operator_allowed` ✓ (no default
  operator outside dev/test ✓); the **invalid module diagnostics** ✓ (partly ✓) —
  `healthz_is_liveness_and_readyz_reflects_degraded_state` ✓ (a degraded module is reported ✓) and
  `doctor` ✓.
- **Gaps**: the **invalid module diagnostics** ✗ is only *partly* covered ✓ (a degraded state is
  reported ✓, but a **malformed** module — a bad manifest, a missing artifact — has no named case ✗);
  the **large registry performance** ✗ (no case asserts that a 10k-route listing stays fast ✓); and
  the atlas's extra — the **active generation, artifact digest and provenance** in the privileged
  inspect output ✗ (the console's `about` ✓ names the version ✓, but the per-artifact digest and the
  provenance chain are not in the inspect output ✗).

---

## 3. Findings

- **F-28 (low) — the destructive commands' environment gates were inline, so no case could name
  them — FIXED.** `migrate:fresh`, `migrate:reset`, `migrate:refresh` and `queue:clear` each
  refused outside dev/test ✓ — but the check was written inline in an async function that loads the
  config ✓, so nothing could assert it ✓ (the `seed` gate had already been extracted for exactly
  this reason ✓ — F-7 ✓). The gate is now one pure function
  (`refuse_destructive_outside_dev_test` ✓) and the case
  **`the_destructive_commands_refuse_outside_dev_and_test`** ✓ names all four ✓ — each refusal names
  its own command ✓ and the rule ✓. **The behaviour is unchanged** ✓ (the same messages ✓, the same
  environments ✓) — only the seam moved ✓.

## 4. Corrections to the parity documents

- `P0-FEATURE-VERIFICATION.md` — **five of the six items had no row at all** ✓ and the one that had
  (LF-081 ✓) rested on the claim "Console tests exist" ✓. All six rows now exist ✓ and name their
  executed cases ✓, with their gaps ✓.
- `LARAVEL-FEATURE-ATLAS-COMPLETE.md` — the LF-083 row said "EXTERNAL/ECOSYSTEM ✓ — AI crates
  (separate) ✓" ✗ while the framework **ships four AI crates** ✓ (`kelson-ai` ✓, `kelson-ai-tools` ✓,
  `kelson-ai-agents` ✓, `kelson-ai-mcp` ✓) with 13 named cases ✓; the row is corrected ✓. The
  LF-084 row said "MCP support in kelson-console ✓" ✓ — the manifest is in `kelson-ai-mcp` ✓ and the
  console only *shows* it ✓ — corrected ✓.
- `P0-EXECUTION-REPORT.md` — the batch is recorded with its finding ✓.

## 5. Summary

- **6 items**: **5 PARTIALLY VERIFIED** (LF-081 console ✓, LF-082 prompts ✓, LF-083 AI SDK ✓,
  LF-084 MCP ✓, LF-086 introspection ✓) · **1 NOT IMPLEMENTED** (LF-085 ✓ — the Boost-style dev
  tooling ✓, which the atlas itself excludes from runtime parity ✓).
- **The sharpest gaps**: the **CLI snapshot** and the **interrupt** ✗ (LF-081 ✓), the **prompt
  validation / TTY / unicode** ✗ (LF-082 ✓), the **structured output / timeout / injection /
  streaming** ✗ (LF-083 ✓ — all P3-scoped ✓), the **MCP transport** and the **replay / budget /
  exfiltration** cases ✗ (LF-084 ✓), and the **invalid-module diagnostics**, the **large-registry
  performance** and the **artifact digest / provenance** in the inspect output ✗ (LF-086 ✓).
- **Findings**: F-28 (the destructive commands' gates were untestable — **fixed** ✓, the behaviour
  unchanged ✓) and the parity rows' missing / claim-shaped evidence (**corrected** ✓ — five rows did
  not exist ✓ and the sixth rested on a claim ✓), plus the atlas's **stale disposition** for the AI
  crates ✓ (corrected ✓).
