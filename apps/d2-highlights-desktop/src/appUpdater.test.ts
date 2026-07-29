import assert from "node:assert/strict";
import test from "node:test";
import {
  UPDATE_POLICY,
  appUpdateCopy,
  updateProgressPercent,
} from "./appUpdater.ts";

test("keeps application updates optional", () => {
  assert.equal(UPDATE_POLICY.checkOnStartup, true);
  assert.equal(UPDATE_POLICY.downloadAutomatically, false);
  assert.equal(UPDATE_POLICY.installAutomatically, false);
});

test("calculates bounded update download progress", () => {
  assert.equal(updateProgressPercent(25, 100), 25);
  assert.equal(updateProgressPercent(150, 100), 100);
  assert.equal(updateProgressPercent(-5, 100), 0);
  assert.equal(updateProgressPercent(10, null), null);
  assert.equal(updateProgressPercent(10, 0), null);
});

test("uses the first useful release-note line for an available update", () => {
  const copy = appUpdateCopy({
    phase: "available",
    metadata: {
      version: "1.8.0",
      currentVersion: "1.7.0",
      notes: "\n## 更新内容\n- 时间轴更清楚",
      publishedAt: null,
    },
  });

  assert.equal(copy.title, "发现 1.8.0");
  assert.equal(copy.detail, "更新内容");
  assert.equal(copy.action, "install");
});

test("blocks installation while a local task is active", () => {
  const copy = appUpdateCopy(
    {
      phase: "available",
      metadata: {
        version: "1.8.0",
        currentVersion: "1.7.0",
        notes: "",
        publishedAt: null,
      },
    },
    true,
  );

  assert.equal(copy.action, null);
  assert.match(copy.detail, /任务完成/);
});

test("offers retry after a failed update check", () => {
  const copy = appUpdateCopy({
    phase: "error",
    message: "网络暂时不可用",
  });

  assert.equal(copy.title, "暂时无法检查");
  assert.equal(copy.action, "check");
});
