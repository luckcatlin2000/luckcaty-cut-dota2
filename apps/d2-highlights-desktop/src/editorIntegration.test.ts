import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import test from "node:test";
import { DraftWriter, EditorOperationGate } from "./editorSession.ts";
import { frameRangeWithinReplay, isValidClipTimeRange, replayEndFrame, roundFrame } from "./frameTime.ts";
import { normalizeStoryDurationClips } from "./storyDuration.ts";
import type { EditPlanClip, ImportedClip } from "./types.ts";
import { canLaunchExternalTool, externalToolNotice, UNRELEASED_MODULE_MESSAGE } from "./externalTools.ts";

// Exercise the actual App handlers without mounting a Webview or calling native IPC.
const source = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const noop = () => undefined;
function handler(name: string, next: string, environment: Record<string, unknown>) {
  const start = source.search(new RegExp(`^\\s*(?:async )?function ${name}\\(`, "m"));
  assert.ok(start >= 0, name);
  const tail = source.slice(start);
  const end = tail.search(new RegExp(`^\\s*(?:async )?function ${next}\\(`, "m"));
  assert.ok(end > 0, next);
  const javascript = stripTypeScriptTypes(tail.slice(0, end).trim());
  return new Function(...Object.keys(environment), `return (${javascript});`)(...Object.values(environment));
}

test("unreleased modules never invoke installation or launch IPC", async () => {
  let calls = 0;
  const notices: Array<{message:string}> = [];
  const openTool = handler("openExternalTool", "openImportDialog", {
    operationGate: { current: new EditorOperationGate() }, isTauriRuntime: true,
    externalToolNotice, canLaunchExternalTool, toErrorMessage: String,
    setExternalNotice: (value: {message:string}) => { notices.push(value); },
    invoke: async () => { calls++; return { status: "opened" }; },
  });
  for (const id of ["wtf", "story", "analysis"]) await openTool(id);
  assert.equal(calls, 0);
  assert.equal(notices.length, 3);
  assert.ok(notices.every(notice => notice.message === UNRELEASED_MODULE_MESSAGE));
});

test("future released modules retain separate installation discovery and launch requests", async () => {
  const calls: Array<{name:string;toolId:string}> = [];
  let notice: unknown = undefined;
  const openTool = handler("openExternalTool", "openImportDialog", {
    operationGate: { current: new EditorOperationGate() }, isTauriRuntime: true,
    externalToolNotice, canLaunchExternalTool: (id: string) => canLaunchExternalTool(id, [
      { id: "wtf", name: "WTF 剪辑", availability: "download_required" },
      { id: "analysis", name: "录像分析", availability: "download_required" },
    ]), toErrorMessage: String,
    setExternalNotice: (value: unknown) => { notice = value; },
    invoke: async (name: string, request: {toolId:string}) => {
      calls.push({ name, toolId: request.toolId });
      return { status: "opened" };
    },
  });
  await openTool("wtf");
  await openTool("analysis");
  assert.deepEqual(calls, [
    { name: "open_external_tool", toolId: "wtf" },
    { name: "open_external_tool", toolId: "analysis" },
  ]);
  assert.equal(notice, null);
});

test("a failed pre-switch save leaves the old path and never starts analysis", async () => {
  const gate = new EditorOperationGate();
  let path = "A.dem";
  let nativeCalls = 0;
  const run = handler("runAnalysis", "confirmReanalysis", {
    selectedPath: path, operationGate: { current: gate }, planConflict: false,
    setBusy: noop, setError: noop, setNotice: noop, toErrorMessage: String,
    latestDraftRef: { current: { jobId: "A" } },
    flushCurrentDraft: async () => { throw new Error("disk full"); },
    setSelectedPath: (value: string) => { path = value; },
    invoke: async () => { nativeCalls++; },
  });
  await run("B.dem");
  assert.equal(path, "A.dem");
  assert.equal(nativeCalls, 0);
  assert.equal(gate.blocked, false);
});

