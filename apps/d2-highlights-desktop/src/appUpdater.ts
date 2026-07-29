export interface AppUpdateMetadata {
  version: string;
  currentVersion: string;
  notes: string;
  publishedAt: string | null;
}

export const UPDATE_POLICY = {
  checkOnStartup: true,
  downloadAutomatically: false,
  installAutomatically: false,
} as const;

export type AppUpdateEvent =
  | {
      event: "started";
      data: { contentLength: number | null };
    }
  | {
      event: "progress";
      data: { chunkLength: number };
    }
  | {
      event: "finished";
    };

export type AppUpdateState =
  | { phase: "checking" }
  | { phase: "current" }
  | { phase: "available"; metadata: AppUpdateMetadata }
  | {
      phase: "downloading";
      metadata: AppUpdateMetadata;
      downloadedBytes: number;
      totalBytes: number | null;
    }
  | { phase: "installing"; metadata: AppUpdateMetadata }
  | { phase: "error"; message: string }
  | { phase: "unavailable" };

export type AppUpdateAction = "check" | "install" | null;

export interface AppUpdateCopy {
  title: string;
  detail: string;
  action: AppUpdateAction;
}

export function updateProgressPercent(
  downloadedBytes: number,
  totalBytes: number | null,
) {
  if (
    totalBytes === null ||
    !Number.isFinite(totalBytes) ||
    totalBytes <= 0
  ) {
    return null;
  }
  const downloaded = Number.isFinite(downloadedBytes)
    ? Math.max(0, downloadedBytes)
    : 0;
  return Math.min(100, Math.round((downloaded / totalBytes) * 100));
}

export function appUpdateCopy(
  state: AppUpdateState,
  installationBlocked = false,
): AppUpdateCopy {
  switch (state.phase) {
    case "checking":
      return {
        title: "正在检查更新",
        detail: "连接官方 GitHub Releases",
        action: null,
      };
    case "current":
      return {
        title: "已是最新版本",
        detail: "后续版本会在这里提示",
        action: "check",
      };
    case "available":
      return {
        title: `发现 ${state.metadata.version}`,
        detail: installationBlocked
          ? "请先等待当前分析或导出任务完成"
          : firstUsefulLine(state.metadata.notes) ||
            "已通过官方渠道发现可用更新",
        action: installationBlocked ? null : "install",
      };
    case "downloading": {
      const percent = updateProgressPercent(
        state.downloadedBytes,
        state.totalBytes,
      );
      return {
        title:
          percent === null ? "正在下载更新" : `正在下载更新 ${percent}%`,
        detail: "下载完成后会验证发布签名",
        action: null,
      };
    }
    case "installing":
      return {
        title: "正在安装更新",
        detail: "签名已验证，软件将自动重启",
        action: null,
      };
    case "error":
      return {
        title: "暂时无法检查",
        detail: state.message,
        action: "check",
      };
    case "unavailable":
      return {
        title: "开发预览模式",
        detail: "应用内更新仅在安装版中启用",
        action: null,
      };
  }
}

function firstUsefulLine(notes: string) {
  return (
    notes
      .split(/\r?\n/)
      .map((line) => line.replace(/^[-*#\s]+/, "").trim())
      .find(Boolean) ?? ""
  );
}
