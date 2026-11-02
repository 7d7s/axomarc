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
/** What Phase 5 is allowed to add to the unpacked package. */
export const PHASE_5_ENVELOPE_BYTES = 12_000;
/** Unpacked npm package, measured the way npm measures it. */
export const MAX_UNPACKED_BYTES = PHASE_4_BASELINE_BYTES + PHASE_5_ENVELOPE_BYTES;
/** The published WebAssembly artifact. */
export const MAX_WASM_BYTES = 40_000;
/** Linear memory the shipped engine demands today. Bump only with the change that causes it. */
export const EXPECTED_PAGES = 17;
/** Approved for Phase 5 on the measurement above; not yet in force. */
export const PHASE_5_PAGES_APPROVED = 18;
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
  "the ceiling is the frozen Phase 4 baseline (70,042) plus a per-phase envelope " +
  "(+12,000), owner review 2026-11-02. Wasm artifact, dependency and allocation " +
  "limits unchanged; the 18th memory page is approved for Phase 5 on measurement " +
  "and takes effect when the resource regions land.";
