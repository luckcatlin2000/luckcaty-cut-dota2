import assert from "node:assert/strict";
import test from "node:test";
import {
  buildTimelineEvents,
  timelineLaneForCandidate,
  timelinePercent,
  timelineTicks,
} from "./eventTimeline.ts";
import type { HighlightCandidate } from "./types.ts";

function candidate(
  patch: Partial<HighlightCandidate> & Pick<HighlightCandidate, "id" | "kind">,
): HighlightCandidate {
  return {
    id: patch.id,
    rank: patch.rank ?? 1,
    kind: patch.kind,
    title: patch.title ?? patch.id,
    score: patch.score ?? 1,
    start_seconds: patch.start_seconds ?? 0,
    peak_seconds: patch.peak_seconds ?? 0,
    end_seconds: patch.end_seconds ?? 1,
    hero_deaths: patch.hero_deaths ?? 0,
    anchor_tick: patch.anchor_tick ?? 1,
    primary_hero: patch.primary_hero ?? null,
    participants: patch.participants ?? [],
    reasons: patch.reasons ?? [],
    interaction: patch.interaction,
    kill_sequence: patch.kill_sequence,
  };
}

test("maps supported candidates onto stable timeline lanes", () => {
  assert.equal(
    timelineLaneForCandidate({ kind: "hero_kill_sequence" }),
    "kills",
  );
  assert.equal(
    timelineLaneForCandidate({
      kind: "mechanical_counterplay",
      interaction: {
        pattern_id: "verified",
        occurrence_index: 1,
        occurrence_count: 1,
        trigger_name: "tree",
        response_name: "cut",
        response_delay_seconds: 0.2,
      },
    }),
    "interactions",
  );
  assert.equal(timelineLaneForCandidate({ kind: "roshan_fight" }), "objectives");
  assert.equal(timelineLaneForCandidate({ kind: "team_fight" }), "fights");
});

test("filters hero scope without losing chronological order", () => {
  const events = buildTimelineEvents(
    [
      candidate({
        id: "later-kill",
        kind: "hero_kill_sequence",
        peak_seconds: 40,
        primary_hero: "mirana",
      }),
      candidate({
        id: "other",
        kind: "hero_kill_sequence",
        peak_seconds: 10,
        primary_hero: "windrunner",
      }),
      candidate({
        id: "earlier-fight",
        kind: "team_fight",
        peak_seconds: 20,
        participants: ["mirana", "axe"],
      }),
    ],
    "mirana",
    "hero",
  );

  assert.deepEqual(
    events.map((event) => event.candidateId),
    ["earlier-fight", "later-kill"],
  );
});

test("all scope retains candidates for every hero", () => {
  const events = buildTimelineEvents(
    [
      candidate({
        id: "mirana",
        kind: "hero_kill_sequence",
        primary_hero: "mirana",
      }),
      candidate({
        id: "windrunner",
        kind: "hero_kill_sequence",
        primary_hero: "windrunner",
      }),
    ],
    "mirana",
    "all",
  );

  assert.deepEqual(
    events.map((event) => event.candidateId),
    ["mirana", "windrunner"],
  );
});

test("clamps positions and creates deterministic ticks", () => {
  assert.equal(timelinePercent(-10, 100), 0);
  assert.equal(timelinePercent(25, 100), 25);
  assert.equal(timelinePercent(120, 100), 100);
  assert.equal(timelinePercent(10, 0), 0);
  assert.deepEqual(timelineTicks(120, 4), [0, 30, 60, 90, 120]);
});
