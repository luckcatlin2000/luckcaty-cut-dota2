import assert from "node:assert/strict";
import test from "node:test";
import { isValidClipTimeRange, replayEndFrame } from "./frameTime.ts";

test("exact 1-second and 100-second ranges are valid at all sampled frame offsets", () => {
  for (let frame = 0; frame < 108000; frame += 17) {
    for (const duration of [30, 3000]) {
      assert.equal(isValidClipTimeRange(frame / 30, (frame + duration) / 30, 4000), true);
    }
  }
});

test("one frame outside duration bounds, nonfinite values and out-of-replay ranges are rejected", () => {
  assert.equal(isValidClipTimeRange(0, 29 / 30, 400), false);
  assert.equal(isValidClipTimeRange(0, 3001 / 30, 400), false);
  assert.equal(isValidClipTimeRange(NaN, 2, 400), false);
  assert.equal(isValidClipTimeRange(0, Infinity, 400), false);
  assert.equal(isValidClipTimeRange(-1, 2, 400), false);
  assert.equal(isValidClipTimeRange(0, 10.0333, 10.01), false);
});

test("last selectable frame remains inside a non-frame-aligned replay", () => {
  const end = replayEndFrame(600.025);
  assert.equal(end, 600);
  assert.ok(end <= 600.025);
  assert.equal(replayEndFrame(600 + 1 / 30), 600 + 1 / 30);
});
