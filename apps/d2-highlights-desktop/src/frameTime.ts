export const EDITOR_FPS = 30;
const FRAME_EPSILON = 0.000001;

export function roundFrame(seconds: number) {
  return Math.round(seconds * EDITOR_FPS) / EDITOR_FPS;
}

export function replayEndFrame(seconds: number) {
  return Math.max(0, Math.floor(seconds * EDITOR_FPS + FRAME_EPSILON)) / EDITOR_FPS;
}

export function frameRangeWithinReplay(start: number, end: number, replayEnd: number) {
  const endSeconds = Math.max(0, Math.min(roundFrame(end), replayEndFrame(replayEnd)));
  const startSeconds = Math.max(0, Math.min(roundFrame(start), endSeconds));
  return { startSeconds, endSeconds };
}

export function isValidClipTimeRange(start: number, end: number, replayEnd: number) {
  if (![start, end, replayEnd].every(Number.isFinite) || start < 0 || end <= start ||
      end > replayEnd + FRAME_EPSILON) return false;
  const frames = Math.round(end * EDITOR_FPS) - Math.round(start * EDITOR_FPS);
  return frames >= EDITOR_FPS && frames <= 100 * EDITOR_FPS;
}
