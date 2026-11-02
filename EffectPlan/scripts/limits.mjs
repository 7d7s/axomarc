/**
 * The release limits, in one place, because three tools enforce them and a
 * divergent number between the packager, the verifier and the test suite is how
 * a limit quietly stops being a limit.
 *
 * The package ceiling was raised three times during Phase 4 (50,000 -> 60,000 ->
 * 70,000 -> 72,000), each by owner review, each against a measurement. A fourth
 * one-off number would have meant the ceiling was no longer a constraint but an
 * apology, so from Phase 5 the policy changes shape: a frozen baseline plus a
 * stated per-phase envelope. The baseline is the Phase 4 artifact as shipped; the
 * envelope is what a phase is allowed to add, agreed before the work starts; the
 * ceiling is the sum. A breach is a scope conversation, not an edit to this file.
 *
 *   Phase 1 36,891 -> Phase 2 43,543 -> Phase 3 51,791 -> Phase 4 70,042
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

/** The Phase 4 artifact, frozen as the baseline every later phase measures from. */
export const PHASE_4_BASELINE_BYTES = 70_042;
/**
 * What Phase 5 may add to the unpacked package.
 *
 * First agreed at 12,000. The engine milestones (M0-M5) cost 6,295 of it, and
 * the public TypeScript API (M6) cost 18,521 more: 94,858 unpacked against the
 * 70,042 baseline, which is 24,816 of growth against a 12,000 envelope. The
 * ceiling was raised to 25,000 by owner review against that measurement.
 */
export const PHASE_5_ENVELOPE_BYTES = 25_000;
/** Unpacked npm package, measured the way npm measures it. */
export const MAX_UNPACKED_BYTES = PHASE_4_BASELINE_BYTES + PHASE_5_ENVELOPE_BYTES;
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
  "the ceiling is the frozen Phase 4 baseline (70,042) plus a per-phase envelope, " +
  "owner review 2026-11-02. The Phase 5 envelope was 12,000 and was raised to 25,000 " +
  "by owner review on 2026-11-02 against the measurement: the engine milestones " +
  "cost 6,295 bytes and the public TypeScript API cost 18,521 more, for 94,858 " +
  "unpacked against the 70,042 baseline. Wasm artifact, dependency and allocation " +
  "limits unchanged; the 18th memory page is approved for Phase 5 on measurement " +
  "and takes effect when the resource regions land.";
