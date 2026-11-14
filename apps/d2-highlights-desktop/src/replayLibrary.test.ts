import assert from "node:assert/strict";
import test from "node:test";
import { displayReplayDirectory, formatReplayDownloadTime, rememberResolvedReplayDirectory, ReplayLibraryRefresher } from "./replayLibrary.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((success, failure) => { resolve = success; reject = failure; });
  return { promise, resolve, reject };
}

test("overlapping refresh requests coalesce without concurrent directory reads", async () => {
  const first = deferred<string>();
  const second = deferred<string>();
  const secondStarted = deferred<void>();
  const results: string[] = [];
  let calls = 0;
  let concurrent = 0;
  let maximum = 0;
  const refresh = new ReplayLibraryRefresher(async () => {
    calls++;
    if (calls === 2) secondStarted.resolve();
    maximum = Math.max(maximum, ++concurrent);
    try { return await (calls === 1 ? first.promise : second.promise); }
    finally { concurrent--; }
  }, value => results.push(value), error => { throw error; }, () => {});
  const pending = refresh.refresh();
  await refresh.refresh();
  await refresh.refresh();
  assert.equal(calls, 1);
  first.resolve("old list");
  await secondStarted.promise;
  assert.equal(calls, 2);
  second.resolve("new list");
  await pending;
  assert.equal(maximum, 1);
  assert.deepEqual(results, ["old list", "new list"]);
});

test("switching directory disposes the old response and pending refresh", async () => {
  const loading = deferred<string>();
  const results: string[] = [];
  let calls = 0;
  const refresh = new ReplayLibraryRefresher(() => { calls++; return loading.promise; }, value => results.push(value), error => { throw error; }, () => {});
  const pending = refresh.refresh();
  await refresh.refresh();
  refresh.dispose();
  loading.resolve("old directory");
  await pending;
  await refresh.refresh();
  assert.equal(calls, 1);
  assert.deepEqual(results, []);
});

test("a missing directory can recover on the next refresh", async () => {
  const errors: unknown[] = [], results: string[] = [], busy: boolean[] = [];
  let calls = 0;
  const refresh = new ReplayLibraryRefresher(async () => {
    if (++calls === 1) throw new Error("directory removed");
    return "downloaded replay";
  }, value => results.push(value), error => errors.push(error), value => busy.push(value));
  await refresh.refresh();
  await refresh.refresh();
  assert.equal(errors.length, 1);
  assert.deepEqual(results, ["downloaded replay"]);
  assert.deepEqual(busy, [true, false, true, false]);
});

test("displaying Windows long paths does not change the source path", () => {
  const source = "\\\\?\\D:\\Dota\\game\\dota\\replays";
  assert.equal(displayReplayDirectory(source), "D:\\Dota\\game\\dota\\replays");
  assert.equal(source, "\\\\?\\D:\\Dota\\game\\dota\\replays");
  assert.equal(displayReplayDirectory("\\\\?\\UNC\\server\\replays"), "\\\\server\\replays");
  assert.equal(displayReplayDirectory("game/dota/replays"), "game/dota/replays");
  assert.equal(formatReplayDownloadTime(null), "时间未知");
  assert.equal(formatReplayDownloadTime(Number.NaN), "时间未知");
});

test("resolved relative directories keep library and numbered replay import on the same folder", () => {
  const resolved = "\\\\?\\D:\\Dota\\game\\dota\\replays";
  for (const entered of ["", "replays", "dota/replays", "game/dota/replays", "D:replays"]) {
    assert.equal(rememberResolvedReplayDirectory(entered, resolved), resolved);
  }
  for (const entered of ["E:\\My Replays", "D:/Dota/replays", resolved, "\\\\server\\replays"]) {
    assert.equal(rememberResolvedReplayDirectory(entered, resolved), entered);
  }
});
