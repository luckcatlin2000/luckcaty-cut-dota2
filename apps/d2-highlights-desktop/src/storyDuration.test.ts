import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_STORY_PRE_ROLL_SECONDS,
  MAX_CLIP_DURATION_SECONDS,
  STORY_POST_ROLL_SECONDS,
  killSequenceRange,
  normalizeStoryDurationClips,
} from "./storyDuration.ts";
import type { HighlightCandidate } from "./types.ts";

function killCandidate(
  id: string,
  hero: string,
  deathTimes: number[],
): HighlightCandidate {
  return {
    id,
    rank: 1,
    kind: "hero_kill_sequence",
    title: `${hero} kill sequence`,
    start_seconds: deathTimes[0]! - 2,
    peak_seconds: deathTimes.at(-1)!,
    end_seconds: deathTimes.at(-1)! + 2,
    anchor_tick: 0,
    primary_hero: hero,
    participants: [hero],
    hero_deaths: deathTimes.length,
    score: 1,
    reasons: [],
    kill_sequence: {
      hero,
      sequence_index: 1,
      sequence_count: 1,
      total_kills: deathTimes.length,
      kills: deathTimes.map((deathTime, index) => ({
        death_tick: index,
        death_time_seconds: deathTime,
        target_hero: `target-${index}`,
        setup_tick: index,
        setup_time_seconds: deathTime - 2,
      })),
    },
  };
}

function storyClip(clipId: string, candidateId: string, hero: string) {
  return {
    clipId,
    candidateId,
    viewHero: hero,
    takeGroupId: null,
    includeInFinal: true,
    durationMode: "story" as const,
    storyPreRollSeconds: DEFAULT_STORY_PRE_ROLL_SECONDS,
    startSeconds: 0,
    endSeconds: 1,
  };
}

test("story range uses the first and final kill with a fixed post-roll", () => {
  const range = killSequenceRange(
    killCandidate("kill-1", "mirana", [120, 126]),
    DEFAULT_STORY_PRE_ROLL_SECONDS,
    600,
  );

  assert.ok(range);
  assert.equal(range.startSeconds, 60);
  assert.equal(range.endSeconds, 126 + STORY_POST_ROLL_SECONDS);
  assert.equal(range.actualPreRollSeconds, 60);
});

test("story range respects replay boundaries and the 100 second contract", () => {
  const opening = killSequenceRange(
    killCandidate("opening", "mirana", [20]),
    90,
    600,
  );
  const longSequence = killSequenceRange(
    killCandidate("long", "mirana", [200, 230]),
    90,
    600,
  );
  const ending = killSequenceRange(
    killCandidate("ending", "mirana", [595]),
    60,
    600,
  );

  assert.deepEqual(
    [opening?.startSeconds, opening?.endSeconds],
    [0, 30],
  );
  assert.equal(
    (longSequence?.endSeconds ?? 0) - (longSequence?.startSeconds ?? 0),
    MAX_CLIP_DURATION_SECONDS,
  );
  assert.equal(ending?.endSeconds, 600);
});

test("overlapping story clips for the same hero do not duplicate replay time", () => {
  const first = killCandidate("kill-1", "mirana", [100]);
  const second = killCandidate("kill-2", "mirana", [150]);
  const normalized = normalizeStoryDurationClips(
    [
      storyClip("clip-1", first.id, "mirana"),
      storyClip("clip-2", second.id, "mirana"),
    ],
    [first, second],
    600,
  );

  assert.deepEqual(
    normalized.map((clip) => [clip.startSeconds, clip.endSeconds]),
    [
      [40, 110],
      [110, 160],
    ],
  );
});

test("take-group cameras keep identical story timecodes", () => {
  const candidate = killCandidate("kill-1", "mirana", [100]);
  const primary = {
    ...storyClip("clip-a", candidate.id, "mirana"),
    takeGroupId: "S001",
  };
  const alternate = {
    ...storyClip("clip-b", candidate.id, "mirana"),
    takeGroupId: "S001",
    includeInFinal: false,
  };
  const normalized = normalizeStoryDurationClips(
    [primary, alternate],
    [candidate],
    600,
  );

  assert.equal(normalized[0]?.startSeconds, normalized[1]?.startSeconds);
  assert.equal(normalized[0]?.endSeconds, normalized[1]?.endSeconds);
});

test("different heroes keep independent story ranges", () => {
  const mirana = killCandidate("kill-1", "mirana", [100]);
  const windrunner = killCandidate("kill-2", "windrunner", [105]);
  const normalized = normalizeStoryDurationClips(
    [
      storyClip("clip-1", mirana.id, "mirana"),
      storyClip("clip-2", windrunner.id, "windrunner"),
    ],
    [mirana, windrunner],
    600,
  );

  assert.equal(normalized[0]?.startSeconds, 40);
  assert.equal(normalized[1]?.startSeconds, 45);
});
