import type { RenderDiagnosticArchive } from "./types.ts";

export interface DiagnosticExportRequest {
  jobId: string;
  report: string;
}

export type DiagnosticExportOutcome =
  | { status: "exported"; archive: RenderDiagnosticArchive }
  | { status: "cancelled" | "busy" }
  | { status: "failed"; message: string };

// Keep the save-dialog request exclusive even before React applies a state update.
export class RenderDiagnosticExporter {
  private pending = false;

  get busy(): boolean {
    return this.pending;
  }

  async run(
    native: boolean,
    request: DiagnosticExportRequest,
    exportArchive: (request: DiagnosticExportRequest) => Promise<RenderDiagnosticArchive | null>,
  ): Promise<DiagnosticExportOutcome> {
    if (this.pending) return { status: "busy" };
    if (!native) {
      return { status: "failed", message: "浏览器预览无法导出排查包，请在桌面软件中操作。" };
    }
    this.pending = true;
    try {
      const archive = await exportArchive(request);
      return archive ? { status: "exported", archive } : { status: "cancelled" };
    } catch (reason: unknown) {
      return { status: "failed", message: reason instanceof Error ? reason.message : String(reason) };
    } finally {
      this.pending = false;
    }
  }
}
