import type { HighlightCandidate } from "./types";

export type TimelineScope = "all" | "hero";
export type TimelineLaneId =
  | "kills"
  | "interactions"
  | "fights"
  | "objectives";

export interface TimelineLane {
  id: TimelineLaneId;
  label: string;
}

export interface TimelineEvent {
  candidateId: string;
  lane: TimelineLaneId;
  timeSeconds: number;
  startSeconds: number;
  endSeconds: number;
}

export const TIMELINE_LANES: readonly TimelineLane[] = [
  { id: "kills", label: "击杀" },
  { id: "interactions", label: "技巧" },
  { id: "fights", label: "团战" },
  { id: "objectives", label: "目标" },
] as const;

export function buildTimelineEvents(
  candidates: readonly HighlightCandidate[],
  hero: string,
  scope: TimelineScope,
): TimelineEvent[] {
  return candidates
    .filter(
      (candidate) =>
        scope === "all" || !hero || candidateIncludesHero(candidate, hero),
    )
    .map((candidate) => ({
      candidateId: candidate.id,
      lane: timelineLaneForCandidate(candidate),
      timeSeconds: finiteTime(candidate.peak_seconds),
      startSeconds: finiteTime(candidate.start_seconds),
      endSeconds: finiteTime(candidate.end_seconds),
    }))
    .sort(
      (left, right) =>
        left.timeSeconds - right.timeSeconds ||
        left.candidateId.localeCompare(right.candidateId),
    );
}

export function timelineLaneForCandidate(
  candidate: Pick<HighlightCandidate, "kind" | "interaction">,
): TimelineLaneId {
  if (candidate.kind === "hero_kill_sequence") {
    return "kills";
  }
  if (candidate.interaction) {
    return "interactions";
  }
  if (
    candidate.kind === "objective" ||
    candidate.kind.includes("roshan") ||
    candidate.kind.includes("tower") ||
    candidate.kind.includes("barracks")
  ) {
    return "objectives";
  }
  return "fights";
}

export function timelinePercent(timeSeconds: number, durationSeconds: number) {
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) {
    return 0;
  }
  const safeTime = Number.isFinite(timeSeconds) ? timeSeconds : 0;
  return Math.min(100, Math.max(0, (safeTime / durationSeconds) * 100));
}

export function timelineTicks(durationSeconds: number, divisions = 4) {
  const safeDivisions = Math.max(1, Math.floor(divisions));
  const duration =
    Number.isFinite(durationSeconds) && durationSeconds > 0
      ? durationSeconds
      : 0;
  return Array.from(
    { length: safeDivisions + 1 },
    (_, index) => (duration * index) / safeDivisions,
  );
}

function candidateIncludesHero(candidate: HighlightCandidate, hero: string) {
  return (
    candidate.primary_hero === hero ||
    candidate.kill_sequence?.hero === hero ||
    candidate.participants.includes(hero)
  );
}

function finiteTime(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}
