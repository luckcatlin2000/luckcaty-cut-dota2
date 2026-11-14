import assert from "node:assert/strict";
import test from "node:test";
import { RenderDiagnosticExporter } from "./diagnosticExport.ts";
import { renderErrorReport } from "./renderError.ts";
import type { DiagnosticExportRequest } from "./diagnosticExport.ts";
import type { RenderDiagnosticArchive } from "./types.ts";

const originalError = "媒体处理失败：Dota 2 原生逐帧导出超时：需要 1200 帧，只得到 900 帧";
const request = {
  jobId: "d2h-example",
  report: renderErrorReport(originalError, {
    version: "2.4.0", module: "高光时刻", matchId: 1234567890, jobId: "d2h-example",
    stage: "capture", currentClip: 1, totalClips: 2,
  }),
};
const archive: RenderDiagnosticArchive = {
  archivePath: "C:\\exports\\diagnostics.zip", fileCount: 4, warnings: ["一份历史日志已省略"],
};

test("diagnostic export preserves the exact error report and archive warnings", async () => {
  let received: DiagnosticExportRequest | undefined;
  const exporter = new RenderDiagnosticExporter();
  const outcome = await exporter.run(true, request, async value => { received = value; return archive; });
  assert.deepEqual(received, request);
  assert.ok(received?.report.endsWith(originalError));
  assert.deepEqual(outcome, { status: "exported", archive });
  assert.equal(exporter.busy, false);
});

test("an open save dialog prevents a second request and cancellation allows retry", async () => {
  let finish: ((value: RenderDiagnosticArchive | null) => void) | undefined;
  let calls = 0;
  const exporter = new RenderDiagnosticExporter();
  const first = exporter.run(true, request, () => {
    calls++;
    return new Promise(resolve => { finish = resolve; });
  });
  assert.equal(exporter.busy, true);
  assert.deepEqual(await exporter.run(true, request, async () => { calls++; return archive; }), { status: "busy" });
  assert.equal(calls, 1);
  finish!(null);
  assert.deepEqual(await first, { status: "cancelled" });
  assert.equal(exporter.busy, false);
  assert.deepEqual(await exporter.run(true, request, async () => archive), { status: "exported", archive });
});

test("browser preview never calls the native exporter", async () => {
  const exporter = new RenderDiagnosticExporter();
  const outcome = await exporter.run(false, request, async () => { assert.fail("native API called in browser preview"); });
  assert.equal(outcome.status, "failed");
  if (outcome.status === "failed") assert.match(outcome.message, /桌面软件/);
  assert.equal(exporter.busy, false);
});

test("save failure is separate from the recording error and permits retry", async () => {
  const exporter = new RenderDiagnosticExporter();
  const before = request.report;
  assert.deepEqual(await exporter.run(true, request, async () => { throw new Error("排查包保存失败"); }), {
    status: "failed", message: "排查包保存失败",
  });
  assert.equal(request.report, before);
  assert.ok(request.report.endsWith(originalError));
  assert.equal(exporter.busy, false);
  assert.deepEqual(await exporter.run(true, request, async () => archive), { status: "exported", archive });
});