test("library requests during analysis or update cannot change the displayed path", async () => {
  for (const kind of ["analysis", "render", "update"] as const) {
    const gate = new EditorOperationGate();
    gate.begin(kind);
    let changed = false;
    const run = handler("runAnalysis", "confirmReanalysis", {
      selectedPath: "A.dem", operationGate: { current: gate },
      setSelectedPath: () => { changed = true; },
    });
    await run("B.dem");
    assert.equal(changed, false);
  }
});

test("closing waits for the latest draft before invoking backend cleanup", async () => {
  const calls: string[] = [];
  let finish!: () => void;
  const saving = new Promise<void>((resolve) => { finish = resolve; });
  const close = handler("requestAppClose", "applyPlanSnapshot", {
    closePending: { current: false }, operationGate: { current: new EditorOperationGate() },
    setClosing: noop, setNotice: noop, toErrorMessage: String,
    flushCurrentDraft: async () => { calls.push("save"); await saving; },
    invoke: async (name: string) => { calls.push(name); },
  });
  const pending = close();
  assert.deepEqual(calls, ["save"]);
  finish();
  await pending;
  assert.deepEqual(calls, ["save", "close_app_after_cleanup"]);
});

test("cancelling during the save phase never dispatches start_render", async () => {
  const calls: string[] = [];
  let finish!: (value: object) => void;
  const save = new Promise<object>((resolve) => { finish = resolve; });
  const cancel = { current: false };
  const start = handler("startMovieRender", "cancelMovieRender", {
    videoOutputDirectory: null,
    result: { job_id: "A", replay: { playback_time_seconds: 600 } }, planReady: true,
    capabilities: { renderReady: true }, clipEdits: [{ startSeconds: 10, endSeconds: 20, includeInFinal: true }],
    isValidClipTimeRange, operationGate: { current: new EditorOperationGate() },
    activeJobRef: { current: "A" }, renderCancelRequested: cancel,
    setRendering: noop, setRenderError: noop, setRenderResult: noop, setRenderProgress: noop,
    setPlanFeedback: noop, saveEditPlan: () => save, setCapabilities: noop,
    invoke: async (name: string) => { calls.push(name); return null; },
  });
  const pending = start();
  cancel.current = true;
  finish({ selectedClipCount: 1 });
  await pending;
  assert.deepEqual(calls, ["get_capabilities"]);
});

function loadEnvironment(savedPlan: unknown, rejectLoad = false) {
  const observed = { ready: true, conflict: false, writes: 0, notice: "", result: null as unknown, applied: null as unknown };
  const empty = { clipEdits: [], selectedClipId: "", highlightHero: "mirana", highlightRuleIds: [], renderSettings: {} };
  const environment = {
    isTauriRuntime: true, createClipEdits: () => [],
    inferHighlightSelection: () => ({ hero: "mirana", ruleIds: [] }), defaultRenderSettings: {},
    createSavedDefaultPlan: (_summary: unknown, saved: { plans: Array<{ mode: string; clips: unknown[] }> }, fallback: unknown) => {
      const plan = saved.plans.find((entry) => entry.mode === "default");
      return plan ? { ...empty, clipEdits: plan.clips } : fallback;
    },
    invoke: async (name: string) => {
      if (name === "get_edit_plan") {
        if (rejectLoad) throw new Error("Unsupported saved schema; original retained");
        return savedPlan;
      }
      return null;
    },
    draftWriter: { current: new DraftWriter(async () => ++observed.writes) },
    closePending: { current: false }, setResult: (value: unknown) => { observed.result = value; },
    applyPlanSnapshot: (value: unknown) => { observed.applied = value; }, setMovieSetupOpen: noop,
    setPlanReady: (value: boolean) => { observed.ready = value; },
    setPlanConflict: (value: boolean) => { observed.conflict = value; }, setPlanFeedback: noop,
    setNotice: (value: { message: string }) => { observed.notice = value.message; },
    setRenderProgress: noop, setRenderResult: noop, setRenderError: noop,
    setRenderResume: noop, setResumeCheckError: noop,
  };
  return { observed, environment };
}

