/** Serialize metadata refreshes and ignore late responses from an old directory. */
export class ReplayLibraryRefresher<T> {
  private active = true;
  private running = false;
  private queued = false;
  private readonly load: () => Promise<T>;
  private readonly onResult: (result: T) => void;
  private readonly onError: (error: unknown) => void;
  private readonly onLoading: (loading: boolean) => void;

  constructor(
    load: () => Promise<T>,
    onResult: (result: T) => void,
    onError: (error: unknown) => void,
    onLoading: (loading: boolean) => void,
  ) { this.load = load; this.onResult = onResult; this.onError = onError; this.onLoading = onLoading; }

  async refresh(): Promise<void> {
    if (!this.active) return;
    if (this.running) {
      this.queued = true;
      return;
    }
    this.running = true;
    try {
      do {
        this.queued = false;
        this.onLoading(true);
        try {
          const result = await this.load();
          if (this.active) this.onResult(result);
        } catch (error: unknown) {
          if (this.active) this.onError(error);
        }
      } while (this.active && this.queued);
    } finally {
      this.running = false;
      if (this.active) this.onLoading(false);
    }
  }

  dispose(): void {
    this.active = false;
    this.queued = false;
  }
}

export function displayReplayDirectory(path: string): string {
  if (path.startsWith("\\\\?\\UNC\\")) return `\\\\${path.slice(8)}`;
  return path.startsWith("\\\\?\\") ? path.slice(4) : path;
}

export function rememberResolvedReplayDirectory(current: string, resolved: string): string {
  const path = current.trim();
  const absolute = /^[a-z]:[\\/]/i.test(path) || path.startsWith("\\\\") || path.startsWith("//");
  return absolute ? current : resolved;
}

export function formatReplayDownloadTime(unixMs: number | null): string {
  if (unixMs === null || !Number.isFinite(unixMs)) return "时间未知";
  const date = new Date(unixMs);
  if (!Number.isFinite(date.getTime())) return "时间未知";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function replayTimeDescription(basis: "created" | "modified" | "unknown"): string {
  return basis === "created"
    ? "按本机文件创建时间排序；复制或还原录像可能改变此时间。"
    : basis === "modified"
      ? "未能读取创建时间，使用文件修改时间。"
      : "未能读取文件时间。";
}
