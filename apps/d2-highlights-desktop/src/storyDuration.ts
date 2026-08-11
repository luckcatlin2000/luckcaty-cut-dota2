import type { ClipDurationMode, HighlightCandidate } from "./types";

export const DEFAULT_STORY_PRE_ROLL_SECONDS = 60;
export const MAX_STORY_PRE_ROLL_SECONDS = 90;
export const STORY_POST_ROLL_SECONDS = 10;
export const MAX_CLIP_DURATION_SECONDS = 100;

export interface StoryDurationClip {
  clipId: string;
  candidateId: string;
  viewHero: string;
  takeGroupId: string | null;
  includeInFinal: boolean;
  durationMode: ClipDurationMode;
  storyPreRollSeconds: number;
  startSeconds: number;
  endSeconds: number;
}

export interface StoryRange {
  startSeconds: number;
  endSeconds: number;
  firstKillSeconds: number;
  lastKillSeconds: number;
  actualPreRollSeconds: number;
}

export function killSequenceRange(
  candidate: HighlightCandidate,
  requestedPreRollSeconds: number,
  replayDurationSeconds: number,
): StoryRange | null {
  const kills = candidate.kill_sequence?.kills;
  if (candidate.kind !== "hero_kill_sequence" || !kills?.length) {
    return null;
  }

  const orderedDeaths = kills
    .map((kill) => kill.death_time_seconds)
    .filter(Number.isFinite)
    .sort((left, right) => left - right);
  const firstKillSeconds = orderedDeaths[0];
  const lastKillSeconds = orderedDeaths.at(-1);
  if (firstKillSeconds === undefined || lastKillSeconds === undefined) {
    return null;
  }

  const replayEnd = Math.max(0, replayDurationSeconds);
  const preRoll = clamp(
    requestedPreRollSeconds,
    0,
    MAX_STORY_PRE_ROLL_SECONDS,
  );
  const endSeconds = roundFrame(
    clamp(lastKillSeconds + STORY_POST_ROLL_SECONDS, 0, replayEnd),
  );
  const desiredStart = clamp(firstKillSeconds - preRoll, 0, endSeconds);
  const startSeconds = roundFrame(
    Math.max(desiredStart, endSeconds - MAX_CLIP_DURATION_SECONDS),
  );

  return {
    startSeconds,
    endSeconds,
    firstKillSeconds,
    lastKillSeconds,
    actualPreRollSeconds: Math.max(0, firstKillSeconds - startSeconds),
  };
}

export function normalizeStoryDurationClips<T extends StoryDurationClip>(
  clips: readonly T[],
  candidates: readonly HighlightCandidate[],
  replayDurationSeconds: number,
): T[] {
  const next = clips.map((clip) => ({ ...clip }));
  const candidateById = new Map(
    candidates.map((candidate) => [candidate.id, candidate]),
  );
  const sceneById = new Map<string, number[]>();

  next.forEach((clip, index) => {
    const sceneId = clip.takeGroupId
      ? `take:${clip.takeGroupId}`
      : `clip:${clip.clipId}`;
    const indexes = sceneById.get(sceneId) ?? [];
    indexes.push(index);
    sceneById.set(sceneId, indexes);
  });

  const scenes = [...sceneById.entries()]
    .map(([sceneId, indexes]) => {
      const representative = next[indexes[0]!];
      const candidate = representative
        ? candidateById.get(representative.candidateId)
        : undefined;
      const range =
        representative?.durationMode === "story" && candidate
          ? killSequenceRange(
              candidate,
              representative.storyPreRollSeconds,
              replayDurationSeconds,
            )
          : null;
      return {
        sceneId,
        indexes,
        candidate,
        range,
        hero: candidate?.kill_sequence?.hero ?? representative?.viewHero ?? "",
        includeInFinal: indexes.some(
          (index) => next[index]?.includeInFinal ?? false,
        ),
      };
    })
    .filter(
      (scene): scene is typeof scene & { range: StoryRange } =>
        Boolean(scene.range && scene.includeInFinal),
    )
    .sort(
      (left, right) =>
        left.range.firstKillSeconds - right.range.firstKillSeconds ||
        left.sceneId.localeCompare(right.sceneId),
    );

  const previousEndByHero = new Map<string, number>();
  for (const scene of scenes) {
    const previousEnd = previousEndByHero.get(scene.hero) ?? 0;
    const latestValidStart = Math.max(0, scene.range.endSeconds - 1);
    const startSeconds = roundFrame(
      Math.min(
        latestValidStart,
        Math.max(scene.range.startSeconds, previousEnd),
      ),
    );
    for (const index of scene.indexes) {
      const clip = next[index];
      if (clip) {
        clip.startSeconds = startSeconds;
        clip.endSeconds = scene.range.endSeconds;
      }
    }
    previousEndByHero.set(
      scene.hero,
      Math.max(previousEnd, scene.range.endSeconds),
    );
  }

  return next;
}

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}

function roundFrame(value: number) {
  return Math.round(value * 30) / 30;
}
