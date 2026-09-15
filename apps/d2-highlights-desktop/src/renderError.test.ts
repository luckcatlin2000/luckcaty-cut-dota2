import assert from "node:assert/strict";
import test from "node:test";
import { presentRenderError } from "./renderError.ts";

test("startup connection failures retain the latest diagnostic without becoming seek errors", () => {
  const error = "Dota 2 启动后未能建立离线控制连接。\n最近一次反馈：VConsole response format is invalid: invalid packet length";
  const shown = presentRenderError(error);
  assert.equal(shown.title, "未能连接游戏回放控制");
  assert.match(shown.message, /控制连接确认/);
  assert.doesNotMatch(shown.message, /播放位置/);
  assert.equal(shown.details, error);
});

test("missing seek feedback is readable and retains the original diagnostic", () => {
  for (const error of [
    "Dota 2 回放控制失败：回放跳转未确认，已停止录制：目标 tick 9000，最后实际 tick None，状态错误 None。",
    "Dota 2 回放控制失败：回放跳转未确认：未能读取录像当前播放位置，本次导出已停止。\n目标位置：9000 tick；最近收到的位置：未收到。",
  ]) {
    const shown = presentRenderError(error);
    assert.match(shown.message, /没有收到当前播放位置/);
    assert.doesNotMatch(shown.message, /None|tick/);
    assert.equal(shown.details, error);
  }
});

test("a wrong position is distinguished from missing feedback", () => {
  const shown = presentRenderError("Dota 2 回放控制失败：回放跳转未确认：录像尚未稳定到达指定位置，本次导出已停止。\n目标位置：9000 tick；最近收到的位置：500 tick。");
  assert.match(shown.message, /尚未稳定到达指定位置/);
  assert.doesNotMatch(shown.message, /没有收到/);
});

test("other failures keep their explanation and do not invent a seek cause", () => {
  const camera = "Dota 2 回放控制失败：镜头位置反馈不可识别";
  assert.equal(presentRenderError(camera).details, camera);
  const environment = "请先手动打开 Steam，再开始导出。";
  assert.deepEqual(presentRenderError(environment), {
    title: "本次导出未完成", message: environment, details: null,
  });
});
