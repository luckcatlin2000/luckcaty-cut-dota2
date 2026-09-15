import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Summary {
  fingerprint: string;
  fileCount: number;
  bytes: number;
  directories: string[];
}

export function CacheCleanupPanel({ jobId, reportPath, disabled, onBusyChange }: {
  jobId: string;
  reportPath: string;
  disabled: boolean;
  onBusyChange: (busy: boolean) => void;
}) {
  const [preview, setPreview] = useState<Summary | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const reportName = reportPath.split(/[\\/]/).pop() ?? "";

  async function inspectOrClean(clean: boolean) {
    if (busy || disabled || (clean && !preview)) return;
    setBusy(true);
    onBusyChange(true);
    setMessage(clean ? "正在重新验证交付视频并清理…" : "正在核对交付视频和可清理文件…");
    try {
      const summary = await invoke<Summary>(clean ? "clean_render_cache" : "inspect_render_cache", {
        jobId, reportName, ...(clean ? { fingerprint: preview!.fingerprint } : {}),
      });
      setPreview(clean || summary.fileCount === 0 ? null : summary);
      setMessage(clean
        ? `已清理 ${summary.fileCount} 个原始文件（${(summary.bytes / 1024 ** 3).toFixed(2)} GiB），MP4 和断点记录已保留。`
        : summary.fileCount === 0 ? "没有可清理的已完成原始帧缓存。" : "");
    } catch (error) {
      setPreview(null);
      setMessage(String(error));
    } finally {
      setBusy(false);
      onBusyChange(false);
    }
  }

  return <section className="cache-cleanup-panel" aria-label="已完成任务缓存清理">
    <button className="secondary-button" disabled={busy || disabled} onClick={() => void inspectOrClean(false)}>
      检查可清理缓存
    </button>
    {message && <p role="status">{message}</p>}
    {preview && <div>
      <p>可清理 {preview.fileCount} 个原始 JPG/WAV 文件，合计 {(preview.bytes / 1024 ** 3).toFixed(2)} GiB。</p>
      <p>仅清理下列目录内已核对的录制原始帧和音频。保留成片、源素材、所有缓存 MP4、断点状态和诊断文件。</p>
      <details><summary>查看具体目录（{preview.directories.length} 个）</summary>
        <ul>{preview.directories.map(path => <li key={path}>{path}</li>)}</ul>
      </details>
      <p>清理后仍可复用完成的 MP4；如需从原始帧重新编码，必须重新录制。删除无法撤销。</p>
      <button className="danger-button" disabled={busy || disabled} onClick={() => void inspectOrClean(true)}>确认清理原始帧缓存</button>
      <button className="secondary-button" disabled={busy} onClick={() => setPreview(null)}>保留缓存</button>
    </div>}
  </section>;
}
