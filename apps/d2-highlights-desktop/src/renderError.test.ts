import assert from "node:assert/strict";
import test from "node:test";
import {readFileSync} from "node:fs";
import { presentRenderError, renderErrorReport } from "./renderError.ts";

test("Steam failures have specific guidance and file errors are not mistaken for Steam",()=>{
  assert.equal(presentRenderError("Steam 未运行。普通画面的昵称需要Steam。").title,"请先打开 Steam");
  const error="文件操作失败：系统找不到指定的文件。 (os error 2)";
  assert.equal(presentRenderError(error).code,"D2H-FS-002");
  assert.equal(presentRenderError(error).details,error);
  assert.doesNotMatch(presentRenderError(error).message,/Steam/);
});

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
  assert.equal(presentRenderError(environment).code,"D2H-EXP-000");
  assert.equal(presentRenderError(environment).details,environment);
});

test("Windows path failure has a stable code without inventing a specific missing path",()=>{
  const error="文件操作失败：系统找不到指定的路径。 (os error 3)";
  const shown=presentRenderError(error);
  assert.equal(shown.code,"D2H-FS-003");
  assert.match(shown.message,/路径/);
  assert.doesNotMatch(shown.message,/Steam|国服|一定|肯定/);
  assert.equal(shown.details,error);
  assert.ok(shown.steps.length>0);
});

test("connection reset is separated from path and seek failures",()=>{
  const shown=presentRenderError("Dota 2 回放控制失败：VConsole I/O error: 远程主机强迫关闭了一个现有的连接。 (os error 10054)");
  assert.equal(shown.code,"D2H-CTRL-054");
  assert.match(shown.message,/控制连接被断开/);
  assert.doesNotMatch(shown.message,/Steam|国服|系统找不到/);
});

test("common export failures and unknown errors remain searchable with original details",()=>{
  const cases=[
    ["Dota 2 已在运行。请先关闭。","D2H-GAME-001"],
    ["Dota 2 在离线导出期间意外退出。","D2H-GAME-002"],
    ["文件操作失败：拒绝访问。 (os error 5)","D2H-FS-005"],
    ["文件操作失败：磁盘空间不足。 (os error 112)","D2H-FS-112"],
    ["找不到或无法运行 FFmpeg。","D2H-TOOL-001"],
    ["Dota 2 没有输出片段 x 的有效帧序列。","D2H-CAPTURE-001"],
    ["媒体处理失败：Dota 2 原生导出没有生成 WAV 音轨。","D2H-CAPTURE-001"],
    ["媒体处理失败：FFmpeg exit 1","D2H-MEDIA-001"],
    ["剪辑方案无效：不在录像范围内","D2H-PLAN-001"],
    ["成片任务已取消。","D2H-CANCEL-001"],
    ["未收录的故障，原文不能丢","D2H-EXP-000"],
  ];
  for(const [error,code] of cases){
    const shown=presentRenderError(error!);
    assert.equal(shown.code,code);
    assert.equal(shown.details,error);
  }
});

test("copyable report contains the version, match, stage, log path and exact original error",()=>{
  const error="文件操作失败：保存录制记录失败；操作：替换正式记录；路径：G:\\案例\\capture-diagnostics.json；系统找不到指定的路径。 (os error 3)";
  const text=renderErrorReport(error,{version:"2.3.7",module:"剧情剪辑",matchId:1234567890,jobId:"d2h-example",stage:"capture",currentClip:1,totalClips:8,logDirectory:"G:\\案例\\render\\logs"});
  for(const value of ["D2H-FS-003","2.3.7","1234567890","剧情剪辑","capture","1/8","G:\\案例\\render\\logs",error])assert.ok(text.includes(value));
});

test("all stable error codes have entries in the independent error catalog",()=>{
  const source=readFileSync(new URL("./renderError.ts",import.meta.url),"utf8");
  const catalog=readFileSync(new URL("../../../docs/ERROR_CATALOG.md",import.meta.url),"utf8");
  const codes=new Set(Array.from(source.matchAll(/"(D2H-[A-Z]+-\d+)"/g),match=>match[1]));
  assert.ok(codes.size>=15);
  for(const code of codes)assert.ok(catalog.includes(code!),`Missing catalog entry for ${code}`);
});
