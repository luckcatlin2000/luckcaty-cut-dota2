import type { HighlightCandidate } from "./types";
import { roundFrame } from "./frameTime.ts";
import { MAX_CLIP_DURATION_SECONDS } from "./storyDuration.ts";

interface SetupClip {
  clipId: string;
  candidateId: string;
  takeGroupId: string | null;
  durationMode: "short" | "story";
  startSeconds: number;
  endSeconds: number;
  shortStartSeconds: number;
  shortEndSeconds: number;
}

/** Explicit user action: extend selected material without restoring deleted clips. */
export function extendKillSetupClips<T extends SetupClip>(
  clips: readonly T[],
  candidates: readonly HighlightCandidate[],
): { clips: T[]; changedScenes: number } {
  const candidatesById = new Map(candidates.map((candidate) => [candidate.id, candidate]));
  const startsByScene = new Map<string, number>();
  const sceneId = (clip: T) => clip.takeGroupId ? `take:${clip.takeGroupId}` : `clip:${clip.clipId}`;
  for (const clip of clips) {
    const candidate = candidatesById.get(clip.candidateId);
    const kills = candidate?.kill_sequence?.kills;
    if (clip.durationMode !== "short" || candidate?.kind !== "hero_kill_sequence" || !kills?.length) continue;
    const deaths = kills.map((kill) => kill.death_time_seconds).filter(Number.isFinite);
    const setups = kills.map((kill) => kill.setup_time_seconds).filter(Number.isFinite);
    if (!deaths.length) continue;
    const start = roundFrame(Math.max(0, clip.endSeconds - MAX_CLIP_DURATION_SECONDS,
      Math.min(clip.startSeconds, candidate.start_seconds, Math.min(...deaths) - 12,
        setups.length ? Math.min(...setups) - 6 : clip.startSeconds)));
    const key = sceneId(clip);
    startsByScene.set(key, Math.min(startsByScene.get(key) ?? clip.startSeconds, start));
  }
  const changed = new Set<string>();
  const next = clips.map((clip) => {
    const key = sceneId(clip);
    const start = startsByScene.get(key);
    if (clip.durationMode !== "short" || start === undefined || start >= clip.startSeconds - 1 / 60) return { ...clip };
    changed.add(key);
    return { ...clip, startSeconds: start, shortStartSeconds: start };
  });
  return { clips: next, changedScenes: changed.size };
}
