import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { adoptWtfStory, createWtfDevelopmentFixture, editWtfSceneTime, filterWtfStories, moveWtfScene,
  validateWtfClips, validateWtfDraft, WtfDraftSequence, wtfDefaultDuration, wtfSceneGroups } from "./wtfWorkbenchModel.ts";
import type { WtfClipInput } from "./wtfWorkbenchModel.ts";

const fixture = createWtfDevelopmentFixture();
const [first, second] = fixture.stories.stories;

test("story adoption preserves synchronized primary/alternate takes and never duplicates final duration", () => {
  const clips = adoptWtfStory(first!, [], 600);
  assert.equal(clips.length, 2);
  assert.deepEqual(clips.map(clip => [clip.takeRole, clip.includeInFinal]), [["primary", true], ["alternate", false]]);
  assert.equal(clips[0]!.takeGroupId, clips[1]!.takeGroupId);
  assert.equal(clips[0]!.startSeconds, clips[1]!.startSeconds);
  assert.notEqual(clips[0]!.hero, clips[1]!.hero);
  assert.equal(wtfDefaultDuration(clips), 12);
  assert.deepEqual(adoptWtfStory(first!, clips, 600), clips);
});

test("editing and ordering operate on scenes together while keeping takes and source identity", () => {
  const clips = adoptWtfStory(second!, adoptWtfStory(first!, [], 600), 600);
  const changed = editWtfSceneTime(clips, clips[0]!.takeGroupId, 18, 33, 600);
  assert.deepEqual(changed.slice(0, 2).map(clip => [clip.startSeconds, clip.endSeconds]), [[18, 33], [18, 33]]);
  assert.equal(wtfDefaultDuration(changed), 27);
  const moved = moveWtfScene(changed, changed[0]!.takeGroupId, 1);
  assert.equal(wtfSceneGroups(moved)[0]![0]!.storyId, second!.id);
  assert.deepEqual(moved.slice(2).map(clip => clip.shotId), changed.slice(0, 2).map(clip => clip.shotId));
  for (const end of [18, 118.1, 601]) assert.throws(() => editWtfSceneTime(clips, clips[0]!.takeGroupId, 18, end, 600));
});

test("filters use actual categories and participant evidence, with all retaining fight/objective", () => {
  const fight = { ...first!, id: "fight-example", category: "fight" as const };
  const stories = [...fixture.stories.stories, fight];
  assert.equal(filterWtfStories(stories, "all").length, 3);
  assert.deepEqual(filterWtfStories(stories, "comedy").map(story => story.id), [second!.id]);
  assert.equal(filterWtfStories(stories, "all", "npc_dota_hero_pudge").length, 3);
  assert.equal(filterWtfStories(stories, "all", "npc_dota_hero_axe").length, 0);
  assert.equal(filterWtfStories(stories, "mistake").length, 0);
});

test("draft restoration keeps order and rejects changed source or broken synchronized takes", () => {
  const clips = adoptWtfStory(first!, [], 600);
  const draft = { jobId: fixture.job_id, sourceSha256: fixture.source.sha256, clips, updatedUnixSeconds: 1 };
  assert.deepEqual(validateWtfDraft(fixture, draft), clips);
  assert.throws(() => validateWtfDraft(fixture, { ...draft, sourceSha256: "changed" }), /不匹配/);
  assert.throws(() => validateWtfClips([{ ...clips[0]!, includeInFinal: false }, clips[1]!], 600), /主机位/);
  assert.throws(() => validateWtfClips([clips[0]!, { ...clips[1]!, startSeconds: 21 }], 600), /相同时间窗/);
});

test("draft saves serialize snapshots and failed saves stay dirty until a successful retry", async () => {
  const sequence = new WtfDraftSequence();
  const clips = adoptWtfStory(first!, [], 600);
  const changed = editWtfSceneTime(clips, clips[0]!.takeGroupId, 19, 33, 600);
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  const writes: WtfClipInput[][] = [];
  const a = sequence.save("job", clips, async request => { writes.push(request.clips); await held; });
  const b = sequence.save("job", changed, async request => { writes.push(request.clips); });
  await Promise.resolve(); await Promise.resolve();
  assert.equal(writes.length, 1);
  release(); await Promise.all([a, b]);
  assert.deepEqual(writes[1], changed);
  assert.equal(sequence.isDirty("job", changed), false);
  await assert.rejects(sequence.save("job", clips, async () => { throw new Error("disk full"); }), /disk full/);
  assert.equal(sequence.isDirty("job", clips), true);
  await sequence.save("job", clips, async () => undefined);
  assert.equal(sequence.isDirty("job", clips), false);
});

const uiSource = readFileSync(new URL("./WtfWorkbench.tsx", import.meta.url), "utf8");
function uiHandler(name: string, next: string, environment: Record<string, unknown>) {
  const start = uiSource.indexOf(`  async function ${name}(`);
  const end = uiSource.indexOf(next, start);
  assert.ok(start >= 0 && end > start);
  const fn = stripTypeScriptTypes(uiSource.slice(start, end).trim());
  return new Function(...Object.keys(environment), `return (${fn});`)(...Object.values(environment));
}

test("window close waits for the pending action and latest draft before requesting backend cleanup", async () => {
  const events: string[] = [];
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  const close = uiHandler("requestClose", "  const requestCloseRef", {
    native: true, closingRef: { current: false }, setClosing: () => undefined, setError: () => undefined,
    pendingAction: { current: pending }, queue: { current: { waitForIdle: async () => { events.push("wait-saves"); } } },
    saveCurrent: async () => { events.push("save-latest"); }, invoke: async (command: string) => { events.push(command); }, errorText: String,
  });
  const task = close();
  assert.deepEqual(events, []);
  release(); await task;
  assert.deepEqual(events, ["wait-saves", "save-latest", "close_app_after_cleanup"]);
});

test("a close-time save failure leaves the native window open and retryable", async () => {
  let closed = false, message = "";
  const closing = { current: false };
  const close = uiHandler("requestClose", "  const requestCloseRef", {
    native: true, closingRef: closing, setClosing: () => undefined, setError: (value: string) => { message = value; },
    pendingAction: { current: null }, queue: { current: { waitForIdle: async () => undefined } },
    saveCurrent: async () => { throw new Error("disk full"); }, invoke: async () => { closed = true; }, errorText: String,
  });
  await close();
  assert.equal(closed, false);
  assert.equal(closing.current, false);
  assert.match(message, /disk full/);
});

test("a rejected existing draft cannot become an autosavable empty replacement", async () => {
  const ready = { current: true };
  const analyze = uiHandler("analyze", "  async function chooseReplay", {
    runAction: async (_label: string, work: () => Promise<void>) => work(), saveCurrent: async () => undefined,
    setProgress: () => undefined, setHandoff: () => undefined, native: true,
    Channel: class { onmessage: unknown },
    invoke: async (command: string) => { if (command === "analyze_wtf_dem") return fixture; throw new Error("damaged saved draft"); },
    readyRef: ready, setDraftReady: () => undefined, summaryRef: { current: null }, setSummary: () => undefined, updateClips: () => undefined,
  });
  await assert.rejects(analyze("example.dem"), /damaged saved draft/);
  assert.equal(ready.current, false);
});
