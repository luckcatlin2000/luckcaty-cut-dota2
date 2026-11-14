import type { AnalysisSummary, HighlightStory, StoryCategory, StoryShot } from "./types.ts";

export type WtfCategoryFilter = "all" | "comedy" | "skill" | "mistake";
export type WtfCameraMode = "player_perspective" | "hero_chase";

export interface WtfClipInput {
  clipId: string;
  storyId: string;
  shotId: string;
  hero: string;
  startSeconds: number;
  endSeconds: number;
  cameraMode: WtfCameraMode;
  takeGroupId: string;
  takeRole: "primary" | "alternate";
  includeInFinal: boolean;
}

export interface WtfDraft {
  jobId: string;
  sourceSha256: string;
  clips: WtfClipInput[];
  updatedUnixSeconds: number;
}

export interface WtfPlanExport { planPath: string; clipCount: number }
export interface WtfHandoffReceipt {
  sessionDirectory: string;
  recordingJobId: string;
  clipCount: number;
  launched: boolean;
  message: string;
}

export const WTF_CATEGORY_LABELS: Record<StoryCategory, string> = {
  comedy: "趣味", skill: "精彩", mistake: "失误", fight: "团战", objective: "目标争夺",
};

export function filterWtfStories(stories: HighlightStory[], category: WtfCategoryFilter, hero = ""): HighlightStory[] {
  return stories.filter(story => (category === "all" || story.category === category) &&
    (!hero || story.primary_hero === hero || story.participants.some(person => person.hero === hero) ||
      story.shots.some(shot => shot.target_hero === hero || shot.fallback.target_hero === hero)));
}

export function wtfSceneGroups(clips: WtfClipInput[]): WtfClipInput[][] {
  const groups = new Map<string, WtfClipInput[]>();
  for (const clip of clips) {
    const group = groups.get(clip.takeGroupId) ?? [];
    group.push(clip);
    groups.set(clip.takeGroupId, group);
  }
  return Array.from(groups.values(), group => [...group].sort((a, b) =>
    Number(a.takeRole === "alternate") - Number(b.takeRole === "alternate")));
}

function editableCamera(shot: StoryShot): WtfCameraMode {
  if (shot.camera_mode === "player_perspective" || shot.camera_mode === "hero_chase") return shot.camera_mode;
  return shot.fallback.camera_mode === "hero_chase" ? "hero_chase" : "player_perspective";
}

export function wtfTimeError(start: number, end: number, replayEnd: number): string | null {
  if (![start, end, replayEnd].every(Number.isFinite) || start < 0 || end > replayEnd) return "起止时间必须位于录像范围内。";
  const duration = end - start;
  if (duration < 1 - 0.0001 || duration > 100 + 0.0001) return "每个场次须为 1 到 100 秒。";
  return null;
}

export function validateWtfClips(clips: WtfClipInput[], replayEnd: number): void {
  const clipIds = new Set<string>();
  const shotIds = new Set<string>();
  for (const clip of clips) {
    if (!/^[A-Za-z0-9_.-]{1,256}$/.test(clip.clipId) || !/^[A-Za-z0-9_.-]{1,256}$/.test(clip.takeGroupId))
      throw new Error("素材或场次标识无效，原始方案已保留。") ;
    if (clipIds.has(clip.clipId) || shotIds.has(`${clip.storyId}\n${clip.shotId}`)) throw new Error("同一故事镜头不能重复加入清单。") ;
    clipIds.add(clip.clipId); shotIds.add(`${clip.storyId}\n${clip.shotId}`);
    if (!clip.hero || !["player_perspective", "hero_chase"].includes(clip.cameraMode)) throw new Error("请为每个机位选择英雄与镜头。") ;
    const timeError = wtfTimeError(clip.startSeconds, clip.endSeconds, replayEnd);
    if (timeError) throw new Error(timeError);
    if (clip.includeInFinal !== (clip.takeRole === "primary")) throw new Error("主机位须入片，备用机位仅保存素材。") ;
  }
  for (const group of wtfSceneGroups(clips)) {
    const first = group[0]!;
    if (group.filter(clip => clip.takeRole === "primary").length !== 1) throw new Error("每个场次必须恰有一个主机位。") ;
    if (group.some(clip => clip.storyId !== first.storyId || Math.abs(clip.startSeconds - first.startSeconds) > 0.0001 ||
      Math.abs(clip.endSeconds - first.endSeconds) > 0.0001)) throw new Error("同场次的所有机位必须使用相同时间窗。") ;
  }
}

