import type { RenderResumeSummary } from "./types";

export function resumeAction(summary: RenderResumeSummary | null) {
  if (!summary || !summary.fingerprint || summary.completedClips <= 0 ||
    summary.completedClips + summary.remainingClips !== summary.totalClips ||
    summary.remainingClips < 0 ||
    (summary.remainingClips > 0 && (summary.firstPendingClip === null ||
      summary.firstPendingClip < 1 || summary.firstPendingClip > summary.totalClips))) return null;
  return {
    title: `已完成 ${summary.completedClips} 段 · 剩余 ${summary.remainingClips} 段`,
    button: summary.remainingClips === 0 ? "继续合成" : `从第 ${summary.firstPendingClip} 段继续`,
    description: summary.remainingClips === 0
      ? "全部片段已保存。继续后直接合成，无需启动 Dota 2。"
      : "继续后只录制缺失或未通过检查的片段，最后按原顺序合成。",
  };
}
