import assert from "node:assert/strict";
import test from "node:test";
import { resumeAction } from "./renderResume.ts";
import type { RenderResumeSummary } from "./types.ts";

const saved: RenderResumeSummary = {
  fingerprint: "verified-plan", totalClips: 25, completedClips: 22, remainingClips: 3,
  firstPendingClip: 23, unverifiedCameraClips: 22, completedDirectory: "saved/segments",
};
test("camera uncertainty keeps the explicit resume action available", () => {
  assert.equal(resumeAction(saved)?.button, "从第 23 段继续");
  assert.equal(resumeAction(saved)?.title, "已完成 22 段 · 剩余 3 段");
});
test("a missing clip in the middle uses its real index instead of completed count plus one", () => {
  assert.equal(resumeAction({ ...saved, firstPendingClip: 8 })?.button, "从第 8 段继续");
});
test("completed capture offers composition without a game launch", () => {
  const action = resumeAction({ ...saved, completedClips: 25, remainingClips: 0, firstPendingClip: null });
  assert.equal(action?.button, "继续合成");
  assert.match(action?.description ?? "", /无需启动 Dota 2/);
});
test("empty or inconsistent checkpoint inspection cannot enable resume", () => {
  assert.equal(resumeAction(null), null);
  for (const change of [{ completedClips: 0 }, { fingerprint: "" }, { remainingClips: -1 },
    { firstPendingClip: null }, { firstPendingClip: 26 }, { totalClips: 24 }]) {
    assert.equal(resumeAction({ ...saved, ...change }), null);
  }
});
