import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import test from "node:test";

const source = readFileSync(new URL("./CacheCleanupPanel.tsx", import.meta.url), "utf8");
const start = source.indexOf("async function inspectOrClean(");
const end = source.indexOf("\n  return <section", start);
const javascript = stripTypeScriptTypes(source.slice(start, end));

function setup(options: { preview?: object | null; busy?: boolean; disabled?: boolean; fail?: boolean } = {}) {
  const calls: { command: string; args: unknown }[] = [];
  let preview: unknown = options.preview ?? null;
  let message = "";
  const busyChanges: boolean[] = [];
  const environment = {
    preview: options.preview ?? null, busy: options.busy ?? false, disabled: options.disabled ?? false,
    jobId: "fixture", reportName: "match_质量报告.json",
    setBusy: () => {}, onBusyChange: (busy: boolean) => busyChanges.push(busy),
    setPreview: (value: unknown) => { preview = value; }, setMessage: (value: string) => { message = value; },
    invoke: async (command: string, args: unknown) => {
      calls.push({ command, args });
      if (options.fail) throw new Error("changed snapshot");
      return { fingerprint: "fresh", fileCount: 2, bytes: 10, directories: ["fixture/raw"] };
    },
  };
  const run = new Function(...Object.keys(environment), `return (${javascript});`)(...Object.values(environment));
  return { run, calls, busyChanges, result: () => ({ preview, message }) };
}

test("checking only previews; cleanup requires an explicit preview fingerprint", async () => {
  const inspect = setup();
  await inspect.run(false);
  assert.equal(inspect.calls[0].command, "inspect_render_cache");
  assert.ok(inspect.result().preview);
  const absent = setup();
  await absent.run(true);
  assert.equal(absent.calls.length, 0);
  const clean = setup({ preview: { fingerprint: "approved" } });
  await clean.run(true);
  assert.deepEqual(clean.calls, [{ command: "clean_render_cache", args: { jobId: "fixture", reportName: "match_质量报告.json", fingerprint: "approved" } }]);
  assert.equal(clean.result().preview, null);
  assert.deepEqual(clean.busyChanges, [true, false]);
});

test("active render blocks cleanup and backend failure invalidates confirmation", async () => {
  for (const options of [{ disabled: true }, { busy: true }]) {
    const blocked = setup({ ...options, preview: { fingerprint: "approved" } });
    await blocked.run(true);
    assert.equal(blocked.calls.length, 0);
  }
  const failed = setup({ fail: true, preview: { fingerprint: "approved" } });
  await failed.run(true);
  assert.equal(failed.result().preview, null);
  assert.match(failed.result().message, /changed snapshot/);
  assert.deepEqual(failed.busyChanges, [true, false]);
});