export function adoptWtfStory(story: HighlightStory, current: WtfClipInput[], replayEnd: number): WtfClipInput[] {
  const existingShots = new Set(current.filter(clip => clip.storyId === story.id).map(clip => clip.shotId));
  const shots = [...story.shots].sort((a, b) => a.order - b.order);
  if (shots.length === 0) throw new Error("这个故事尚无可交接的镜头方案，请先核对证据。") ;
  const added = shots.filter(shot => !existingShots.has(shot.id)).map(shot => ({
    clipId: `wtf-${story.id}-${shot.id}`, storyId: story.id, shotId: shot.id,
    hero: shot.target_hero ?? shot.fallback.target_hero ?? story.primary_hero,
    startSeconds: shot.source_start_seconds, endSeconds: shot.source_end_seconds,
    cameraMode: editableCamera(shot), takeGroupId: `wtf-${story.id}-${shot.take_group_id}`,
    takeRole: shot.take_role, includeInFinal: shot.take_role === "primary",
  }));
  const result = wtfSceneGroups([...current, ...added]).flat();
  validateWtfClips(result, replayEnd);
  return result;
}

export function editWtfSceneTime(clips: WtfClipInput[], groupId: string, start: number, end: number, replayEnd: number): WtfClipInput[] {
  const timeError = wtfTimeError(start, end, replayEnd);
  if (timeError) throw new Error(timeError);
  return clips.map(clip => clip.takeGroupId === groupId ? { ...clip, startSeconds: start, endSeconds: end } : clip);
}

export function moveWtfScene(clips: WtfClipInput[], groupId: string, direction: -1 | 1): WtfClipInput[] {
  const groups = wtfSceneGroups(clips);
  const index = groups.findIndex(group => group[0]?.takeGroupId === groupId);
  const next = index + direction;
  if (index < 0 || next < 0 || next >= groups.length) return clips;
  [groups[index], groups[next]] = [groups[next]!, groups[index]!];
  return groups.flat();
}

export function wtfDefaultDuration(clips: WtfClipInput[]): number {
  return wtfSceneGroups(clips).reduce((sum, group) => {
    const primary = group.find(clip => clip.takeRole === "primary" && clip.includeInFinal);
    return sum + (primary ? primary.endSeconds - primary.startSeconds : 0);
  }, 0);
}

export function validateWtfDraft(summary: AnalysisSummary, draft: WtfDraft): WtfClipInput[] {
  if (draft.jobId !== summary.job_id || draft.sourceSha256 !== summary.source.sha256) throw new Error("草稿与当前录像不匹配，尚未覆盖原文件。") ;
  const stories = new Map(summary.stories.stories.map(story => [story.id, story]));
  for (const clip of draft.clips) {
    const shot = stories.get(clip.storyId)?.shots.find(item => item.id === clip.shotId);
    if (!shot || clip.takeGroupId !== `wtf-${clip.storyId}-${shot.take_group_id}` || clip.takeRole !== shot.take_role)
      throw new Error("草稿引用的故事或镜头已变化，原草稿已保留，请先核对来源。") ;
  }
  validateWtfClips(draft.clips, summary.replay.playback_time_seconds);
  return structuredClone(draft.clips);
}

// Save snapshots serially so a slow earlier write cannot replace the latest edit.
export class WtfDraftSequence {
  private tail: Promise<void> = Promise.resolve();
  private savedSignature = "";

  markLoaded(jobId: string, clips: WtfClipInput[]): void { this.savedSignature = JSON.stringify({ jobId, clips }); }
  isDirty(jobId: string, clips: WtfClipInput[]): boolean { return this.savedSignature !== JSON.stringify({ jobId, clips }); }
  waitForIdle(): Promise<void> { return this.tail; }

