import assert from "node:assert/strict";
import test from "node:test";
import { EXTERNAL_TOOLS, UNRELEASED_MODULE_MESSAGE, canLaunchExternalTool, externalToolNotice } from "./externalTools.ts";

test("the release catalog keeps unavailable modules visibly distinct from installable downloads", () => {
  assert.equal(new Set(EXTERNAL_TOOLS.map((tool) => tool.id)).size, EXTERNAL_TOOLS.length);
  for (const tool of EXTERNAL_TOOLS) {
    const notice = externalToolNotice(tool.id);
    assert.ok(notice.title.includes(tool.name));
    if (tool.availability === "planned") {
      assert.equal(notice.message, UNRELEASED_MODULE_MESSAGE);
      assert.match(notice.title, /尚未正式提供/);
      assert.equal(canLaunchExternalTool(tool.id), false);
    } else {
      assert.match(notice.message, /未检测到.*独立软件/);
      assert.match(notice.message, /请单独下载并安装/);
      assert.doesNotMatch(notice.message, /仍在开发|暂未开放/);
    }
    assert.equal(tool.action, undefined);
  }
});

test("future released software can use installation discovery while unknown IDs stay unavailable", () => {
  const tools = [
    { id: "custom-video", name: "视频生成工具", availability: "download_required" },
  ] as const;
  const custom = externalToolNotice("custom-video", tools);
  assert.equal(custom.title, "视频生成工具需单独下载");
  assert.match(custom.message, /请单独下载/);
  assert.equal(canLaunchExternalTool("custom-video", tools), true);
  assert.equal(canLaunchExternalTool("unknown", tools), false);
  assert.equal(externalToolNotice("unknown").message, UNRELEASED_MODULE_MESSAGE);
});

test("WTF editing and replay analysis are distinct integrations", () => {
  assert.equal(EXTERNAL_TOOLS.find(tool => tool.id === "wtf")?.name, "WTF 剪辑");
  assert.equal(EXTERNAL_TOOLS.find(tool => tool.id === "analysis")?.name, "录像分析");
  assert.equal(externalToolNotice("wtf").title, "WTF 剪辑尚未正式提供");
  assert.equal(externalToolNotice("analysis").title, "录像分析尚未正式提供");
  for (const id of ["wtf", "story", "analysis"]) {
    assert.equal(externalToolNotice(id).message, UNRELEASED_MODULE_MESSAGE);
    assert.equal(canLaunchExternalTool(id), false);
  }
});
