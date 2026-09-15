import assert from "node:assert/strict";
import test from "node:test";
import { extendKillSetupClips } from "./extendKillSetup.ts";
import type { HighlightCandidate } from "./types";

const candidate = {
  id: "kill", kind: "hero_kill_sequence", start_seconds: 98.5,
  kill_sequence: { kills: [{ death_time_seconds: 100, setup_time_seconds: 99.5 }] },
} as HighlightCandidate;
const clip = {
  clipId: "a", candidateId: "kill", takeGroupId: null, durationMode: "short" as const,
  startSeconds: 98.5, endSeconds: 103, shortStartSeconds: 98.5, shortEndSeconds: 103,
};

test("old selected clips gain setup without restoring removed clips or changing the end", () => {
  const result = extendKillSetupClips([clip], [candidate]);
  assert.equal(result.clips.length, 1);
  assert.equal(result.clips[0]?.startSeconds, 88);
  assert.equal(result.clips[0]?.shortStartSeconds, 88);
  assert.equal(result.clips[0]?.endSeconds, 103);
  assert.equal(result.changedScenes, 1);
  assert.equal(clip.startSeconds, 98.5);
  assert.equal(extendKillSetupClips(result.clips, [candidate]).changedScenes, 0);
});

test("synchronized takes keep the same setup time and count as one scene", () => {
  const a = { ...clip, takeGroupId: "scene" };
  const b = { ...a, clipId: "b" };
  const result = extendKillSetupClips([a,b], [candidate]);
  assert.deepEqual(result.clips.map((item) => item.startSeconds), [88,88]);
  assert.equal(result.changedScenes, 1);
});

test("story mode, unknown candidates and already longer manual lead-ins are retained", () => {
  const story = { ...clip, durationMode: "story" as const, startSeconds: 40 };
  const custom = { ...clip, clipId: "custom", candidateId: "missing" };
  const longer = { ...clip, clipId: "long", startSeconds: 70, shortStartSeconds: 70 };
  const input = [story,custom,longer];
  assert.deepEqual(extendKillSetupClips(input, [candidate]).clips, input);
});

test("setup extension respects the replay start and 100-second duration bound", () => {
  const opening = { ...candidate, start_seconds: 0, kill_sequence: { kills: [{ death_time_seconds: 5, setup_time_seconds: 4 }] } } as HighlightCandidate;
  assert.equal(extendKillSetupClips([{ ...clip, startSeconds: 3, endSeconds: 8 }], [opening]).clips[0]?.startSeconds, 0);
  const long = { ...clip, startSeconds: 95, endSeconds: 195 };
  assert.equal(extendKillSetupClips([long], [candidate]).clips[0]?.startSeconds, 95);
});
