import assert from "node:assert/strict";
import test from "node:test";
import { DraftWriter, EditorOperationGate } from "./editorSession.ts";

test("latest complete dual draft is persisted after an older delayed write", async () => {
  let release!: () => void;
  const delayed = new Promise<void>((resolve) => { release = resolve; });
  const written: unknown[] = [];
  const writer = new DraftWriter(async (request: { jobId: string; plans: number[][] }) => {
    if (written.length === 0) await delayed;
    written.push(request);
    return written.length;
  });
  const old = { jobId: "A", plans: [[1, 2], [3]] };
  const first = writer.persist(old);
  old.plans[0]!.push(999); // mutation after enqueue must not alter the saved snapshot
  const latest = { jobId: "A", plans: [[], [3, 4]] };
  const beforeSwitch = writer.flush(latest);
  release();
  await Promise.all([first, beforeSwitch]);
  assert.deepEqual(written, [{ jobId: "A", plans: [[1, 2], [3]] }, latest]);
  await writer.flush(latest);
  assert.equal(written.length, 2, "a repeated flush must not rewrite an unchanged draft");
});

test("failed save rejects a switch/close flush and remains retryable", async () => {
  let fail = true;
  const writer = new DraftWriter(async (value: number) => {
    if (fail) throw new Error("disk full");
    return value;
  });
  await assert.rejects(writer.flush(2), /disk full/);
  fail = false;
  assert.equal(await writer.flush(2), 2);
});

test("reanalysis recreates an archived plan even if its new contents are unchanged", async () => {
  let writes = 0;
  const writer = new DraftWriter(async (_request: number) => ++writes);
  await writer.persist(1);
  writer.invalidateSavedSnapshot();
  await writer.persist(1);
  assert.equal(writes, 2);
});

test("update, render and analysis are mutually exclusive in either start order", () => {
  for (const first of ["analysis", "render", "update"] as const) {
    const gate = new EditorOperationGate();
    const token = gate.begin(first)!;
    for (const second of ["analysis", "render", "update"] as const) {
      assert.equal(gate.begin(second), null);
    }
    gate.finish(token);
    assert.ok(gate.begin("analysis"));
  }
});

test("late callbacks cannot claim a newer operation or a closing editor", () => {
  const gate = new EditorOperationGate();
  const old = gate.begin("analysis")!;
  gate.finish(old);
  const current = gate.begin("render")!;
  assert.equal(gate.current(old), false);
  gate.finish(old);
  assert.equal(gate.current(current), true);
  gate.setClosing(true);
  assert.equal(gate.current(current), false);
  assert.equal(gate.begin("analysis"), null);
  gate.setClosing(false);
  assert.equal(gate.current(current), true);
});