test("missing candidate IDs preserve the full draft and disable editing/autosave", async () => {
  const saved = { activeMode: "default", plans: [{ mode: "default", clips: [{ candidateId: "retired-event" }] }, { mode: "wtf", clips: [] }] };
  const { observed, environment } = loadEnvironment(saved);
  const load = handler("restoreAnalysisWorkspace", "flushCurrentDraft", environment);
  await load({ job_id: "A", highlights: { candidates: [{ id: "new-event" }] } }, null);
  assert.equal(observed.ready, false);
  assert.equal(observed.conflict, true);
  assert.equal(observed.writes, 0);
  assert.match(observed.notice, /保留完整旧草稿/);
});

test("saved-plan load failures never fall back to an autosavable default workspace", async () => {
  const { observed, environment } = loadEnvironment(null, true);
  const load = handler("restoreAnalysisWorkspace", "flushCurrentDraft", environment);
  await assert.rejects(load({ job_id: "A", highlights: { candidates: [] } }, null), /Unsupported saved schema/);
  assert.equal(observed.result, null);
  assert.equal(observed.writes, 0);
});

test("retired external-tool drafts cannot block or replace the default highlight plan", async () => {
  const saved = { activeMode: "wtf", plans: [
    { mode: "default", clips: [{ candidateId: "current-highlight" }] },
    { mode: "wtf", clips: [{ candidateId: "retired-story" }] },
  ] };
  const before = structuredClone(saved);
  const { observed, environment } = loadEnvironment(saved);
  const load = handler("restoreAnalysisWorkspace", "flushCurrentDraft", environment);
  await load({ job_id: "A", highlights: { candidates: [{ id: "current-highlight" }] } }, null);
  assert.equal(observed.ready, true);
  assert.equal(observed.conflict, false);
  assert.equal(observed.writes, 0);
  assert.deepEqual((observed.applied as { clipEdits: unknown[] }).clipEdits, saved.plans[0].clips);
  assert.deepEqual(saved, before);
});

function importedTakeFixture(): EditPlanClip[] {
  const shared = "c".repeat(64);
  return ["primary", "alternate"].map((role, index) => ({
    clipId: `clip-imported-${index}`, candidateId: `import-${shared}`,
    imported: { title: `示例故事${index ? "备用" : "主机位"}`, sourceId: (index ? "b" : "a").repeat(64), candidateSourceId: shared, anchorTick: 900, anchorSeconds: 30 },
    viewHero: index ? "npc_dota_hero_pudge" : "npc_dota_hero_mirana", cameraMode: index ? "hero_chase" : "player_perspective",
    takeGroupId: "example-scene", takeRole: role as "primary" | "alternate", includeInFinal: !index,
    durationMode: "short", storyPreRollSeconds: 60, shortSourceStartSeconds: 20, shortSourceEndSeconds: 35,
    sourceStartSeconds: 20, sourceEndSeconds: 35,
  }));
}

function importedEditorFunctions() {
  const normalizeUserCameraMode = handler("normalizeUserCameraMode", "cameraModeShortLabel", {});
  const createClipEdits = handler("createClipEdits", "heroHighlightCandidates", {
    frameRangeWithinReplay, normalizeStoryDurationClips, normalizeUserCameraMode, DEFAULT_STORY_PRE_ROLL_SECONDS: 60,
  });
  const planClips = handler("planClips", "shortDurationState", { roundFrame });
  const toEditPlanSlot = handler("toEditPlanSlot", "createClipEdits", { planClips });
  const copyImportedMetadata = handler("copyImportedMetadata", "normalizeUserCameraMode", {});
  return { createClipEdits, planClips, toEditPlanSlot, copyImportedMetadata };
}

