/**
 * The release limits, in one place, because three tools enforce them and a
 * divergent number between the packager, the verifier and the test suite is how
 * a limit quietly stops being a limit.
 *
 * The package ceiling was raised three times during Phase 4 (50,000 -> 60,000 ->
 * 70,000 -> 72,000), each by owner review, each against a measurement. A fourth
 * one-off number would have meant the ceiling was no longer a constraint but an
 * apology, so from Phase 5 the policy changes shape: a frozen baseline plus a
 * stated per-phase envelope. The baseline is the previous phase's artifact as
 * shipped, re-frozen at each phase boundary from a measurement a tool printed; the
 * envelope is what the next phase is allowed to add, agreed before the work
 * starts; the ceiling is the sum. A breach is a scope conversation, not an edit to
 * this file.
 *
 *   Phase 1 36,891 -> Phase 2 43,543 -> Phase 3 51,791 -> Phase 4 70,042 -> Phase 5 94,565
 *
 * Phase 6's planning document asked for an npm package of 50 KB or less. That is
 * the pre-Phase-4 ceiling, superseded three times over, and Phase 5 alone ended
 * 44,565 bytes above it — so the number was not a constraint Phase 6 could fail,
 * it was a stale claim Phase 6 could not be measured against. The baseline and
 * envelope carry the whole history instead.
 *
 * Linear memory works the same way but in the other direction: pages are not a
 * setting, they are a consequence of the arena. The 18th page was approved for
 * Phase 5 on measured evidence — a temporary 26,216-byte arena extension moved the
 * artifact's declared minimum from 17 pages (1,114,112 B) to 18 (1,179,648 B),
 * while every Phase 5 candidate configuration from 8 to 256 capacity resources
 * fits inside that single page. `EXPECTED_PAGES` stays at 17 until the resource
 * regions actually land, so the gate fails loudly at the moment memory grows
 * rather than silently permitting it in advance.
 */

/** The Phase 4 artifact, frozen as the baseline Phase 5 measured from. */
export const PHASE_4_BASELINE_BYTES = 70_042;
/**
 * What Phase 5 added to the unpacked package.
 *
 * First agreed at 12,000. The engine milestones (M0-M5) cost 6,295 of it, and
 * the public TypeScript API (M6) cost 18,521 more, so the envelope was raised to
 * 25,000 by owner review against that measurement. Phase 5 finished 477 bytes
 * under the ceiling it met, at 94,565.
 */
export const PHASE_5_ENVELOPE_BYTES = 25_000;
/**
 * The Phase 5 artifact as shipped, frozen as the baseline Phase 6 measures from.
 *
 * Measured, not asserted: 94,565 unpacked from `scripts/build-package.mjs
 * --dry-run` at the commit that closed Phase 5. A baseline is only useful if it
 * is the number a tool printed, which is why it is not a round one.
 */
export const PHASE_5_BASELINE_BYTES = 94_565;
/**
 * What Phase 6 may add to the unpacked package.
 *
 * Agreed at 12,000 before Phase 6 started, by owner review on 2026-11-02, in the
 * same shape as Phase 5's policy. It replaces the Phase 6 planning document's
 * "npm unpacked ≤ 50 KB" target, which was the superseded pre-Phase-4 ceiling
 * (see `PREVIOUS_MAX_UNPACKED_BYTES`) and could not be met by any Phase 6 that
 * kept the Phase 4 session layer and the Phase 5 resource API.
 */
export const PHASE_6_ENVELOPE_BYTES = 12_000;
/** Unpacked npm package, measured the way npm measures it. */
export const MAX_UNPACKED_BYTES = PHASE_5_BASELINE_BYTES + PHASE_6_ENVELOPE_BYTES;
/** The published WebAssembly artifact. */
export const MAX_WASM_BYTES = 40_000;
/**
 * Linear memory the shipped engine demands. Phase 5's resource regions landed on
 * 2026-11-02 and took the 18th page approved in §1 of `docs/RESOURCE-ABI.md`:
 * the arena grew from 61,920 to 75,336 bytes, which is 13,416 bytes more than the
 * 2,264 that were spare. The wasm artifact cost 1,790 bytes (13,499 to 15,289) and
 * is still 24,711 under its budget.
 */
export const EXPECTED_PAGES = 18;
/** The page count before Phase 5, kept so the change stays visible. */
export const PREVIOUS_EXPECTED_PAGES = 17;
/** The limits before the Phase 4 reviews, kept so the changes stay visible. */
export const PREVIOUS_MAX_UNPACKED_BYTES = 50_000;
export const PREVIOUS_PHASE_4_MAX_UNPACKED_BYTES = 60_000;
export const PREVIOUS_PHASE_4B_MAX_UNPACKED_BYTES = 70_000;
export const PREVIOUS_PHASE_4C_MAX_UNPACKED_BYTES = 72_000;

export const LIMIT_REVIEW =
  "unpacked limit raised 50,000 -> 60,000 for Phase 4 (owner review, 2026-11-01, " +
  "set from a projection), then 60,000 -> 70,000 once the session layer was measured " +
  "at 68,506 bytes (owner review, 2026-11-01), then 70,000 -> 72,000 for the " +
  "checkpoint-retention fix at 70,042 bytes (owner review, 2026-11-02); from Phase 5 " +
  "the ceiling is a frozen baseline plus a per-phase envelope, owner review " +
  "2026-11-02. The Phase 5 envelope was 12,000 and was raised to 25,000 by owner " +
  "review on 2026-11-02 against the measurement: the engine milestones cost 6,295 " +
  "bytes and the public TypeScript API cost 18,521 more. Phase 5 shipped at 94,565, " +
  "and on 2026-11-03 the owner froze that as the Phase 6 baseline with a fresh " +
  "12,000-byte envelope (ceiling 106,565), rejecting the Phase 6 planning " +
  "document's 50 KB target as the superseded pre-Phase-4 number: no Phase 6 that " +
  "keeps the Phase 4 session layer and the Phase 5 resource API could meet it. " +
  "Wasm artifact, dependency and allocation limits unchanged; the 18th memory page " +
  "is approved for Phase 5 on measurement and takes effect when the resource " +
  "regions land.";
