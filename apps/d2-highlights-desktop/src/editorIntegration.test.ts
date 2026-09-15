import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import test from "node:test";
import { DraftWriter, EditorOperationGate } from "./editorSession.ts";
import { frameRangeWithinReplay, isValidClipTimeRange } from "./frameTime.ts";

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

test("a successful analysis package remains successful when opening its folder fails", async () => {
  let notice: { kind: string; title: string; message: string } | null = null;
  const calls: string[] = [];
  const run = handler("exportAnalysisPackage", "updateClipEdit", {
    isTauriRuntime: true, result: { job_id: "A" }, highlightHero: "mirana", exportingAnalysis: false,
    operationGate: { current: new EditorOperationGate() }, setExportingAnalysis: noop, setError: noop,
    flushCurrentDraft: async () => undefined,
    open: async () => "D:/exports", preferredAnalysisExportDirectory: noop,
    analysisExportDirectory: "", capabilities: {}, setAnalysisExportDirectory: noop, heroLabel: String,
    invoke: async (name: string) => {
      calls.push(name);
      if (name === "export_replay_analysis_package") return { packagePath: "D:/exports/package", eventCount: 1, incidentCount: 1 };
      throw new Error("Explorer unavailable");
    },
    setNotice: (value: typeof notice) => { notice = value; }, toErrorMessage: String,
  });
  await run();
  assert.deepEqual(calls, ["export_replay_analysis_package", "open_exported_analysis_package"]);
  assert.equal(notice?.kind, "success");
  assert.match(notice?.message ?? "", /D:\/exports\/package/);
});

test("analysis export waits for pending draft persistence before acquiring native work", async () => {
  const calls: string[] = [];
  let finish!: () => void;
  const saving = new Promise<void>((resolve) => { finish = resolve; });
  const run = handler("exportAnalysisPackage", "updateClipEdit", {
    isTauriRuntime: true, result: { job_id: "A" }, highlightHero: "mirana", exportingAnalysis: false,
    operationGate: { current: new EditorOperationGate() }, setExportingAnalysis: noop, setError: noop,
    flushCurrentDraft: async () => { calls.push("save"); await saving; },
    open: async () => { calls.push("choose"); return "D:/exports"; }, preferredAnalysisExportDirectory: noop,
    analysisExportDirectory: "", capabilities: {}, setAnalysisExportDirectory: noop, heroLabel: String,
    invoke: async (name: string) => {
      calls.push(name);
      return { packagePath: "D:/exports/package", eventCount: 1, incidentCount: 1 };
    },
    setNotice: noop, toErrorMessage: String,
  });
  const pending = run();
  assert.deepEqual(calls, ["save"]);
  finish();
  await pending;
  assert.deepEqual(calls, ["save", "choose", "export_replay_analysis_package", "open_exported_analysis_package"]);
});

function loadEnvironment(savedPlan: unknown, rejectLoad = false) {
  const observed = { ready: true, conflict: false, writes: 0, notice: "", result: null as unknown };
  const empty = { clipEdits: [], selectedClipId: "", highlightHero: "mirana", highlightRuleIds: [], sourceStoryId: null, renderSettings: {} };
  const old = { ...empty, clipEdits: [{ candidateId: "retired-event" }] };
  const environment = {
    isTauriRuntime: true, createClipEdits: () => [],
    inferHighlightSelection: () => ({ hero: "mirana", ruleIds: [] }), defaultRenderSettings: {},
    createEmptyPlanSnapshot: () => empty, createSavedPlanWorkspace: () => ({ default: old, wtf: empty }),
    invoke: async (name: string) => {
      if (name === "get_edit_plan") {
        if (rejectLoad) throw new Error("Unsupported saved schema; original retained");
        return savedPlan;
      }
      return null;
    },
    draftWriter: { current: new DraftWriter(async () => ++observed.writes) },
    closePending: { current: false }, setResult: (value: unknown) => { observed.result = value; },
    setPlanSlots: noop, setEditorMode: noop, applyPlanSnapshot: noop, setMovieSetupOpen: noop,
    setPlanReady: (value: boolean) => { observed.ready = value; },
    setPlanConflict: (value: boolean) => { observed.conflict = value; }, setPlanFeedback: noop,
    setNotice: (value: { message: string }) => { observed.notice = value.message; },
    setRenderProgress: noop, setRenderResult: noop, setRenderError: noop,
    setRenderResume: noop, setResumeCheckError: noop,
  };
  return { observed, environment };
}

test("missing candidate IDs preserve the full draft and disable editing/autosave", async () => {
  const saved = { activeMode: "default", plans: [{ clips: [{ candidateId: "retired-event" }] }, { clips: [] }] };
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

test("generated hero and story clips keep both live and short timecodes inside the replay", () => {
  const shortDurationState = (start: number, end: number) => ({ shortStartSeconds: start, shortEndSeconds: end });
  const createHero = handler("createHeroHighlightClips", "createStoryClips", {
    DEFAULT_HIGHLIGHT_RULE_IDS: [], frameRangeWithinReplay, shortDurationState,
    heroHighlightCandidates: () => [{ id: "c", start_seconds: 595, end_seconds: 600.025 }],
  });
  const createStory = handler("createStoryClips", "storyDefaultDuration", { frameRangeWithinReplay, shortDurationState });
  const heroClips = createHero({ replay: { playback_time_seconds: 600.025 } }, "mirana");
  const storyClips = createStory({ primary_hero: "mirana", shots: [{ id: "s", order: 1, candidate_id: "c",
    source_start_seconds: 595, source_end_seconds: 600.025 }] }, 600.025);
  for (const clip of [...heroClips, ...storyClips]) {
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