test("validated 1.2 primary/alternate imports restore as renderable and keep every anchor through the save payload", async () => {
  const clips = importedTakeFixture();
  const saved = { activeMode: "default", plans: [{ mode: "default", clips }] };
  const { observed, environment } = loadEnvironment(saved);
  const { createClipEdits, toEditPlanSlot } = importedEditorFunctions();
  environment.createSavedDefaultPlan = (summary: unknown, plan: typeof saved) => ({
    clipEdits: createClipEdits(summary, plan.plans[0]), selectedClipId: clips[0]!.clipId, highlightHero: "",
    highlightRuleIds: [], renderSettings: {},
  });
  const load = handler("restoreAnalysisWorkspace", "flushCurrentDraft", environment);
  await load({ job_id: "A", replay: { playback_time_seconds: 600, players: [] }, highlights: { candidates: [] } }, null);
  assert.equal(observed.ready, true);
  assert.equal(observed.conflict, false);
  const payload = { jobId: "A", activeMode: "default", plans: [toEditPlanSlot("default", observed.applied)] };
  const received: unknown[] = [];
  const writer = new DraftWriter(async value => { received.push(value); return { selectedClipCount: 2, totalDurationSeconds: 15 }; });
  await writer.persist(payload);
  const serialized = (received[0] as typeof payload).plans[0]!.clips as EditPlanClip[];
  assert.deepEqual(serialized, clips);
  assert.equal(serialized[0]!.candidateId, serialized[1]!.candidateId);
  assert.notEqual(serialized[0]!.imported!.sourceId, serialized[1]!.imported!.sourceId);
  assert.deepEqual(serialized.map(clip => [clip.takeRole, clip.includeInFinal]), [["primary", true], ["alternate", false]]);
});

test("duplicating an imported take gives it its own source identity while retaining the shared candidate and anchor", () => {
  const sourceClips = importedTakeFixture();
  const { createClipEdits, planClips, copyImportedMetadata } = importedEditorFunctions();
  const editor = createClipEdits({ replay: { playback_time_seconds: 600, players: [] }, highlights: { candidates: [] } }, { clips: sourceClips });
  let next = editor;
  const duplicate = handler("duplicateClip", "deleteClip", {
    planReady: true, operationGate: { current: new EditorOperationGate() }, clipEdits: editor,
    createClipId: () => "clip-user-copy", copyImportedMetadata,
    setClipEdits: (value: unknown) => { next = value; }, setSelectedClipId: noop, setPlanFeedback: noop,
  });
  duplicate(editor[1].clipId);
  const copy = planClips(next)[2] as EditPlanClip;
  assert.equal(copy.candidateId, sourceClips[1]!.candidateId);
  assert.match(copy.imported!.sourceId, /^[a-f0-9]{64}$/);
  assert.notEqual(copy.imported!.sourceId, sourceClips[1]!.imported!.sourceId);
  assert.equal(copy.imported!.candidateSourceId, sourceClips[1]!.imported!.candidateSourceId);
  assert.deepEqual([copy.imported!.anchorTick, copy.imported!.anchorSeconds], [900, 30]);
  assert.deepEqual(planClips(next).slice(0, 2), sourceClips);
  const legacy: ImportedClip = { title: "旧版导入", sourceId: "d".repeat(64), anchorTick: 900, anchorSeconds: 30 };
  assert.equal(copyImportedMetadata(legacy).candidateSourceId, legacy.sourceId);
});

test("adding from an imported source preserves a new anchor identity; replacing its event clears metadata for the whole scene", () => {
  const { createClipEdits, copyImportedMetadata } = importedEditorFunctions();
  const summary = { replay: { playback_time_seconds: 600, players: [] }, highlights: { candidates: [] } };
  const editor = createClipEdits(summary, { clips: importedTakeFixture() });
  let next = editor;
  const add = handler("addClip", "activateTimelineCandidate", {
    planReady: true, operationGate: { current: new EditorOperationGate() }, result: summary, clipEdits: editor,
    selectedClipId: editor[0].clipId, replayEndFrame, roundFrame, DEFAULT_STORY_PRE_ROLL_SECONDS: 60,
    createClipId: () => "clip-user-added", copyImportedMetadata,
    setClipEdits: (value: unknown) => { next = value; }, setSelectedClipId: noop, setPlanFeedback: noop,
  });
  add();
  assert.equal(next[2].candidateId, editor[0].candidateId);
  assert.notEqual(next[2].imported.sourceId, editor[0].imported.sourceId);
  assert.equal(next[2].imported.anchorTick, 900);
  const update = handler("updateClipEdit", "extendCurrentKillSetups", {
    planReady: true, operationGate: { current: new EditorOperationGate() }, result: summary, normalizeStoryDurationClips,
    setClipEdits: (work: (value: unknown) => unknown) => { next = work(editor); }, setPlanFeedback: noop,
  });
  update(editor[0].clipId, { candidateId: "actual-highlight" });
  assert.ok(next.every((clip: {candidateId:string;imported:unknown}) => clip.candidateId === "actual-highlight" && clip.imported === null));
  assert.deepEqual(next.map((clip: {takeRole:string;includeInFinal:boolean}) => [clip.takeRole, clip.includeInFinal]), [["primary", true], ["alternate", false]]);
});

