/**
 * The release limits, in one place, because three tools enforce them and a
 * divergent number between the packager, the verifier and the test suite is how
 * a limit quietly stops being a limit.
 *
 * The package limit was reviewed and raised for Phase 4 on 2026-11-01, at the
 * owner's decision, before any of the incremental-planning TypeScript was
 * written. It was not fitted to a measurement: the projection at the time of the
 * review was roughly 57,000 bytes (51,791 with the resumable engine and no
 * session layer, plus an estimated session module) and the new ceiling was set
 * above that with room to spare. Everything else from the Phase 1 brief is
 * unchanged — the 40,000-byte artifact, zero runtime dependencies, zero Rust
 * heap allocation, and 17 pages of linear memory, which Phase 4 was designed
 * around rather than against.
 */

/** Unpacked npm package, measured the way npm measures it. */
export const MAX_UNPACKED_BYTES = 70_000;
/** The published WebAssembly artifact. */
export const MAX_WASM_BYTES = 40_000;
/** Linear memory the engine may demand at run time. */
export const MAX_PAGES = 17;
/** The limits before the Phase 4 reviews, kept so the changes stay visible. */
export const PREVIOUS_MAX_UNPACKED_BYTES = 50_000;
export const PREVIOUS_PHASE_4_MAX_UNPACKED_BYTES = 60_000;

export const LIMIT_REVIEW =
  "unpacked limit raised 50,000 -> 60,000 for Phase 4 (owner review, 2026-11-01, " +
  "set from a projection), then 60,000 -> 70,000 once the session layer was measured " +
  "at 68,506 bytes (owner review, 2026-11-01); wasm, dependency, allocation and " +
  "page limits unchanged";
