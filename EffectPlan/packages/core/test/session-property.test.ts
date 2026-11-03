/**
 * The property the whole phase rests on: an incremental plan equals a fresh
 * rebuild of the same batch.
 *
 * Each run builds a session, applies a random sequence of edits, and compares
 * the session's plan with a plan of the same batch built from scratch by the
 * stateless API. The comparison is on the dependency structure, the waves and
 * the schedule: the resource *numbers* differ on purpose, because a session
 * freezes its interning table and a fresh plan numbers by first appearance.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import fc from "fast-check";

import { EffectPlan } from "../src/index.js";
import type { PlanChange, ToolCall } from "../src/index.js";
import { wasmBytes } from "./support.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

const MODES = ["read", "write"] as const;
const RESOURCES = ["f0", "f1", "f2"];

/**
 * One call: an id, one or two accesses, and a duration.
 *
 * The id is deliberately not part of what the engine sees, so a call rebuilt
 * with the same seed declares the same dependencies under a different name.
 * That is what makes a "retime" edit possible: the same dependencies, a
 * different clock, and therefore a change the dependency rules cannot see.
 */
function makeCall(id: string, seed: number): { call: ToolCall; duration: number } {
  const first = RESOURCES[seed % RESOURCES.length]!;
  const second = RESOURCES[(seed + 1) % RESOURCES.length]!;
  const accesses =
    seed % 3 === 0
      ? [{ resource: { namespace: "fs", id: first }, mode: MODES[seed % 2]! }]
      : [
          { resource: { namespace: "fs", id: first }, mode: MODES[seed % 2]! },
          { resource: { namespace: "fs", id: second }, mode: MODES[(seed + 1) % 2]! },
        ];
  return {
    call: { id, effect: { version: 1, mode: "resources", accesses } },
    duration: 1 + (seed % 5),
  };
}

interface Entry {
  readonly key: string;
  readonly call: ToolCall;
  readonly duration: number;
}

/** The batch a session should hold after the edits, tracked beside it. */
function applyEdits(entries: Entry[], changes: readonly PlanChange[]): Entry[] {
  const next = [...entries];
  for (const change of changes) {
    if (change.type === "append") {
      next.push({
        key: change.key,
        call: change.call,
        duration: change.duration ?? 1,
      });
      continue;
    }
    const at = next.findIndex((entry) => entry.key === change.key);
    if (at < 0) continue;
    if (change.type === "remove") {
      next.splice(at, 1);
      continue;
    }
    next[at] = { key: change.key, call: change.call, duration: change.duration ?? 1 };
  }
  return next;
}

/** The parts of two plans that must be identical. */
function comparable(plan: ReturnType<EffectPlan["planBatch"]>) {
  return {
    edges: plan.edges.map((edge) => [edge.from, edge.to] as const),
    levels: [...plan.execution.levelByCall],
    makespan: plan.timing?.criticalPathDuration ?? null,
    peak: plan.timing?.peakConcurrency ?? null,
    work: plan.timing?.totalWork ?? null,
    critical: [...(plan.timing?.criticalEvents ?? [])],
  };
}

describe("incremental planning properties", () => {
  it("matches a fresh rebuild after any sequence of edits", async () => {
    const planner = await open();
    // A second engine for the fresh rebuild: a session and a stateless batch on
    // the same instance would fight over the arena.
    const freshPlanner = await open();
    await fc.assert(
      fc.asyncProperty(
        fc.array(fc.integer({ min: 0, max: 64 }), { minLength: 1, maxLength: 8 }),
        fc.array(
          fc.record({
            kind: fc.constantFrom("append", "replace", "remove", "retime"),
            at: fc.integer({ min: 0, max: 40 }),
            seed: fc.integer({ min: 0, max: 64 }),
          }),
          { minLength: 1, maxLength: 6 },
        ),
        async (seeds, edits) => {
          let counter = 0;
          const entries: Entry[] = seeds.map((seed, index) => {
            const made = makeCall(`c${index}`, seed);
            return { key: `k${index}`, call: made.call, duration: made.duration };
          });
          const session = planner.createSession(
            entries.map((entry) => ({
              key: entry.key,
              call: entry.call,
              duration: entry.duration,
            })),
            { timing: { unit: "ms" }, checkpointInterval: 3 },
          );

          const changes: PlanChange[] = [];
          let model = entries;
          for (const edit of edits) {
            counter++;
            const key = `k${edit.at}`;
            if (edit.kind === "remove") {
              if (!model.some((entry) => entry.key === key)) continue;
              changes.push({ type: "remove", key });
            } else if (edit.kind === "replace") {
              if (!model.some((entry) => entry.key === key)) continue;
              const made = makeCall(`c${edit.at}-${counter}`, edit.seed);
              changes.push({ type: "replace", key, call: made.call, duration: made.duration });
            } else if (edit.kind === "retime") {
              // The same call, the same dependencies, a different duration. The
              // dependency graph cannot have changed, so the session is allowed
              // to reuse it whole - and the rebuild below is what says whether
              // that was allowed for the right reason.
              const existing = model.find((entry) => entry.key === key);
              if (!existing) continue;
              const duration = 1 + ((edit.seed % 7) + 1);
              changes.push({ type: "replace", key, call: existing.call, duration });
            } else {
              // An appended key is always fresh: the session refuses to reuse one.
              const made = makeCall(`n${counter}`, edit.seed);
              changes.push({
                type: "append",
                key: `n${counter}`,
                call: made.call,
                duration: made.duration,
              });
            }
            model = applyEdits(model, changes.slice(-1));
            const applied = session.applyChanges({
              expectedVersion: session.current().version,
              changes: changes.slice(-1),
            });
            if (edit.kind === "retime") {
              // The edit is only interesting if it really took the scheduling
              // path: a test that silently stopped exercising the reuse rule
              // would still pass.
              assert.equal(applied.update.invalidated, "scheduling");
              assert.equal(applied.update.reusedPrefixCalls, applied.callCount);
            }
          }

          const fresh = freshPlanner.planBatch(
            model.map((entry) => entry.call),
            { timing: { durations: model.map((entry) => entry.duration) } },
          );
          assert.deepEqual(comparable(session.current().plan), comparable(fresh));
        },
      ),
      { numRuns: 60 },
    );
  });

  it("never lets a version be reused or a stale update through", async () => {
    const planner = await open();
    const made = makeCall("c0", 1);
    const session = planner.createSession(
      [{ key: "k0", call: made.call, duration: made.duration }],
      { timing: { unit: "ms" } },
    );
    const version = session.current().version;
    const other = makeCall("c1", 2);
    session.applyChanges({
      expectedVersion: version,
      changes: [{ type: "append", key: "k1", call: other.call, duration: other.duration }],
    });
    assert.equal(session.current().version, version + 1);
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: version,
          changes: [{ type: "append", key: "k2", call: other.call, duration: other.duration }],
        }),
      /EP_STALE_VERSION/,
    );
  });
});