test("generated hero clips keep both live and short timecodes inside the replay", () => {
  const shortDurationState = (start: number, end: number) => ({ shortStartSeconds: start, shortEndSeconds: end });
  const createHero = handler("createHeroHighlightClips", "takeRoleLabel", {
    DEFAULT_HIGHLIGHT_RULE_IDS: [], frameRangeWithinReplay, shortDurationState,
    heroHighlightCandidates: () => [{ id: "c", start_seconds: 595, end_seconds: 600.025 }],
  });
  const heroClips = createHero({ replay: { playback_time_seconds: 600.025 } }, "mirana");
  for (const clip of heroClips) {
    assert.equal(clip.endSeconds, 600);
    assert.equal(clip.shortEndSeconds, 600);
  }
});

test("opening export saves the current draft before read-only checkpoint inspection", async () => {
  const calls: string[] = [];
  const gate = new EditorOperationGate();
  const openSetup = handler("openMovieSetup", "startMovieRender", {
    result: { job_id: "A" }, planReady: true, interactionBlocked: false, isTauriRuntime: true,
    operationGate: { current: gate }, activeJobRef: { current: "A" },
    setMovieSetupOpen: noop, setRenderResume: noop, setResumeCheckError: noop, setCheckingResume: noop,
    saveEditPlan: async () => { calls.push("save"); return {}; },
    refreshRenderResume: async (job: string) => { assert.ok(gate.blocked); calls.push(`inspect:${job}`); },
  });
  await openSetup();
  assert.deepEqual(calls, ["save", "inspect:A"]);
  assert.equal(gate.blocked, false);
});

test("a late checkpoint response cannot become the next job's resume action", async () => {
  let current = true;
  let shown: unknown = "old";
  let finish!: (value: object) => void;
  const pending = new Promise<object>((resolve) => { finish = resolve; });
  const refresh = handler("refreshRenderResume", "openMovieSetup", {
    setCheckingResume: noop, setResumeCheckError: noop,
    setRenderResume: (value: unknown) => { shown = value; }, invoke: () => pending, toErrorMessage: String,
  });
  const running = refresh("A", () => current);
  assert.equal(shown, null);
  current = false;
  finish({ fingerprint: "old-A" });
  await running;
  assert.equal(shown, null);
});

test("explicit resume sends its inspected fingerprint and failure refreshes the saved checkpoint", async () => {
  for (const fingerprint of [null, "verified-plan-A"]) {
    const calls: string[] = [];
    const gate = new EditorOperationGate();
    const start = handler("startMovieRender", "cancelMovieRender", {
    videoOutputDirectory: null,
      result: { job_id: "A", replay: { playback_time_seconds: 600 } }, planReady: true,
      capabilities: { renderReady: true }, clipEdits: [{ startSeconds: 10, endSeconds: 20, includeInFinal: true }],
      isValidClipTimeRange, operationGate: { current: gate }, activeJobRef: { current: "A" },
      renderCancelRequested: { current: false }, setRendering: noop, setRenderError: noop,
      setRenderResult: noop, setRenderProgress: noop, setRenderResume: noop, setPlanFeedback: noop,
      setCapabilities: noop, setNotice: noop, toErrorMessage: String, saveEditPlan: async () => ({}),
      Channel: class { onmessage: unknown },
      invoke: async (name: string, args: { resumeFingerprint?: string | null }) => {
        calls.push(name);
        if (name === "start_render") {
          assert.equal(args.resumeFingerprint, fingerprint);
          throw new Error("connection closed 10054");
        }
        return null;
      },
      refreshRenderResume: async (job: string) => {
        assert.ok(gate.blocked); calls.push(`inspect:${job}`);
      },
    });
    await start(fingerprint);
    assert.deepEqual(calls, ["start_render", "inspect:A", "get_capabilities"]);
    assert.equal(gate.blocked, false);
  }
});