  save(jobId: string, clips: WtfClipInput[], persist: (request: { jobId: string; clips: WtfClipInput[] }) => Promise<unknown>): Promise<void> {
    const snapshot = structuredClone({ jobId, clips });
    const signature = JSON.stringify(snapshot);
    const task = this.tail.catch(() => undefined).then(async () => {
      if (signature === this.savedSignature) return;
      await persist(snapshot);
      this.savedSignature = signature;
    });
    this.tail = task;
    return task;
  }
}

export function createWtfDevelopmentFixture(): AnalysisSummary {
  const hero = "npc_dota_hero_mirana", target = "npc_dota_hero_pudge";
  const createStory = (id: string, category: StoryCategory, start: number): HighlightStory => ({
    id, rank: start / 20, category, template_id: "synthetic-ui-fixture", title: category === "skill" ? "演示：先手命中后的连续追击" : "演示：重复互动的前因与结果",
    primary_hero: hero, participants: [{ hero, role: "protagonist" }, { hero: target, role: "opponent" }],
    candidate_ids: [`candidate-${id}`], source_start_seconds: start, source_peak_seconds: start + 5, source_end_seconds: start + 12,
    priority_score: 80, confidence: { level: "medium", score: 0.72, reasons: ["合成界面数据，不代表真实录像识别结果。"] },
    review_required: true, reasons: ["只演示工作台与多机位交接。"],
    evidence: [{ id: `evidence-${id}`, kind: "trigger", time_seconds: start + 3, actor: hero, target, detail: "合成证据：先手动作与随后事件用于展示核对流程。" }],
    beats: [{ id: `beat-${id}`, kind: "setup", source_start_seconds: start, source_end_seconds: start + 12, summary: "核对铺垫与事件，不自动补写反转。", evidence_ids: [`evidence-${id}`] }],
    shots: ["primary", "alternate"].map((role, index) => ({
      id: `shot-${id}-${index}`, order: index + 1, take_group_id: `scene-${id}`, take_role: role as "primary" | "alternate",
      include_in_default_cut: index === 0, beat_id: `beat-${id}`, candidate_id: `candidate-${id}`,
      source_start_seconds: start, source_end_seconds: start + 12,
      camera_mode: index ? "hero_chase" : "player_perspective", target_hero: index ? target : hero,
      framing: index ? "close_action" : "normal_gameplay", purpose: index ? "备用跟随画面" : "保留完整操作过程",
      fallback: { camera_mode: "player_perspective", target_hero: hero, reason: "无法应用镜头时使用玩家视角。" },
    })),
    switch_windows: [{ take_group_id: `scene-${id}`, alternate_shot_id: `shot-${id}-1`, source_start_seconds: start + 4, source_end_seconds: start + 8, reason: "合成切换建议，需人工核对。" }],
  });
  return {
    job_id: "d2h-wtf-development", job_dir: "C:\\WtfPractice\\jobs\\d2h-wtf-development",
    source: { path: "C:\\WtfPractice\\example.dem", byte_length: 1, sha256: "synthetic-wtf-fixture", magic: "PBDEMS2" },
    replay: { playback_ticks: 18000, playback_time_seconds: 600, game_build: 1, match_id: null, game_mode: null, game_winner: null,
      players: [hero, target].map((hero_name, slot) => ({ slot, hero_name, game_team: slot + 2, is_fake_client: false })) },
    event_count: 20, highlights: { schema_version: "1.4", source_sha256: "synthetic-wtf-fixture", detector: { name: "fixture", version: "1" }, candidates: [] },
    stories: { schema_version: "1.2", source_sha256: "synthetic-wtf-fixture", detector: { name: "fixture", version: "1" }, stories: [createStory("skill-example", "skill", 20), createStory("comedy-example", "comedy", 60)] },
    director: { schema_version: "1", source_sha256: "synthetic-wtf-fixture", template: "fixture", total_duration_seconds: 0, transition_seconds: 0, segments: [],
      audio: { music_role: "original", bpm_min: 0, bpm_max: 0, game_audio_duck_db: 0, music_duck_under_voice_db: 0, cues: [] } },
    reused_existing_job: false,
  };
}
