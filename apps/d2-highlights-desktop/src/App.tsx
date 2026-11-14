import { getVersion } from "@tauri-apps/api/app";
import { CacheCleanupPanel } from "./CacheCleanupPanel";
import { VideoOutputLocation } from "./VideoOutputLocation";
import { VideoOutputSetting } from "./VideoOutputSetting";
import { ExternalToolDialog } from "./ExternalToolDialog";
import { CameraModeDiagram } from "./CameraModeDiagram";
import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DraftWriter, EditorOperationGate } from "./editorSession";
import { frameRangeWithinReplay, isValidClipTimeRange, replayEndFrame, roundFrame } from "./frameTime";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Bell,
  Camera,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CircleAlert,
  Clapperboard,
  Clock3,
  Copy,
  Download,
  Eye,
  FileUp,
  FileText,
  FileVideo2,
  Film,
  FolderOpen,
  Gamepad2,
  Library,
  ListFilter,
  ListChecks,
  LoaderCircle,
  Minus,
  Pause,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  SearchX,
  Settings2,
  ShieldCheck,
  SkipBack,
  SkipForward,
  SlidersHorizontal,
  Sparkles,
  Square,
  Swords,
  Trash2,
  UserRound,
  Video,
  X,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  UPDATE_POLICY,
  appUpdateCopy,
  type AppUpdateEvent,
  type AppUpdateState,
} from "./appUpdater";
import mascot from "./assets/cat-editor-mascot.png";
import {
  TIMELINE_LANES,
  buildTimelineEvents,
  timelinePercent,
  timelineTicks,
} from "./eventTimeline";
import type { TimelineScope } from "./eventTimeline";
import { heroLabel } from "./heroes";
import { extendKillSetupClips } from "./extendKillSetup";
import {
  DEFAULT_HIGHLIGHT_RULE_IDS,
  HERO_KILL_RULE_ID,
  HIGHLIGHT_RULES,
  filterHeroHighlightCandidates,
  formatHighlightRuleSelection,
  inferHighlightSelection,
  normalizeHighlightRuleIds,
} from "./highlightRules";
import type { HighlightRuleId } from "./highlightRules";
import {
  DEFAULT_STORY_PRE_ROLL_SECONDS,
  MAX_CLIP_DURATION_SECONDS,
  MAX_STORY_PRE_ROLL_SECONDS,
  STORY_POST_ROLL_SECONDS,
  killSequenceRange,
  normalizeStoryDurationClips,
} from "./storyDuration";
import type {
  AnalysisProgress,
  AnalysisSummary,
  Capabilities,
  ClipCameraMode,
  ClipDurationMode,
  EditPlanSlot,
  EditPlanClip,
  HighlightCandidate,
  ImportedClip,
  LoadedEditPlan,
  RecentJob,
  ReplayLookupResult,
  ReplayLibraryFile,
  ReplayLibrarySnapshot,
  ReplayPlayer,
  RenderDiagnosticArchive,
  RenderProgress,
  RenderResult,
  RenderResumeSummary,
  RenderSettings,
  SaveEditPlanResult,
  StageStatus,
  StoryTakeRole,
} from "./types";
import { presentRenderError, renderErrorReport } from "./renderError";
import { RenderDiagnosticExporter } from "./diagnosticExport";
import { resumeAction } from "./renderResume";
import { canLaunchExternalTool, externalToolNotice } from "./externalTools";
import type { ExternalToolId } from "./externalTools";
import { displayReplayDirectory, formatReplayDownloadTime, rememberResolvedReplayDirectory, replayTimeDescription, ReplayLibraryRefresher } from "./replayLibrary";
import "./replayLibrary.css";

type View = "workbench" | "library";
type TitlebarPanel = "notifications" | "settings" | null;
type InspectorTab = "hero" | "edit" | "camera";

interface AppNotice {
  kind: "success" | "error";
  title: string;
  message: string;
}

interface ClipEditState {
  clipId: string;
  candidateId: string;
  imported?: ImportedClip | null;
  viewHero: string;
  cameraMode: ClipCameraMode;
  takeGroupId: string | null;
  takeRole: StoryTakeRole;
  includeInFinal: boolean;
  durationMode: ClipDurationMode;
  storyPreRollSeconds: number;
  shortStartSeconds: number;
  shortEndSeconds: number;
  startSeconds: number;
  endSeconds: number;
}

type ClipEdits = ClipEditState[];

interface EditorPlanSnapshot {
  clipEdits: ClipEdits;
  selectedClipId: string;
  highlightHero: string;
  highlightRuleIds: HighlightRuleId[];
  renderSettings: RenderSettings;
}

interface EditPlanWorkspaceRequest {
  jobId: string;
  activeMode: "default";
  plans: EditPlanSlot[];
}

interface ReanalysisReset {
  hero: string;
  ruleIds: HighlightRuleId[];
  renderSettings: RenderSettings;
}

const isTauriRuntime = "__TAURI_INTERNALS__" in window;
const appWindow = isTauriRuntime ? getCurrentWindow() : null;
const developmentFixtureEnabled =
  import.meta.env.DEV &&
  new URLSearchParams(window.location.search).get("fixture") === "mirana";
const replayDirectoryStorageKey = "cat-cut-replay-directory";
const fallbackAppVersion = "2.5.0";

const emptyCapabilities: Capabilities = {
  analysisReady: true,
  renderReady: false,
  ffmpegFound: false,
  ffprobeFound: false,
  dota2Found: false,
  renderReason: null,
  jobsRoot: "",
  recommendedReplayDirectory: null,
  dota2Path: null,
  dota2PathSource: "missing",
};

const defaultRenderSettings: RenderSettings = {
  cameraStyle: "auto_director",
  cleanHud: true,
  slowMotion: false,
  replayEmphasis: false,
  bgmMode: "game_only",
  customBgmPath: null,
  gameAudioVolume: 1,
  bgmVolume: 0,
  impactSfx: false,
  systemNarration: false,
};

const pipelineStages = [
  { id: "ingest", label: "校验" },
  { id: "parse", label: "解析" },
  { id: "detect", label: "高光" },
] as const;

function App() {

  const [view, setView] = useState<View>("workbench");
  const [capabilities, setCapabilities] =
    useState<Capabilities>(emptyCapabilities);
  const [recentJobs, setRecentJobs] = useState<RecentJob[]>([]);
  const [selectedPath, setSelectedPath] = useState("");
  const [result, setResult] = useState<AnalysisSummary | null>(null);
  const [selectedClipId, setSelectedClipId] = useState("");
  const [progress, setProgress] = useState<AnalysisProgress | null>(null);
  const [completedStages, setCompletedStages] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const [error, setError] = useState("");
  const [dragActive, setDragActive] = useState(false);
  const [notice, setNotice] = useState<AppNotice | null>(null);
  const [externalNotice, setExternalNotice] = useState<ReturnType<typeof externalToolNotice> | null>(null);
  const [appVersion, setAppVersion] = useState(fallbackAppVersion);
  const [appUpdate, setAppUpdate] = useState<AppUpdateState>(
    isTauriRuntime ? { phase: "checking" } : { phase: "unavailable" },
  );
  const [clipEdits, setClipEdits] = useState<ClipEdits>([]);
  const [highlightHero, setHighlightHero] = useState("");
  const [highlightRuleIds, setHighlightRuleIds] = useState<HighlightRuleId[]>(
    () => [...DEFAULT_HIGHLIGHT_RULE_IDS],
  );
  const [movieSetupOpen, setMovieSetupOpen] = useState(false);
  const [savingPlan, setSavingPlan] = useState(false);
  const [planFeedback, setPlanFeedback] = useState("");
  const [renderSettings, setRenderSettings] = useState<RenderSettings>(
    defaultRenderSettings,
  );

  const [rendering, setRendering] = useState(false);
  const renderCancelRequested = useRef(false);
  const [renderProgress, setRenderProgress] = useState<RenderProgress | null>(
    null,
  );
  const [renderResult, setRenderResult] = useState<RenderResult | null>(null);
  const [renderError, setRenderError] = useState("");
  const [videoOutputDirectory, setVideoOutputDirectory] = useState<string|null>(null);
  useEffect(()=>setVideoOutputDirectory(null),[result?.job_id]);
  const [renderResume, setRenderResume] = useState<RenderResumeSummary | null>(null);
  const [checkingResume, setCheckingResume] = useState(false);
  const [resumeCheckError, setResumeCheckError] = useState("");
  const [completionNoticeEnabled, setCompletionNoticeEnabled] = useState(
    () => localStorage.getItem("cat-cut-completion-notice") !== "disabled",
  );
  const [importDialogOpen, setImportDialogOpen] = useState(false);
  const [replayDirectory, setReplayDirectory] = useState(
    () => localStorage.getItem(replayDirectoryStorageKey) ?? "",
  );
  const [replayLibrary, setReplayLibrary] = useState<ReplayLibrarySnapshot | null>(null);
  const [replayLibraryLoading, setReplayLibraryLoading] = useState(false);
  const [replayLibraryError, setReplayLibraryError] = useState("");
  const replayLibraryRefresh = useRef<(() => Promise<void>) | null>(null);
  const replayLibraryReadStarted = useRef(false);
  const replayLibraryQueryKey = `${capabilities.dota2Path ?? ""}\0${replayDirectory.trim()}`;
  const replayLibraryQueryRef = useRef(replayLibraryQueryKey);
  replayLibraryQueryRef.current = replayLibraryQueryKey;
  const [replayId, setReplayId] = useState("");
  const [replayLookupBusy, setReplayLookupBusy] = useState(false);
  const [replayLookupError, setReplayLookupError] = useState("");
  const [planReady, setPlanReady] = useState(false);
  const [planConflict, setPlanConflict] = useState(false);
  const [closing, setClosing] = useState(false);
  const operationGate = useRef(new EditorOperationGate());
  const closePending = useRef(false);
  const activeJobRef = useRef<string | null>(null);
  activeJobRef.current = result?.job_id ?? null;
  const updating = appUpdate.phase === "downloading" || appUpdate.phase === "installing";
  const interactionBlocked = busy || rendering || checkingResume || updating || closing;
  const draftWriter = useRef(new DraftWriter<EditPlanWorkspaceRequest, SaveEditPlanResult>(
    (request) => invoke<SaveEditPlanResult>("save_edit_plan", { request }),
  ));
  const draftRequest = useMemo<EditPlanWorkspaceRequest | null>(() => {
    if (!result || !planReady) return null;
    return { jobId: result.job_id, activeMode: "default", plans: [toEditPlanSlot("default", {
      clipEdits, selectedClipId, highlightHero, highlightRuleIds, renderSettings,
    })] };
  }, [result, planReady, clipEdits, selectedClipId, highlightHero, highlightRuleIds, renderSettings]);
  const latestDraftRef = useRef(draftRequest);
  latestDraftRef.current = draftRequest;
  const autoSaveTimerRef = useRef<number | null>(null);
  const closeHandlerRef = useRef(requestAppClose);
  closeHandlerRef.current = requestAppClose;

  useEffect(() => {
    if (!isTauriRuntime || !draftRequest) return;
    const snapshot = draftRequest;
    const timer = window.setTimeout(() => {
      autoSaveTimerRef.current = null;
      void draftWriter.current.persist(snapshot).then(() => {
        if (latestDraftRef.current === snapshot) setPlanFeedback("剪辑草稿已自动保存。");
      }).catch((reason: unknown) => {
        if (latestDraftRef.current === snapshot) {
          const message = `草稿未保存：${toErrorMessage(reason)}`;
          setPlanFeedback(message);
          setNotice({ kind: "error", title: "剪辑草稿保存失败", message });
        }
      });
    }, 400);
    autoSaveTimerRef.current = timer;
    return () => {
      window.clearTimeout(timer);
      if (autoSaveTimerRef.current === timer) autoSaveTimerRef.current = null;
    };
  }, [draftRequest]);

  useEffect(() => {
    if (!appWindow) return;
    let disposed = false;
    const stops: Array<() => void> = [];
    const register = (promise: Promise<() => void>) => {
      void promise.then((stop) => disposed ? stop() : stops.push(stop))
        .catch((reason: unknown) => setError(toErrorMessage(reason)));
    };
    register(appWindow.onCloseRequested((event) => {
      event.preventDefault();
      void closeHandlerRef.current();
    }));
    register(listen("app-close-requested", () => void closeHandlerRef.current()));
    return () => { disposed = true; stops.forEach((stop) => stop()); };
  }, []);

  useEffect(() => {
    if (!developmentFixtureEnabled) {
      return;
    }
    let disposed = false;
    void import("./devFixture").then(({ miranaDevelopmentSummary }) => {
      if (!disposed) {
        void restoreAnalysisWorkspace(miranaDevelopmentSummary, null).then(() => {
          if (!disposed) {
            setSelectedPath(miranaDevelopmentSummary.source.path);
            const resumeFixture = new URLSearchParams(window.location.search).get("resume");
            const exportLayoutFixture = new URLSearchParams(window.location.search).get("exportLayout");
            if (exportLayoutFixture === "ready" || exportLayoutFixture === "complete") {
              setCapabilities({ ...emptyCapabilities, renderReady: true });
              setMovieSetupOpen(true);
              setPlanFeedback("剪辑草稿已保存，可开始导出。");
              if (exportLayoutFixture === "complete") {
                setRenderResult({ outputPath: "fixture/export.mp4", qcReportPath: "fixture/quality.json",
                  sourceAssetsDir: "fixture/assets", sourceAssets: [], durationSeconds: 30,
                  width: 1920, height: 1080, segmentCount: 2, sourceAssetCount: 2,
                  warnings: ["素材文件已保存，请查看视频确认目标英雄和事件。"] });
              }
            }
            if (resumeFixture === "interrupted" || resumeFixture === "compose") {
              setClipEdits((existing) => Array.from({ length: 25 }, (_, index) => ({
                ...existing[0]!, clipId: `fixture-resume-${index}`, takeGroupId: null,
              })));
              const complete = resumeFixture === "compose";
              setRenderResume({ fingerprint: "development-fixture", totalClips: 25,
                completedClips: complete ? 25 : 22, remainingClips: complete ? 0 : 3,
                firstPendingClip: complete ? null : 23, unverifiedCameraClips: complete ? 25 : 22,
                completedDirectory: "fixture/segments" });
              setCapabilities({ ...emptyCapabilities, renderReady: true, videoOutputDirectory: JSON.parse(localStorage.getItem("cat-cut-dev-video-output")||"null") });
              setRenderError("Dota 2 回放控制失败：VConsole I/O error: 远程主机强迫关闭了一个现有的连接。 (os error 10054)");
              setMovieSetupOpen(true);
            }
            if (new URLSearchParams(window.location.search).get("exportProgress") === "native") {
              setRenderProgress({ stage: "capture", status: "running",
                message: "第 1/25 段 · 已保存 7.5/16.0 秒 · 60 FPS",
                percent: 12, currentClip: 1, totalClips: 25 });
              setMovieSetupOpen(true);
            }
            if (new URLSearchParams(window.location.search).get("exportError") === "seek") {
              setRenderError("Dota 2 回放控制失败：回放跳转未确认：未能读取录像当前播放位置，本次导出已停止。\n目标位置：9000 tick；最近收到的位置：未收到。\n请保留本次控制日志以便排查。");
              setMovieSetupOpen(true);
            }
          }
        });
      }
    });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!isTauriRuntime) {
      return;
    }
    void Promise.all([
      invoke<Capabilities>("get_capabilities"),
      invoke<RecentJob[]>("get_recent_jobs"),
      getVersion(),
      invoke<string|null>("get_startup_replay"),
    ])
      .then(([capabilityResult, jobs, version, startupReplay]) => {
        setCapabilities(capabilityResult);
        setRecentJobs(jobs);
        setAppVersion(version);
        if (startupReplay) { setSelectedPath(startupReplay); void runAnalysis(startupReplay); }
        setReplayDirectory(
          (current) =>
            current.trim() ||
            capabilityResult.recommendedReplayDirectory ||
            "",
        );
      })
      .catch((reason: unknown) => setError(toErrorMessage(reason)));
  }, []);

  useEffect(() => {
    if (!isTauriRuntime || !UPDATE_POLICY.checkOnStartup) {
      return;
    }
    const timer = window.setTimeout(() => {
      void checkForAppUpdate(false);
    }, 1_200);
    return () => window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    localStorage.setItem(
      "cat-cut-completion-notice",
      completionNoticeEnabled ? "enabled" : "disabled",
    );
  }, [completionNoticeEnabled]);

  useEffect(() => {
    const directory = replayDirectory.trim();
    if (directory) {
      localStorage.setItem(replayDirectoryStorageKey, directory);
    } else {
      localStorage.removeItem(replayDirectoryStorageKey);
    }
  }, [replayDirectory]);

  useEffect(() => {
    replayLibraryReadStarted.current = false;
    setReplayLibrary(null);
    setReplayLibraryLoading(false);
    setReplayLibraryError("");
    if ((!isTauriRuntime && !developmentFixtureEnabled) ||
        (!developmentFixtureEnabled && !replayDirectory.trim() && !capabilities.dota2Path)) {
      replayLibraryRefresh.current = null;
      return;
    }
    const refresh = new ReplayLibraryRefresher(
      async () => {
        if (developmentFixtureEnabled) {
          const { replayLibraryDevelopmentSnapshot } = await import("./devReplayLibrary");
          return replayLibraryDevelopmentSnapshot(replayDirectory.trim());
        }
        return invoke<ReplayLibrarySnapshot>("list_replays", { directory: replayDirectory.trim() || null });
      },
      (snapshot) => {
        if (replayLibraryQueryRef.current !== replayLibraryQueryKey) return;
        setReplayLibrary(snapshot);
        setReplayLibraryError("");
        setReplayDirectory((current) => rememberResolvedReplayDirectory(current, snapshot.directory));
      },
      (reason) => {
        if (replayLibraryQueryRef.current !== replayLibraryQueryKey) return;
        setReplayLibrary(null); setReplayLibraryError(toErrorMessage(reason));
      },
      (loading) => { if (replayLibraryQueryRef.current === replayLibraryQueryKey) setReplayLibraryLoading(loading); },
    );
    replayLibraryRefresh.current = () => refresh.refresh();
    return () => { refresh.dispose(); replayLibraryRefresh.current = null; };
  }, [replayDirectory, capabilities.dota2Path]);

  useEffect(() => {
    if (view !== "library" || replayLibraryReadStarted.current) return;
    if (replayLibraryRefresh.current) {
      // One initial read per directory. Later reads require the refresh button.
      replayLibraryReadStarted.current = true;
      void replayLibraryRefresh.current();
    } else setReplayLibraryError(isTauriRuntime
      ? "请先选择录像目录，或在设置中指定 Dota 2 位置。"
      : "请在桌面应用中读取本地录像目录。");
  }, [view, replayDirectory, capabilities.dota2Path]);

  useEffect(() => {
    if (!isTauriRuntime) {
      return;
    }

    let disposed = false;
    let unlisten: (() => void) | undefined;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragActive(true);
          return;
        }

        setDragActive(false);
        if (event.payload.type !== "drop") {
          return;
        }

        const paths = event.payload.paths;
        const path = paths[0];
        if (paths.length !== 1 || !path || !isDemPath(path)) {
          setError("一次请拖入一个 Dota 2 .dem 录像文件。");
          return;
        }
        if (operationGate.current.blocked) {
          setError("当前任务尚未结束，请完成后再导入下一份。");
          return;
        }

        void runAnalysis(path);
      })
      .then((stopListening) => {
        if (disposed) {
          stopListening();
        } else {
          unlisten = stopListening;
        }
      })
      .catch((reason: unknown) => setError(toErrorMessage(reason)));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [interactionBlocked,result]);

  useEffect(() => {
    const first = clipEdits[0];
    if (first) {
      setSelectedClipId((current) =>
        clipEdits.some((clip) => clip.clipId === current)
          ? current
          : first.clipId,
      );
    } else {
      setSelectedClipId("");
    }
  }, [clipEdits]);

  async function restoreAnalysisWorkspace(summary: AnalysisSummary, reset: ReanalysisReset | null) {
    const initialClips = reset
      ? createHeroHighlightClips(summary, reset.hero, reset.ruleIds)
      : createClipEdits(summary);
    const selection = reset ? { hero: reset.hero, ruleIds: reset.ruleIds }
      : inferHighlightSelection(summary.highlights.candidates, initialClips);
    const initialSnapshot: EditorPlanSnapshot = {
      clipEdits: initialClips, selectedClipId: initialClips[0]?.clipId ?? "",
      highlightHero: selection.hero, highlightRuleIds: selection.ruleIds,
      renderSettings: reset?.renderSettings ?? defaultRenderSettings,
    };
    let snapshot = initialSnapshot;
    let latest: RenderResult | null = null;
    let conflictingCandidates: string[] = [];
    if (isTauriRuntime) {
      const [saved, previousRender] = await Promise.all([
        reset ? Promise.resolve(null) : invoke<LoadedEditPlan | null>("get_edit_plan", { jobId: summary.job_id }),
        invoke<RenderResult | null>("get_latest_render", { jobId: summary.job_id }).catch(() => null),
      ]);
      if (saved) {
        snapshot = createSavedDefaultPlan(summary, saved, initialSnapshot);
        const candidateIds = new Set(summary.highlights.candidates.map((candidate) => candidate.id));
        const savedDefault = saved.plans.find((plan) => plan.mode === "default");
        conflictingCandidates = [...new Set((savedDefault?.clips ?? [])
          .filter((clip) => !clip.imported && !candidateIds.has(clip.candidateId)).map((clip) => clip.candidateId))];
      }
      latest = reset ? null : previousRender;
      if (reset) {
        draftWriter.current.invalidateSavedSnapshot();
        await draftWriter.current.persist({ jobId: summary.job_id, activeMode: "default",
          plans: [toEditPlanSlot("default", snapshot)] });
      }
    }
    if (closePending.current) return;
    setResult(summary);
    applyPlanSnapshot(snapshot);
    setMovieSetupOpen(false);
    const conflict = conflictingCandidates.length > 0;
    setPlanReady(!conflict);
    setPlanConflict(conflict);
    setPlanFeedback(conflict ? "候选事件已变化，旧草稿保持原样。请点击重新分析，确认备份后重建。" :
      reset ? "已恢复当前主角的完整高光，重新分析前的剪辑草稿已备份。" : "");
    if (conflict) setNotice({ kind: "error", title: "剪辑草稿需要重新确认",
      message: `${conflictingCandidates.length} 个旧候选无法对应。已保留完整旧草稿并暂停编辑和自动保存；请点击重新分析，确认备份后重建。` });
    setRenderProgress(null);
    setRenderResult(latest);
    setRenderError("");
    setRenderResume(null);
    setResumeCheckError("");
    return !conflict;
  }

  async function flushCurrentDraft() {
    if (!isTauriRuntime) return null;
    if (autoSaveTimerRef.current !== null) {
      window.clearTimeout(autoSaveTimerRef.current);
      autoSaveTimerRef.current = null;
    }
    return draftWriter.current.flush(latestDraftRef.current);
  }

  async function requestAppClose() {
    if (closePending.current) return;
    closePending.current = true;
    operationGate.current.setClosing(true);
    setClosing(true);
    try {
      await flushCurrentDraft();
      await invoke("close_app_after_cleanup");
    } catch (reason: unknown) {
      setNotice({ kind: "error", title: "暂时无法关闭", message: toErrorMessage(reason) });
      closePending.current = false;
      operationGate.current.setClosing(false);
      setClosing(false);
    }
  }

  function applyPlanSnapshot(snapshot: EditorPlanSnapshot) {
    setClipEdits(snapshot.clipEdits);
    setSelectedClipId(snapshot.selectedClipId);
    setHighlightHero(snapshot.highlightHero);
    setHighlightRuleIds(snapshot.highlightRuleIds);
    setRenderSettings(snapshot.renderSettings);
  }

  const selectedClip = useMemo(
    () => clipEdits.find((clip) => clip.clipId === selectedClipId) ?? null,
    [clipEdits, selectedClipId],
  );

  const selectedCandidate = useMemo(
    () =>
      result?.highlights.candidates.find(
        (candidate) => candidate.id === selectedClip?.candidateId,
      ) ?? null,
    [result, selectedClip],
  );

  async function openExternalTool(id: ExternalToolId) {
    if (operationGate.current.blocked) return;
    const notice = externalToolNotice(id);
    if (!canLaunchExternalTool(id) || !isTauriRuntime) {
      setExternalNotice(notice);
      return;
    }
    try {
      const opened = await invoke<{ status: "opened" | "missing" }>("open_external_tool", { toolId: id });
      setExternalNotice(opened.status === "opened" ? null : notice);
    } catch (reason: unknown) {
      setExternalNotice({ title: "暂时无法打开独立软件", message: toErrorMessage(reason) });
    }
  }

  function openImportDialog() {
    if (operationGate.current.blocked) return;
    setError("");
    setReplayLookupError("");
    setImportDialogOpen(true);
  }

  function rememberReplayPath(path: string) {
    setReplayDirectory(parentPath(path));
    const match = fileName(path).match(/^(\d{1,20})\.dem$/i);
    if (match?.[1]) {
      setReplayId(match[1]);
    }
  }

  async function chooseReplayDirectory() {
    if (!isTauriRuntime) {
      setReplayLookupError("请从桌面应用中选择录像目录。");
      if (view === "library") setReplayLibraryError("请从桌面应用中选择录像目录。");
      return;
    }
    setReplayLookupError("");
    try {
      const picked = await open({
        multiple: false,
        directory: true,
        title: "选择 Dota 2 录像目录",
        defaultPath: replayDirectory || undefined,
      });
      if (typeof picked === "string") {
        setReplayDirectory(picked);
        setReplayLibraryError("");
        if (picked === replayDirectory) void replayLibraryRefresh.current?.();
      }
    } catch (reason: unknown) {
      setReplayLookupError(toErrorMessage(reason));
      if (view === "library") setReplayLibraryError(toErrorMessage(reason));
    }
  }

  async function chooseReplayFile() {
    if (operationGate.current.blocked) return;
    setReplayLookupError("");
    if (!isTauriRuntime) {
      setReplayLookupError("请从桌面应用中选择录像文件。");
      return;
    }
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        title: "选择 Dota 2 录像",
        defaultPath: replayDirectory || undefined,
        filters: [{ name: "Dota 2 录像", extensions: ["dem"] }],
      });
      if (typeof picked === "string") {
        await runAnalysis(picked);
      }
    } catch (reason: unknown) {
      setReplayLookupError(toErrorMessage(reason));
    }
  }

  async function loadReplayById() {
    if (operationGate.current.blocked || replayLookupBusy) {
      return;
    }
    if (!isTauriRuntime) {
      setReplayLookupError("请从桌面应用中按编号载入录像。");
      return;
    }

    setReplayLookupBusy(true);
    setReplayLookupError("");
    try {
      const resolved = await invoke<ReplayLookupResult>(
        "resolve_replay_by_id",
        {
          replayDirectory,
          replayId,
        },
      );
      await runAnalysis(resolved.path);
    } catch (reason: unknown) {
      setReplayLookupError(toErrorMessage(reason));
    } finally {
      setReplayLookupBusy(false);
    }
  }

  async function runAnalysis(path = selectedPath, resetEditPlan = false) {
    if (!path) return;
    const token = operationGate.current.begin("analysis");
    if (!token) return;
    const reset: ReanalysisReset | null = resetEditPlan ? {
      hero: highlightHero || (result ? inferHighlightSelection(result.highlights.candidates, clipEdits).hero : "") ||
        result?.replay.players[0]?.hero_name || "",
      ruleIds: normalizeHighlightRuleIds(highlightRuleIds), renderSettings,
    } : null;

    setBusy(true);
    setError("");
    try {
      // Flush the current in-memory plan before changing paths or archiving it.
      await flushCurrentDraft();
      if (!operationGate.current.current(token)) return;
      setSelectedPath(path);
      rememberReplayPath(path);
      setImportDialogOpen(false);
      setView("workbench");
      setPlanReady(false);
      setResult(null);
      setMovieSetupOpen(false);
      setCompletedStages([]);
      setProgress({ stage: "ingest", status: "running", message: "正在准备分析" });
      const channel = new Channel<AnalysisProgress>();
      channel.onmessage = (message) => {
        if (!operationGate.current.current(token)) return;
        setProgress(message);
        if (message.status === "complete") setCompletedStages((current) =>
          current.includes(message.stage) ? current : [...current, message.stage]);
      };
      const summary = await invoke<AnalysisSummary>("analyze_dem", {
        demPath: path, resetEditPlan, onProgress: channel,
      });
      if (!operationGate.current.current(token)) return;
      const restored = await restoreAnalysisWorkspace(summary, reset);
      if (!operationGate.current.current(token)) return;
      setProgress({ stage: "complete", status: "complete",
        message: summary.reused_existing_job ? "已载入之前的分析结果" : "录像分析完成" });
      const jobs = await invoke<RecentJob[]>("get_recent_jobs").catch(() => null);
      if (!operationGate.current.current(token)) return;
      if (jobs) setRecentJobs(jobs);
      if (completionNoticeEnabled && restored) setNotice({ kind: "success", title: reset ? "重新分析完成" : "录像分析完成",
        message: reset ? "完整高光已恢复，重新分析前的剪辑草稿已备份。" :
          `${fileName(path)} · ${summary.highlights.candidates.filter((candidate) => candidate.kind === "hero_kill_sequence").length} 段英雄击杀 · 可按英雄筛选` });
    } catch (reason: unknown) {
      if (operationGate.current.current(token)) {
        const message = toErrorMessage(reason);
        setError(message);
        setNotice({ kind: "error", title: "任务未能打开", message });
        // A failed pre-switch save keeps the previous editable workspace intact.
        if (!latestDraftRef.current) setProgress({ stage: "parse", status: "failed", message });
      }
    } finally {
      operationGate.current.finish(token);
      setBusy(false);
    }
  }

  function confirmReanalysis() {
    if (!result) {
      void runAnalysis();
      return;
    }
    const confirmed = window.confirm(
      planConflict
        ? "旧候选事件已变化。将先备份完整旧草稿，再重建当前主角的高光。是否继续？"
        : "重新分析会先保存并备份当前剪辑草稿，再恢复当前主角的完整高光候选并替换删减方案。是否继续？",
    );
    if (confirmed) {
      void runAnalysis(selectedPath, true);
    }
  }

  async function openRecent(job: RecentJob) {
    await runAnalysis(job.sourcePath);
  }

  function updateClipEdit(clipId: string, patch: Partial<ClipEditState>) {
    if (!planReady || operationGate.current.blocked) return;
    setClipEdits((current) => {
      const selected = current.find((clip) => clip.clipId === clipId);
      if (!selected) {
        return current;
      }
      const enrichedPatch: Partial<ClipEditState> = { ...patch };
      const changesCandidate = patch.candidateId !== undefined && patch.candidateId !== selected.candidateId;
      if (changesCandidate) enrichedPatch.imported = null;
      if (selected.durationMode === "short") {
        if (patch.startSeconds !== undefined) {
          enrichedPatch.shortStartSeconds = patch.startSeconds;
        }
        if (patch.endSeconds !== undefined) {
          enrichedPatch.shortEndSeconds = patch.endSeconds;
        }
      }
      const synchronizesTime =
        Boolean(selected?.takeGroupId) &&
        ("startSeconds" in enrichedPatch ||
          "endSeconds" in enrichedPatch ||
          "durationMode" in enrichedPatch ||
          "storyPreRollSeconds" in enrichedPatch ||
          "shortStartSeconds" in enrichedPatch ||
          "shortEndSeconds" in enrichedPatch);
      const updated = current.map((clip) => {
        if (clip.clipId === clipId) {
          return { ...clip, ...enrichedPatch };
        }
        if (
          (synchronizesTime || changesCandidate) &&
          Boolean(selected?.takeGroupId) &&
          clip.takeGroupId === selected?.takeGroupId
        ) {
          return {
            ...clip,
            ...(changesCandidate ? { candidateId: enrichedPatch.candidateId!, imported: null } : {}),
            ...(enrichedPatch.startSeconds === undefined
              ? {}
              : { startSeconds: enrichedPatch.startSeconds }),
            ...(enrichedPatch.endSeconds === undefined
              ? {}
              : { endSeconds: enrichedPatch.endSeconds }),
            ...(enrichedPatch.durationMode === undefined
              ? {}
              : { durationMode: enrichedPatch.durationMode }),
            ...(enrichedPatch.storyPreRollSeconds === undefined
              ? {}
              : { storyPreRollSeconds: enrichedPatch.storyPreRollSeconds }),
            ...(enrichedPatch.shortStartSeconds === undefined
              ? {}
              : { shortStartSeconds: enrichedPatch.shortStartSeconds }),
            ...(enrichedPatch.shortEndSeconds === undefined
              ? {}
              : { shortEndSeconds: enrichedPatch.shortEndSeconds }),
          };
        }
        return clip;
      });
      return result
        ? normalizeStoryDurationClips(
            updated,
            result.highlights.candidates,
            result.replay.playback_time_seconds,
          )
        : updated;
    });
    setPlanFeedback("");
  }

  function extendCurrentKillSetups() {
    if (!planReady || operationGate.current.blocked || !result) return;
    const extended = extendKillSetupClips(clipEdits, result.highlights.candidates);
    setClipEdits(extended.clips);
    setPlanFeedback(extended.changedScenes
      ? `已为 ${extended.changedScenes} 个场次补足击杀前铺垫，保留当前片段选择和出点。`
      : "当前片段已保留足够铺垫，完整交战时长和自定义片段保持原样。");
  }

  function applyHeroHighlightSelection(
    hero: string,
    ruleIds: readonly HighlightRuleId[],
    announce: boolean,
  ) {
    if (!planReady || operationGate.current.blocked) return;
    if (!result) {
      return;
    }
    const normalizedRules = normalizeHighlightRuleIds(ruleIds);
    const next = createHeroHighlightClips(result, hero, normalizedRules);
    setHighlightHero(hero);
    setHighlightRuleIds(normalizedRules);
    setClipEdits(next);
    setSelectedClipId(next[0]?.clipId ?? "");
    setRenderResult(null);
    setRenderError("");
    const selectionLabel = formatHighlightRuleSelection(normalizedRules);
    const detail = heroHighlightSelectionDetail(
      result,
      hero,
      normalizedRules,
    );
    setPlanFeedback(
      next.length > 0
        ? `已生成 ${heroLabel(hero)}${selectionLabel}高光：${next.length} 段 · ${detail}`
        : `${heroLabel(hero)}在本局没有检测到“${selectionLabel}”高光。`,
    );
    if (announce) {
      setNotice({
        kind: next.length > 0 ? "success" : "error",
        title: `${heroLabel(hero)}高光已更新`,
        message:
          next.length > 0
            ? `${selectionLabel} · ${next.length} 段 · ${detail}`
            : `本局没有检测到“${selectionLabel}”高光。`,
      });
    }
  }

  function selectHeroHighlights(hero: string) {
    if (!planReady || operationGate.current.blocked) return;
    applyHeroHighlightSelection(hero, highlightRuleIds, true);
  }

  function updateHighlightRules(ruleIds: HighlightRuleId[]) {
    if (!planReady || operationGate.current.blocked) return;
    const normalizedRules = normalizeHighlightRuleIds(ruleIds);
    if (normalizedRules.length === 0) {
      return;
    }
    setHighlightRuleIds(normalizedRules);
    if (result && highlightHero) {
      applyHeroHighlightSelection(highlightHero, normalizedRules, false);
    }
  }

  function addClip() {
    if (!planReady || operationGate.current.blocked) return;
    if (!result) {
      return;
    }
    const selected = clipEdits.find((clip) => clip.clipId === selectedClipId);
    const fallbackCandidate = result.highlights.candidates[0] ?? null;
    if (!selected && !fallbackCandidate) {
      return;
    }
    const replayDuration = replayEndFrame(result.replay.playback_time_seconds);
    const baseStart = selected
      ? Math.min(
          Math.max(0, selected.endSeconds),
          Math.max(0, replayDuration - 12),
        )
      : Math.max(0, (fallbackCandidate?.peak_seconds ?? 6) - 6);
    const candidateId = selected?.candidateId ?? fallbackCandidate!.id;
    const hero =
      selected?.viewHero ??
      fallbackCandidate?.primary_hero ??
      fallbackCandidate?.participants[0] ??
      result.replay.players[0]?.hero_name ??
      "";
    const clip: ClipEditState = {
      clipId: createClipId(),
      candidateId,
      imported: copyImportedMetadata(selected?.imported),
      viewHero: hero,
      cameraMode: "player_perspective",
      takeGroupId: null,
      takeRole: "primary",
      includeInFinal: true,
      durationMode: "short",
      storyPreRollSeconds: DEFAULT_STORY_PRE_ROLL_SECONDS,
      shortStartSeconds: roundFrame(baseStart),
      shortEndSeconds: roundFrame(Math.min(replayDuration, baseStart + 12)),
      startSeconds: roundFrame(baseStart),
      endSeconds: roundFrame(Math.min(replayDuration, baseStart + 12)),
    };
    setClipEdits([...clipEdits, clip]);
    setSelectedClipId(clip.clipId);
    setPlanFeedback("");
  }

  function activateTimelineCandidate(candidateId: string) {
    if (!planReady || operationGate.current.blocked) return;
    if (!result) {
      return;
    }
    const existing = clipEdits.find(
      (clip) => clip.candidateId === candidateId,
    );
    if (existing) {
      setSelectedClipId(existing.clipId);
      setPlanFeedback("已定位到片段清单中的对应事件。");
      return;
    }

    const candidate = result.highlights.candidates.find(
      (item) => item.id === candidateId,
    );
    if (!candidate) {
      return;
    }

    const replayDuration = replayEndFrame(result.replay.playback_time_seconds);
    const startSeconds = roundFrame(
      clamp(
        candidate.start_seconds,
        0,
        Math.max(0, replayDuration - 1),
      ),
    );
    const endSeconds = roundFrame(
      Math.min(
        replayDuration,
        Math.max(
          startSeconds + 1,
          Math.min(candidate.end_seconds, startSeconds + 90),
        ),
      ),
    );
    const clip: ClipEditState = {
      clipId: createClipId(),
      candidateId: candidate.id,
      viewHero:
        highlightHero ||
        candidate.kill_sequence?.hero ||
        candidate.primary_hero ||
        candidate.participants[0] ||
        result.replay.players[0]?.hero_name ||
        "",
      cameraMode: "player_perspective",
      takeGroupId: null,
      takeRole: "primary",
      includeInFinal: true,
      durationMode: "short",
      storyPreRollSeconds: DEFAULT_STORY_PRE_ROLL_SECONDS,
      shortStartSeconds: startSeconds,
      shortEndSeconds: endSeconds,
      startSeconds,
      endSeconds,
    };
    setClipEdits((current) =>
      [...current, clip].sort(
        (left, right) =>
          left.startSeconds - right.startSeconds ||
          left.endSeconds - right.endSeconds,
      ),
    );
    setSelectedClipId(clip.clipId);
    setRenderResult(null);
    setRenderError("");
    setPlanFeedback(
      `已从事件时间轴新增“${candidateTitle(candidate)}”，可继续精调时间与镜头。`,
    );
  }

  function duplicateClip(clipId: string) {
    if (!planReady || operationGate.current.blocked) return;
    const index = clipEdits.findIndex((clip) => clip.clipId === clipId);
    const source = clipEdits[index];
    if (!source) {
      return;
    }
    const copy = {
      ...source,
      clipId: createClipId(),
      imported: copyImportedMetadata(source.imported),
      takeGroupId: null,
      takeRole: "primary" as const,
      includeInFinal: true,
    };
    const next = [...clipEdits];
    next.splice(index + 1, 0, copy);
    setClipEdits(next);
    setSelectedClipId(copy.clipId);
    setPlanFeedback("");
  }

  function deleteClip(clipId: string) {
    if (!planReady || operationGate.current.blocked) return;
    const index = clipEdits.findIndex((clip) => clip.clipId === clipId);
    if (index < 0) {
      return;
    }
    const source = clipEdits[index]!;
    const deleteWholeGroup =
      source.takeRole === "primary" && Boolean(source.takeGroupId);
    const next = clipEdits.filter(
      (clip) =>
        clip.clipId !== clipId &&
        !(
          deleteWholeGroup &&
          clip.takeGroupId === source.takeGroupId
        ),
    );
    const fallback = next[Math.min(index, Math.max(0, next.length - 1))];
    setClipEdits(next);
    setSelectedClipId(fallback?.clipId ?? "");
    setPlanFeedback("");
  }

  function moveClip(clipId: string, direction: -1 | 1) {
    if (!planReady || operationGate.current.blocked) return;
    setClipEdits((current) => {
      const index = current.findIndex((clip) => clip.clipId === clipId);
      const target = index + direction;
      if (index < 0 || target < 0 || target >= current.length) {
        return current;
      }
      const next = [...current];
      [next[index], next[target]] = [next[target]!, next[index]!];
      return next;
    });
    setPlanFeedback("");
  }

  function updateRenderSettings(patch: Partial<RenderSettings>) {
    if (!planReady || operationGate.current.blocked) return;
    setRenderSettings((current) => ({ ...current, ...patch }));
    setPlanFeedback("");
    setRenderError("");
  }

  async function saveEditPlan(showNotice = true) {
    setSavingPlan(true);
    try {
      const saved = await flushCurrentDraft();
      if (saved) {
        const message = `剪辑草稿已保存：${saved.selectedClipCount} 份源素材，成片 ${formatDuration(saved.totalDurationSeconds)}`;
        setPlanFeedback(message);
        if (showNotice) setNotice({ kind: "success", title: "剪辑方案已保存", message });
      }
      return saved;
    } catch (reason: unknown) {
      const message = toErrorMessage(reason);
      setPlanFeedback(message);
      setNotice({ kind: "error", title: "剪辑方案保存失败", message });
      return null;
    } finally {
      setSavingPlan(false);
    }
  }

  async function refreshRenderResume(jobId: string, current: () => boolean) {
    setCheckingResume(true);
    setRenderResume(null);
    setResumeCheckError("");
    try {
      const summary = await invoke<RenderResumeSummary>("get_render_resume", { jobId });
      if (current()) setRenderResume(summary);
    } catch (reason: unknown) {
      if (current()) setResumeCheckError(`已保存片段检查未完成：${toErrorMessage(reason)}`);
    } finally {
      setCheckingResume(false);
    }
  }

  async function openMovieSetup() {
    if (!result || !planReady || interactionBlocked) return;
    const token = operationGate.current.begin("render");
    if (!token) return;
    const jobId = result.job_id;
    const current = () => operationGate.current.current(token) && activeJobRef.current === jobId;
    setMovieSetupOpen(true);
    setRenderResume(null);
    setResumeCheckError("");
    setCheckingResume(true);
    try {
      if (isTauriRuntime && await saveEditPlan(false) && current()) {
        await refreshRenderResume(jobId, current);
      }
    } finally {
      setCheckingResume(false);
      operationGate.current.finish(token);
    }
  }

  async function startMovieRender(resumeFingerprint: string | null = null) {
    if (!result || !planReady) return;
    if (!capabilities.renderReady) {
      setRenderError(capabilities.renderReason ?? "当前成片环境不完整，请检查 Dota 2 和媒体工具。");
      return;
    }
    if (!clipEdits.some((clip) => clip.includeInFinal) || clipEdits.some((clip) =>
      !isValidClipTimeRange(clip.startSeconds, clip.endSeconds, result.replay.playback_time_seconds))) {
      setRenderError("请检查片段起止时间，并至少保留一个默认入片素材。");
      return;
    }
    const token = operationGate.current.begin("render");
    if (!token) return;
    renderCancelRequested.current = false;
    const jobId = result.job_id;
    const current = () => operationGate.current.current(token) && activeJobRef.current === jobId;
    setRendering(true);
    setRenderError("");
    setRenderResult(null);
    setRenderProgress({ stage: "preflight", status: "running", message: "正在保存方案并准备成片",
      percent: 1, currentClip: 0, totalClips: 0 });
    try {
      const saved = await saveEditPlan(false);
      if (!saved || !current() || renderCancelRequested.current) {
        setRenderProgress(null);
        if (renderCancelRequested.current) setPlanFeedback("已取消导出准备。");
        return;
      }
      const channel = new Channel<RenderProgress>();
      channel.onmessage = (message) => {
        if (!current()) return;
        setRenderProgress(message);
        if (message.status === "failed") setRenderError(message.message);
      };
      const completed = await invoke<RenderResult>("start_render", { jobId, onProgress: channel, resumeFingerprint, outputDirectory: videoOutputDirectory });
      if (!current()) return;
      setRenderResult(completed);
      setRenderResume(null);
      setRenderProgress({ stage: "complete", status: "complete", message: "成片已完成",
        percent: 100, currentClip: completed.segmentCount, totalClips: completed.segmentCount });
      setNotice({ kind: "success", title: "高光成片已完成",
        message: `${formatDuration(completed.durationSeconds)} · ${completed.width}x${completed.height}` });
    } catch (reason: unknown) {
      if (current()) {
        const message = toErrorMessage(reason);
        setRenderError(message);
        setRenderProgress(null);
        setNotice({ kind: "error", title: "成片任务没有完成", message });
        await refreshRenderResume(jobId, current);
      }
    } finally {
      operationGate.current.finish(token);
      setRendering(false);
      const refreshed = await invoke<Capabilities>("get_capabilities").catch(() => null);
      if (refreshed) setCapabilities(refreshed);
    }
  }

  async function cancelMovieRender() {
    if (!rendering) {
      return;
    }
    renderCancelRequested.current = true;
    setPlanFeedback("正在安全停止导出并关闭 Dota 2...");
    try {
      await invoke<boolean>("cancel_render");
    } catch (reason: unknown) {
      setRenderError(toErrorMessage(reason));
    }
  }

  async function openRenderPath(path: string) {
    try {
      await invoke("open_local_path", { path });
    } catch (reason: unknown) {
      setRenderError(toErrorMessage(reason));
    }
  }

  async function checkForAppUpdate(showResultNotice = true) {
    if (operationGate.current.updating || closePending.current) return;
    if (!isTauriRuntime) {
      setAppUpdate({ phase: "unavailable" });
      return;
    }

    setAppUpdate({ phase: "checking" });
    try {
      const metadata =
        await invoke<Extract<AppUpdateState, { phase: "available" }>["metadata"] | null>(
          "check_for_app_update",
        );
      if (operationGate.current.updating || closePending.current) return;
      if (metadata) {
        setAppUpdate({ phase: "available", metadata });
        setNotice({
          kind: "success",
          title: `发现新版本 ${metadata.version}`,
          message: "可在设置中查看简介并下载安装。",
        });
      } else {
        setAppUpdate({ phase: "current" });
        if (showResultNotice) {
          setNotice({
            kind: "success",
            title: "已经是最新版本",
            message: `当前版本 ${appVersion} 无需更新。`,
          });
        }
      }
    } catch (reason: unknown) {
      const message = toErrorMessage(reason);
      if (operationGate.current.updating || closePending.current) return;
      setAppUpdate({ phase: "error", message });
      if (showResultNotice) {
        setNotice({
          kind: "error",
          title: "暂时无法检查更新",
          message,
        });
      }
    }
  }

  async function installAppUpdate() {
    if (appUpdate.phase !== "available") {
      return;
    }
    const token = operationGate.current.begin("update");
    if (!token) {
      setNotice({
        kind: "error",
        title: "当前任务尚未完成",
        message: "请等待分析或视频导出结束后再安装更新。",
      });
      return;
    }

    const metadata = appUpdate.metadata;
    let downloadedBytes = 0;
    let totalBytes: number | null = null;
    const channel = new Channel<AppUpdateEvent>();
    channel.onmessage = (event) => {
      if (!operationGate.current.current(token)) return;
      if (event.event === "started") {
        totalBytes = event.data.contentLength;
        setAppUpdate({
          phase: "downloading",
          metadata,
          downloadedBytes,
          totalBytes,
        });
      } else if (event.event === "progress") {
        downloadedBytes += event.data.chunkLength;
        setAppUpdate({
          phase: "downloading",
          metadata,
          downloadedBytes,
          totalBytes,
        });
      } else {
        setAppUpdate({ phase: "installing", metadata });
      }
    };
    setAppUpdate({
      phase: "downloading",
      metadata,
      downloadedBytes,
      totalBytes,
    });

    try {
      await flushCurrentDraft();
      if (!operationGate.current.current(token)) return;
      await invoke("install_app_update", { onEvent: channel });
    } catch (reason: unknown) {
      const message = toErrorMessage(reason);
      setAppUpdate({ phase: "error", message });
      setNotice({
        kind: "error",
        title: "更新没有完成",
        message,
      });
    } finally {
      operationGate.current.finish(token);
    }
  }

  return (
    <div className={`app-shell ${dragActive ? "drag-active" : ""}`}>
      <Titlebar
        notice={notice}
        completionNoticeEnabled={completionNoticeEnabled}
        capabilities={capabilities}
        appVersion={appVersion}
        appUpdate={appUpdate}
        updateInstallationBlocked={interactionBlocked}
        configurationBlocked={interactionBlocked}
        onCompletionNoticeChange={setCompletionNoticeEnabled}
        onCapabilitiesChange={setCapabilities}
        onCheckForUpdate={() => void checkForAppUpdate()}
        onInstallUpdate={() => void installAppUpdate()}
        onClearNotice={() => setNotice(null)}
      />
      <Sidebar
        activeView={view}
        capabilities={capabilities}
        recentJobs={recentJobs}
        replayCount={replayLibrary?.files.length}
        busy={interactionBlocked}
        onImport={openImportDialog}
        onViewChange={setView}
        onOpenExternalTool={openExternalTool}
        onOpenRecent={(job) => void openRecent(job)}
      />

      <main className="workspace">
        <ProjectHeader
          selectedPath={selectedPath}
          result={result}
          busy={interactionBlocked}
          renderReady={capabilities.renderReady}
          onImport={openImportDialog}
          onAnalyze={confirmReanalysis}
          onOpenMovieSetup={() => void openMovieSetup()}
        />

        {view === "library" ? (
          <LibraryView snapshot={replayLibrary} directory={replayDirectory} busy={interactionBlocked}
            loading={replayLibraryLoading} error={replayLibraryError}
            onOpen={(file) => void runAnalysis(file.path)} onImport={openImportDialog}
            onRefresh={() => { if (replayLibraryRefresh.current) void replayLibraryRefresh.current(); }}
            onChooseDirectory={() => void chooseReplayDirectory()} />
        ) : busy || progress?.status === "failed" ? (
          <AnalysisState
            path={selectedPath}
            progress={progress}
            completedStages={completedStages}
            error={error}
            onRetry={() => void runAnalysis()}
          />
        ) : result ? (
          <ResultsView
            result={result}
            clipEdits={clipEdits}
            selectedClip={selectedClip}
            selectedCandidate={selectedCandidate}
            highlightHero={highlightHero}
            highlightRuleIds={highlightRuleIds}
            renderSettings={renderSettings}
            onSelectClip={setSelectedClipId}
            onUpdateClip={updateClipEdit}
            onSelectHighlightHero={selectHeroHighlights}
            onUpdateHighlightRules={updateHighlightRules}
            onUpdateRenderSettings={updateRenderSettings}
            onAddClip={addClip}
            onExtendKillSetups={extendCurrentKillSetups}
            onActivateTimelineCandidate={activateTimelineCandidate}
            onDuplicateClip={duplicateClip}
            onDeleteClip={deleteClip}
            onMoveClip={moveClip}
          />
        ) : (
          <EmptyWorkbench
            recentJobs={recentJobs}
            error={error}
            onImport={openImportDialog}
            onOpenRecent={(job) => void openRecent(job)}
          />
        )}

        <Statusbar
          progress={progress}
          hasResult={Boolean(result)}
          capabilities={capabilities}
        />
      </main>
      <ExternalToolDialog notice={externalNotice} onClose={() => setExternalNotice(null)} />
      {dragActive && (
        <div className="drop-overlay" aria-live="polite">
          <div className="drop-overlay-content">
            <FileUp />
            <strong>松开即可导入录像</strong>
            <span>只接收一个 .dem 文件</span>
          </div>
        </div>
      )}
      {importDialogOpen && (
        <ReplayImportDialog
          replayDirectory={replayDirectory}
          replayId={replayId}
          busy={busy || replayLookupBusy}
          error={replayLookupError}
          onReplayDirectoryChange={(value) => {
            setReplayDirectory(value);
            setReplayLookupError("");
          }}
          onReplayIdChange={(value) => {
            setReplayId(value);
            setReplayLookupError("");
          }}
          onChooseDirectory={() => void chooseReplayDirectory()}
          onChooseFile={() => void chooseReplayFile()}
          onLoadById={() => void loadReplayById()}
          onClose={() => {
            if (!busy && !replayLookupBusy) {
              setImportDialogOpen(false);
            }
          }}
        />
      )}
      {movieSetupOpen && result && (
        <MovieSetupDialog
          version={appVersion}
          result={result}
          clipEdits={clipEdits}
          capabilities={capabilities}
          renderSettings={renderSettings}
          saving={savingPlan}
          rendering={rendering}
          renderProgress={renderProgress}
          renderResult={renderResult}
          renderError={renderError}
          outputDirectory={videoOutputDirectory}
          onOutputDirectoryChange={setVideoOutputDirectory}
          onOutputError={setRenderError}
          resume={renderResume}
          checkingResume={checkingResume}
          resumeCheckError={resumeCheckError}
          feedback={planFeedback}
          onClose={() => {
            if (!rendering && !checkingResume && !savingPlan) {
              setMovieSetupOpen(false);
            }
          }}
          onStart={() => void startMovieRender()}
          onResume={() => { if (resumeAction(renderResume)) void startMovieRender(renderResume!.fingerprint); }}
          onCancel={() => void cancelMovieRender()}
          onOpenPath={(path) => void openRenderPath(path)}
        />
      )}
    </div>
  );
}

interface TitlebarProps {
  notice: AppNotice | null;
  completionNoticeEnabled: boolean;
  capabilities: Capabilities;
  appVersion: string;
  appUpdate: AppUpdateState;
  updateInstallationBlocked: boolean;
  configurationBlocked: boolean;
  onCompletionNoticeChange: (enabled: boolean) => void;
  onCapabilitiesChange: (capabilities: Capabilities) => void;
  onCheckForUpdate: () => void;
  onInstallUpdate: () => void;
  onClearNotice: () => void;
}

function Titlebar({
  notice,
  completionNoticeEnabled,
  capabilities,
  appVersion,
  appUpdate,
  updateInstallationBlocked,
  configurationBlocked,
  onCompletionNoticeChange,
  onCapabilitiesChange,
  onCheckForUpdate,
  onInstallUpdate,
  onClearNotice,
}: TitlebarProps) {
  const [panel, setPanel] = useState<TitlebarPanel>(null);
  const [dota2PathDraft, setDota2PathDraft] = useState(
    capabilities.dota2Path ?? "",
  );
  const [dota2PathBusy, setDota2PathBusy] = useState(false);
  const [dota2PathFeedback, setDota2PathFeedback] = useState("");
  const [dota2PathError, setDota2PathError] = useState("");
  const actionsRef = useRef<HTMLDivElement>(null);
  const updateCopy = appUpdateCopy(
    appUpdate,
    updateInstallationBlocked,
  );

  useEffect(() => {
    setDota2PathDraft(capabilities.dota2Path ?? "");
  }, [capabilities.dota2Path]);

  async function saveDota2Path(path: string) {
    const trimmed = path.trim();
    if (!trimmed) {
      setDota2PathError("请输入 dota2.exe 的完整路径，或使用浏览按钮选择。");
      setDota2PathFeedback("");
      return;
    }
    setDota2PathBusy(true);
    setDota2PathError("");
    setDota2PathFeedback("");
    try {
      const updated = await invoke<Capabilities>("set_dota2_executable", {
        path: trimmed,
      });
      onCapabilitiesChange(updated);
      setDota2PathDraft(updated.dota2Path ?? trimmed);
      setDota2PathFeedback("路径已保存在这台电脑，成片能力已刷新。");
    } catch (reason: unknown) {
      setDota2PathError(toErrorMessage(reason));
    } finally {
      setDota2PathBusy(false);
    }
  }

  async function chooseDota2Path() {
    if (!isTauriRuntime) {
      setDota2PathError("请从桌面应用中选择 Dota 2 程序。");
      return;
    }
    setDota2PathError("");
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: "选择 Dota 2 程序",
        defaultPath: dota2PathDraft || undefined,
        filters: [{ name: "Dota 2 程序", extensions: ["exe"] }],
      });
      if (typeof selected === "string") {
        setDota2PathDraft(selected);
        await saveDota2Path(selected);
      }
    } catch (reason: unknown) {
      setDota2PathError(toErrorMessage(reason));
    }
  }

  async function resetDota2Path() {
    setDota2PathBusy(true);
    setDota2PathError("");
    setDota2PathFeedback("");
    try {
      const updated = await invoke<Capabilities>("set_dota2_executable", {
        path: null,
      });
      onCapabilitiesChange(updated);
      setDota2PathDraft(updated.dota2Path ?? "");
      setDota2PathFeedback(
        updated.dota2Found
          ? "已恢复自动检测，并找到 Dota 2。"
          : "已恢复自动检测，当前没有找到 Dota 2。",
      );
    } catch (reason: unknown) {
      setDota2PathError(toErrorMessage(reason));
    } finally {
      setDota2PathBusy(false);
    }
  }

  useEffect(() => {
    if (!panel) {
      return;
    }

    function closePanel(event: PointerEvent) {
      if (
        event.target instanceof Node &&
        !actionsRef.current?.contains(event.target)
      ) {
        setPanel(null);
      }
    }

    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setPanel(null);
      }
    }

    document.addEventListener("pointerdown", closePanel);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closePanel);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [panel]);

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="brand" data-tauri-drag-region>
        <div className="brand-mascot-wrap" data-tauri-drag-region>
          <img
            className="brand-mascot"
            src={mascot}
            alt=""
            data-tauri-drag-region
          />
        </div>
        <div data-tauri-drag-region>
          <div className="brand-name" data-tauri-drag-region>
            猫猫的剪辑小助手
          </div>
          <div className="brand-build" data-tauri-drag-region>
            本地高光工作台
          </div>
        </div>
      </div>
      <div className="titlebar-state" data-tauri-drag-region>
        <span className="save-dot" data-tauri-drag-region />
        <span data-tauri-drag-region>本地运行</span>
      </div>
      <div className="titlebar-actions" ref={actionsRef}>
        <button
          className={`icon-button ${panel === "notifications" ? "active" : ""}`}
          title="通知"
          aria-label="通知"
          aria-expanded={panel === "notifications"}
          onClick={() =>
            setPanel((current) =>
              current === "notifications" ? null : "notifications",
            )
          }
        >
          <Bell />
          {notice && <span className="notification-dot" />}
        </button>
        <button
          className={`icon-button ${panel === "settings" ? "active" : ""}`}
          title="设置"
          aria-label="设置"
          aria-expanded={panel === "settings"}
          onClick={() =>
            setPanel((current) =>
              current === "settings" ? null : "settings",
            )
          }
        >
          <Settings2 />
        </button>
        {panel === "notifications" && (
          <section
            className="titlebar-popover notification-popover"
            aria-label="通知中心"
          >
            <div className="popover-heading">
              <strong>通知</strong>
              {notice && (
                <button onClick={onClearNotice} type="button">
                  清除
                </button>
              )}
            </div>
            {notice ? (
              <div className={`notice-item ${notice.kind}`}>
                {notice.kind === "success" ? (
                  <CheckCircle2 />
                ) : (
                  <CircleAlert />
                )}
                <span>
                  <strong>{notice.title}</strong>
                  <small>{notice.message}</small>
                </span>
              </div>
            ) : (
              <div className="popover-empty">
                <Bell />
                <span>暂时没有新通知</span>
              </div>
            )}
          </section>
        )}
        {panel === "settings" && (
          <section
            className="titlebar-popover settings-popover"
            aria-label="应用设置"
          >
            <div className="popover-heading">
              <strong>应用设置</strong>
            </div>
            <div className="dota-path-setting">
              <div className="dota-path-heading">
                <span>
                  <Gamepad2 />
                  <strong>Dota 2 程序路径</strong>
                </span>
                <small className={capabilities.dota2Found ? "ready" : "missing"}>
                  {capabilities.dota2PathSource === "custom"
                    ? "用户指定"
                    : capabilities.dota2PathSource === "automatic"
                      ? "自动检测"
                      : "未找到"}
                </small>
              </div>
              <small className="dota-path-help">
                每台电脑单独保存。请选择游戏目录中的 dota2.exe。
              </small>
              <div className="dota-path-input-row">
                <input
                  type="text"
                  value={dota2PathDraft}
                  placeholder="例如 F:\\SteamLibrary\\...\\win64\\dota2.exe"
                  title={dota2PathDraft}
                  disabled={configurationBlocked || dota2PathBusy}
                  onChange={(event) => {
                    setDota2PathDraft(event.target.value);
                    setDota2PathError("");
                    setDota2PathFeedback("");
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      void saveDota2Path(dota2PathDraft);
                    }
                  }}
                />
                <button
                  type="button"
                  title="浏览并选择 dota2.exe"
                  aria-label="浏览并选择 dota2.exe"
                  disabled={configurationBlocked || dota2PathBusy}
                  onClick={() => void chooseDota2Path()}
                >
                  <FolderOpen />
                </button>
              </div>
              <div className="dota-path-actions">
                <button
                  type="button"
                  disabled={
                    configurationBlocked ||
                    dota2PathBusy ||
                    !dota2PathDraft.trim()
                  }
                  onClick={() => void saveDota2Path(dota2PathDraft)}
                >
                  {dota2PathBusy ? <LoaderCircle className="spin" /> : <Check />}
                  <span>应用路径</span>
                </button>
                <button
                  type="button"
                  disabled={configurationBlocked || dota2PathBusy}
                  onClick={() => void resetDota2Path()}
                >
                  <RotateCcw />
                  <span>恢复自动检测</span>
                </button>
              </div>
              {dota2PathFeedback && (
                <small className="dota-path-feedback success">
                  {dota2PathFeedback}
                </small>
              )}
              {dota2PathError && (
                <small className="dota-path-feedback error">{dota2PathError}</small>
              )}
            </div>
            <VideoOutputSetting capabilities={capabilities} blocked={configurationBlocked} native={isTauriRuntime} onChange={onCapabilitiesChange}/>
            <label className="setting-row">
              <span>
                <strong>分析完成提醒</strong>
                <small>完成或失败时显示在通知中心</small>
              </span>
              <input
                type="checkbox"
                checked={completionNoticeEnabled}
                onChange={(event) =>
                  onCompletionNoticeChange(event.target.checked)
                }
              />
              <span className="toggle-track" aria-hidden="true">
                <span />
              </span>
            </label>
            <div className="settings-status">
              <span>处理方式</span>
              <strong>仅限本机</strong>
            </div>
            <div className={`settings-update ${appUpdate.phase}`}>
              <div className="settings-update-copy">
                <span className="settings-update-heading">
                  <ShieldCheck />
                  <strong>软件更新</strong>
                </span>
                <strong className="settings-update-title">
                  {updateCopy.title}
                </strong>
                <small>{updateCopy.detail}</small>
              </div>
              {updateCopy.action && (
                <button
                  className="settings-update-button"
                  type="button"
                  onClick={
                    updateCopy.action === "install"
                      ? onInstallUpdate
                      : onCheckForUpdate
                  }
                >
                  {updateCopy.action === "install" ? (
                    <Download />
                  ) : (
                    <RefreshCw />
                  )}
                  <span>
                    {updateCopy.action === "install" ? "下载安装" : "重新检查"}
                  </span>
                </button>
              )}
              {appUpdate.phase === "downloading" &&
                appUpdate.totalBytes !== null && (
                  <progress
                    max={appUpdate.totalBytes}
                    value={appUpdate.downloadedBytes}
                    aria-label="更新下载进度"
                  />
                )}
            </div>
            <div className="settings-version">
              <span>版本 {appVersion}</span>
              <span>作者：猫猫只用虎</span>
            </div>
          </section>
        )}
        <span className="window-divider" />
        <button
          className="window-button"
          title="最小化"
          aria-label="最小化"
          onClick={() => void appWindow?.minimize()}
        >
          <Minus />
        </button>
        <button
          className="window-button"
          title="最大化或还原"
          aria-label="最大化或还原"
          onClick={() => void appWindow?.toggleMaximize()}
        >
          <Square />
        </button>
        <button
          className="window-button close"
          title="关闭"
          aria-label="关闭"
          onClick={() => void appWindow?.close()}
        >
          <X />
        </button>
      </div>
    </header>
  );
}

interface SidebarProps {
  activeView: View;
  capabilities: Capabilities;
  recentJobs: RecentJob[];
  replayCount?: number;
  busy: boolean;
  onImport: () => void;
  onViewChange: (view: View) => void;
  onOpenExternalTool: (id: ExternalToolId) => void;
  onOpenRecent: (job: RecentJob) => void;
}

function Sidebar({
  activeView,
  capabilities,
  recentJobs,
  replayCount,
  busy,
  onImport,
  onViewChange,
  onOpenExternalTool,
  onOpenRecent,
}: SidebarProps) {
  return (
    <aside className="sidebar">
      <button className="import-button" onClick={onImport} disabled={busy}>
        <FileUp />
        <span>导入录像</span>
      </button>

      <nav className="primary-nav" aria-label="主导航">
        <NavButton
          icon={<Clapperboard />}
          label="高光时刻"
          active={activeView === "workbench"}
          onClick={() => onViewChange("workbench")}
        />
        <NavButton
          icon={<Sparkles />}
          label="WTF 剪辑"
          active={false}
          disabled={busy}
          onClick={() => onOpenExternalTool("wtf")}
        />
        <NavButton
          icon={<FileText />}
          label="剧情剪辑"
          active={false}
          disabled={busy}
          onClick={() => onOpenExternalTool("story")}
        />
        <NavButton
          icon={<FileText />}
          label="录像分析"
          active={false}
          disabled={busy}
          onClick={() => onOpenExternalTool("analysis")}
        />
        <NavButton
          icon={<Library />}
          label="录像库"
          count={replayCount}
          active={activeView === "library"}
          onClick={() => onViewChange("library")}
        />
      </nav>

      <div className="sidebar-section">
        <div className="section-label">最近任务</div>
        <div className="sidebar-jobs">
          {recentJobs.map((job, index) => (
            <button
              className="job-item"
              key={job.jobId}
              onClick={() => onOpenRecent(job)}
              disabled={busy}
            >
              <span
                className={`job-indicator ${index === 0 ? "" : "idle"}`}
              />
              <span className="job-copy">
                <strong>{job.sourceName}</strong>
                <small>
                  {job.candidateCount} 个事件锚点 ·{" "}
                  {formatDuration(job.durationSeconds)}
                </small>
              </span>
            </button>
          ))}
          {recentJobs.length === 0 && (
            <div className="no-recent">还没有剪辑任务</div>
          )}
        </div>
      </div>

      <div className="environment-panel">
        <div className="environment-title">
          <span>本机能力</span>
          <span className="status-ok">
            {capabilities.renderReady ? "成片可用" : "分析可用"}
          </span>
        </div>
        <EnvironmentRow
          label="DEM 分析"
          ready={capabilities.analysisReady}
        />
        <EnvironmentRow label="FFmpeg" ready={capabilities.ffmpegFound} />
        <EnvironmentRow label="FFprobe" ready={capabilities.ffprobeFound} />
        <EnvironmentRow label="Dota 2" ready={capabilities.dota2Found} />
      </div>
    </aside>
  );
}

interface NavButtonProps {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  disabled?: boolean;
  count?: number;
  onClick?: () => void;
}

function NavButton({
  icon,
  label,
  active,
  disabled,
  count,
  onClick,
}: NavButtonProps) {
  return (
    <button
      className={`nav-item ${active ? "active" : ""}`}
      onClick={onClick}
      disabled={disabled}
      title={disabled ? "后续版本开放" : undefined}
    >
      {icon}
      <span>{label}</span>
      {typeof count === "number" && <span className="nav-count">{count}</span>}
    </button>
  );
}

function EnvironmentRow({ label, ready }: { label: string; ready: boolean }) {
  return (
    <div className="environment-row">
      <span>{label}</span>
      <span className={ready ? "ready" : "waiting"}>
        {ready ? "就绪" : "未找到"}
      </span>
    </div>
  );
}

interface ProjectHeaderProps {
  selectedPath: string;
  result: AnalysisSummary | null;
  busy: boolean;
  renderReady: boolean;
  onImport: () => void;
  onAnalyze: () => void;
  onOpenMovieSetup: () => void;
}

function ProjectHeader({
  selectedPath,
  result,
  busy,
  renderReady,
  onImport,
  onAnalyze,
  onOpenMovieSetup,
}: ProjectHeaderProps) {
  const sourceName = selectedPath ? fileName(selectedPath) : "等待导入录像";
  const genericCandidates =
    result?.highlights.candidates.filter(
      (candidate) => candidate.kind !== "hero_kill_sequence",
    ).length ?? 0;
  const heroSequences =
    result?.highlights.candidates.filter(
      (candidate) => candidate.kind === "hero_kill_sequence",
    ).length ?? 0;
  const summary = result
    ? `${formatDuration(result.replay.playback_time_seconds)} · ${genericCandidates} 个通用候选 · ${heroSequences} 段英雄击杀`
    : "选择一个 Dota 2 .dem 文件开始";

  return (
    <section className="project-header">
      <div className="project-title">
        <div className="file-icon">
          <FileVideo2 />
        </div>
        <div>
          <h1>{sourceName}</h1>
          <p>{summary}</p>
        </div>
      </div>
      <div className="header-controls">
        <span className="manual-mode-badge">
          <SlidersHorizontal />
          精确剪辑
        </span>
        {selectedPath ? (
          <button
            className="secondary-button"
            onClick={onAnalyze}
            disabled={busy}
          >
            <RefreshCw />
            <span>重新分析</span>
          </button>
        ) : (
          <button className="secondary-button" onClick={onImport}>
            <FolderOpen />
            <span>选择录像</span>
          </button>
        )}
        <button
          className="primary-button"
          disabled={!result || busy}
          title={
            result
              ? renderReady
                ? "检查方案并开始生成"
                : "检查并保存剪辑方案"
              : "请先分析一份录像"
          }
          onClick={onOpenMovieSetup}
        >
          <Play />
          <span>导出视频</span>
        </button>
      </div>
    </section>
  );
}

interface EmptyWorkbenchProps {
  recentJobs: RecentJob[];
  error: string;
  onImport: () => void;
  onOpenRecent: (job: RecentJob) => void;
}

function EmptyWorkbench({
  recentJobs,
  error,
  onImport,
  onOpenRecent,
}: EmptyWorkbenchProps) {
  return (
    <div className="empty-workbench">
      <section className="import-zone">
        <div className="empty-mascot-wrap">
          <img src={mascot} alt="戴着耳机、拿着场记板的猫猫助手" />
        </div>
        <div className="empty-copy">
          <span className="eyebrow">本地高光剪辑</span>
          <h2>把 DEM 交给猫猫</h2>
          <p>录像只在这台电脑处理，原文件不会被修改。</p>
        </div>
        <button className="primary-button large" onClick={onImport}>
          <FolderOpen />
          <span>载入录像</span>
        </button>
        <div className="format-hint">
          <ShieldCheck />
          <span>只读校验 · Source 2 DEM</span>
        </div>
        {error && <InlineError message={error} />}
      </section>

      <section className="recent-panel">
        <div className="panel-heading">
          <div>
            <span className="eyebrow">继续工作</span>
            <h2>最近任务</h2>
          </div>
          <Clock3 />
        </div>
        <div className="recent-list">
          {recentJobs.slice(0, 5).map((job) => (
            <button
              className="recent-row"
              key={job.jobId}
              onClick={() => onOpenRecent(job)}
            >
              <span className="recent-file-icon">
                <Video />
              </span>
              <span className="recent-copy">
                <strong>{job.sourceName}</strong>
                <small>
                  {job.candidateCount} 个事件锚点 ·{" "}
                  {formatBytes(job.byteLength)}
                </small>
              </span>
              <ChevronRight />
            </button>
          ))}
          {recentJobs.length === 0 && (
            <div className="recent-empty">
              <Search />
              <span>首次分析后，任务会出现在这里</span>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

function ReplayImportDialog({
  replayDirectory,
  replayId,
  busy,
  error,
  onReplayDirectoryChange,
  onReplayIdChange,
  onChooseDirectory,
  onChooseFile,
  onLoadById,
  onClose,
}: {
  replayDirectory: string;
  replayId: string;
  busy: boolean;
  error: string;
  onReplayDirectoryChange: (value: string) => void;
  onReplayIdChange: (value: string) => void;
  onChooseDirectory: () => void;
  onChooseFile: () => void;
  onLoadById: () => void;
  onClose: () => void;
}) {
  useEffect(() => {
    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape" && !busy) {
        onClose();
      }
    }
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) {
          onClose();
        }
      }}
    >
      <section
        className="import-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="import-dialog-title"
      >
        <header className="movie-dialog-header">
          <span className="movie-dialog-icon">
            <FileVideo2 />
          </span>
          <span>
            <small>本地 DEM</small>
            <h2 id="import-dialog-title">载入录像</h2>
          </span>
          <button
            className="icon-button"
            onClick={onClose}
            disabled={busy}
            title="关闭"
            aria-label="关闭载入录像窗口"
          >
            <X />
          </button>
        </header>

        <div className="import-dialog-body">
          <div className="import-method-heading">
            <Search />
            <strong>按录像编号</strong>
          </div>

          <label className="import-field">
            <span>录像目录</span>
            <span className="import-path-control">
              <input
                value={replayDirectory}
                onChange={(event) =>
                  onReplayDirectoryChange(event.target.value)
                }
                placeholder="选择或输入 replay 文件夹"
                spellCheck={false}
                disabled={busy}
              />
              <button
                type="button"
                onClick={onChooseDirectory}
                disabled={busy}
                title="选择录像目录"
                aria-label="选择录像目录"
              >
                <FolderOpen />
              </button>
            </span>
          </label>

          <form
            className="import-field"
            onSubmit={(event) => {
              event.preventDefault();
              onLoadById();
            }}
          >
            <span>录像编号</span>
            <span className="replay-id-control">
              <input
                value={replayId}
                onChange={(event) =>
                  onReplayIdChange(
                    event.target.value.replace(/\D/g, "").slice(0, 20),
                  )
                }
                placeholder="例如 1000000001"
                inputMode="numeric"
                autoComplete="off"
                autoFocus
                disabled={busy}
              />
              <button
                className="primary-button"
                type="submit"
                disabled={
                  busy || !replayDirectory.trim() || !replayId.trim()
                }
              >
                {busy ? <LoaderCircle className="spin" /> : <Search />}
                <span>{busy ? "正在载入" : "载入并分析"}</span>
              </button>
            </span>
          </form>

          {error && <InlineError message={error} />}

          <div className="import-choice-divider">
            <span>或</span>
          </div>

          <button
            className="secondary-button file-import-action"
            type="button"
            onClick={onChooseFile}
            disabled={busy}
          >
            <FileUp />
            <span>选择 .dem 文件</span>
          </button>

          <div className="import-privacy">
            <ShieldCheck />
            <span>录像目录仅保存在本机</span>
          </div>
        </div>
      </section>
    </div>
  );
}

interface AnalysisStateProps {
  path: string;
  progress: AnalysisProgress | null;
  completedStages: string[];
  error: string;
  onRetry: () => void;
}

function AnalysisState({
  path,
  progress,
  completedStages,
  error,
  onRetry,
}: AnalysisStateProps) {
  const failed = progress?.status === "failed";
  return (
    <div className="analysis-state">
      <div className={`analysis-symbol ${failed ? "failed" : ""}`}>
        {failed ? <CircleAlert /> : <LoaderCircle className="spin" />}
      </div>
      <span className="eyebrow">{fileName(path)}</span>
      <h2>{failed ? "这次分析没有完成" : progress?.message}</h2>
      <p>
        {failed
          ? "录像没有被修改，可以修正问题后重新运行。"
          : "正在本机读取比赛事件，请保持应用开启。"}
      </p>
      <div className="pipeline-board">
        {pipelineStages.map((stage) => {
          const isComplete = completedStages.includes(stage.id);
          const isActive = progress?.stage === stage.id && !isComplete;
          const status = isComplete ? "complete" : isActive ? "active" : "";
          return (
            <div className={`pipeline-card ${status}`} key={stage.id}>
              <span className="pipeline-card-icon">
                {isComplete ? (
                  <Check />
                ) : isActive ? (
                  <LoaderCircle className="spin" />
                ) : (
                  <span />
                )}
              </span>
              <strong>{stage.label}</strong>
            </div>
          );
        })}
      </div>
      {error && <InlineError message={error} />}
      {failed && (
        <button className="primary-button large" onClick={onRetry}>
          <RefreshCw />
          <span>重新分析</span>
        </button>
      )}
    </div>
  );
}

interface ResultsViewProps {
  result: AnalysisSummary;
  clipEdits: ClipEdits;
  selectedClip: ClipEditState | null;
  selectedCandidate: HighlightCandidate | null;
  highlightHero: string;
  highlightRuleIds: HighlightRuleId[];
  renderSettings: RenderSettings;
  onSelectClip: (id: string) => void;
  onUpdateClip: (clipId: string, patch: Partial<ClipEditState>) => void;
  onSelectHighlightHero: (hero: string) => void;
  onUpdateHighlightRules: (ruleIds: HighlightRuleId[]) => void;
  onUpdateRenderSettings: (patch: Partial<RenderSettings>) => void;
  onAddClip: () => void;
  onExtendKillSetups: () => void;
  onActivateTimelineCandidate: (candidateId: string) => void;
  onDuplicateClip: (clipId: string) => void;
  onDeleteClip: (clipId: string) => void;
  onMoveClip: (clipId: string, direction: -1 | 1) => void;
}

function ResultsView({
  result,
  clipEdits,
  selectedClip,
  selectedCandidate,
  highlightHero,
  highlightRuleIds,
  renderSettings,
  onSelectClip,
  onUpdateClip,
  onSelectHighlightHero,
  onUpdateHighlightRules,
  onUpdateRenderSettings,
  onAddClip,
  onExtendKillSetups,
  onActivateTimelineCandidate,
  onDuplicateClip,
  onDeleteClip,
  onMoveClip,
}: ResultsViewProps) {
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("hero");

  const [highlightQueryFeedback, setHighlightQueryFeedback] = useState<{
    kind: "success" | "error";
    message: string;
  } | null>(null);
  const highlightQueryFeedbackRef = useRef<HTMLDivElement>(null);
  const rosterPlayers = replayPlayers(result);
  const totalDuration = clipEdits.reduce(
    (total, clip) =>
      total +
      (clip.includeInFinal
        ? Math.max(0, clip.endSeconds - clip.startSeconds)
        : 0),
    0,
  );
  const finalClipCount = clipEdits.filter(
    (clip) => clip.includeInFinal,
  ).length;
  const assetLabels = clipAssetLabels(clipEdits);
  const selectedRuleLabel = formatHighlightRuleSelection(highlightRuleIds);
  const selectedHighlightSummary = highlightHero
    ? heroHighlightSummary(
        result,
        highlightHero,
        highlightRuleIds,
        finalClipCount,
      )
    : null;
  const filteredAnchorCandidates = highlightHero
    ? heroHighlightCandidates(result, highlightHero, highlightRuleIds)
    : result.highlights.candidates.filter(
        (candidate) => candidate.kind === "hero_kill_sequence",
      );
  const anchorCandidates =
    selectedCandidate &&
    !filteredAnchorCandidates.some(
      (candidate) => candidate.id === selectedCandidate.id,
    )
      ? [selectedCandidate, ...filteredAnchorCandidates].sort(
          (left, right) => left.peak_seconds - right.peak_seconds,
        )
      : filteredAnchorCandidates;

  useEffect(() => {
    setHighlightQueryFeedback(null);
  }, [result.job_id]);

  useEffect(() => {
    if (highlightQueryFeedback) {
      highlightQueryFeedbackRef.current?.scrollIntoView({
        block: "nearest",
      });
    }
  }, [highlightQueryFeedback]);

  function toggleHighlightRule(ruleId: HighlightRuleId) {
    const isSelected = highlightRuleIds.includes(ruleId);
    const next = isSelected
      ? highlightRuleIds.filter((id) => id !== ruleId)
      : [...highlightRuleIds, ruleId];
    const normalized = normalizeHighlightRuleIds(next);
    if (normalized.length === 0) {
      setHighlightQueryFeedback({
        kind: "error",
        message: "至少保留一种高光内容。",
      });
      return;
    }
    onUpdateHighlightRules(normalized);
    setHighlightQueryFeedback({
      kind: "success",
      message: highlightHero
        ? `已显示 ${heroLabel(highlightHero)}的${formatHighlightRuleSelection(
            normalized,
          )}高光。`
        : `已选择${formatHighlightRuleSelection(
            normalized,
          )}，再选择高光主角。`,
    });
  }

  return (
    <div className="results-workspace precision-workspace">
      <section className="candidate-panel">
        <div className="result-summary">
          <SummaryStat
            icon={<ListChecks />}
            label="默认素材"
            value={`${clipEdits.length}`}
            tone="pink"
          />
          <SummaryStat
            icon={<Clock3 />}
            label="整局时长"
            value={formatDuration(result.replay.playback_time_seconds)}
            tone="yellow"
          />
          <SummaryStat
            icon={<Clapperboard />}
            label="成片时长"
            value={formatDuration(totalDuration)}
            tone="blue"
          />
          <SummaryStat
            icon={<UserRound />}
            label={selectedHighlightSummary?.label ?? "英雄阵容"}
            value={
              selectedHighlightSummary?.value ?? `${rosterPlayers.length}/10`
            }
            tone="mint"
          />
        </div>

        <ReplayEventTimeline
          result={result}
          clipEdits={clipEdits}
          selectedClip={selectedClip}
          highlightHero={highlightHero}
          onSelectClip={onSelectClip}
          onActivateCandidate={onActivateTimelineCandidate}
        />

        <div className="candidate-table-header">
          <div>
            <span className="eyebrow">
              {highlightHero
                ? `${heroLabel(highlightHero)} · ${selectedRuleLabel} · 按时间排列`
                : "按导出顺序排列"}
            </span>
            <h2>片段清单</h2>
          </div>
          <button className="add-clip-button" onClick={onExtendKillSetups}
            title="将当前短击杀至少延长到首杀前 12 秒，并保留起手前 6 秒；不会重新添加已删除片段">
            <SkipBack />
            <span>补足击杀铺垫</span>
          </button>
          <button className="add-clip-button" onClick={onAddClip}>
            <Plus />
            <span>新增片段</span>
          </button>
        </div>

        <div className="candidate-list clip-list">
          {clipEdits.map((clip, index) => {
            const candidate = result.highlights.candidates.find(
              (item) => item.id === clip.candidateId,
            );
            const isSelected = selectedClip?.clipId === clip.clipId;

            return (
              <div
                className={`candidate-row review included clip-row ${
                  clip.takeRole === "alternate" ? "alternate-source" : ""
                } ${isSelected ? "selected" : ""}`}
                key={clip.clipId}
                role="button"
                tabIndex={0}
                aria-label={`${assetLabels.get(clip.clipId) ?? `素材 ${index + 1}`}，${
                  candidate ? candidateTitle(candidate) : clip.imported?.title ?? "自定义片段"
                }`}
                onClick={() => onSelectClip(clip.clipId)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    onSelectClip(clip.clipId);
                  }
                }}
              >
                <span className="candidate-rank">{index + 1}</span>
                <span className="candidate-main">
                  <strong>
                    <span className="source-asset-code">
                      {assetLabels.get(clip.clipId) ??
                        `S${String(index + 1).padStart(3, "0")}-A`}
                    </span>
                    {candidate
                      ? candidateTitle(candidate)
                      : clip.imported?.title ?? `自定义片段 ${index + 1}`}
                  </strong>
                  <small>
                    {clip.includeInFinal
                      ? "默认入片"
                      : "备用素材，不计入成片"}{" "}
                    ·{" "}
                    {formatTimecode(clip.startSeconds)} -{" "}
                    {formatTimecode(clip.endSeconds)} ·{" "}
                    {clip.takeRole === "alternate"
                      ? "跟随目标："
                      : highlightHero
                        ? "主角："
                        : "视角："}
                    {heroLabel(clip.viewHero)}
                  </small>
                </span>
                <span className="candidate-kind">
                  {takeRoleLabel(clip.takeRole)} ·{" "}
                  {cameraModeShortLabel(clip.cameraMode)}
                </span>
                <span className="candidate-score">
                  <strong>
                    {formatDuration(clip.endSeconds - clip.startSeconds)}
                  </strong>
                  <small>时长</small>
                </span>
                <span
                  className="clip-row-actions"
                  onClick={(event) => event.stopPropagation()}
                >
                  <button
                    title="上移"
                    aria-label={`上移片段 ${index + 1}`}
                    disabled={index === 0}
                    onClick={() => onMoveClip(clip.clipId, -1)}
                  >
                    <ChevronUp />
                  </button>
                  <button
                    title="下移"
                    aria-label={`下移片段 ${index + 1}`}
                    disabled={index === clipEdits.length - 1}
                    onClick={() => onMoveClip(clip.clipId, 1)}
                  >
                    <ChevronDown />
                  </button>
                  <button
                    title="复制片段"
                    aria-label={`复制片段 ${index + 1}`}
                    onClick={() => onDuplicateClip(clip.clipId)}
                  >
                    <Copy />
                  </button>
                  <button
                    className="delete"
                    title={
                      clip.takeRole === "primary" && clip.takeGroupId
                        ? "删除整组素材"
                        : "删除素材"
                    }
                    aria-label={`删除素材 ${index + 1}`}
                    onClick={() => onDeleteClip(clip.clipId)}
                  >
                    <Trash2 />
                  </button>
                </span>
              </div>
            );
          })}
          {clipEdits.length === 0 && (
            <div
              className={`clip-list-empty ${
                highlightHero ? "filtered" : ""
              }`}
            >
              {highlightHero ? (
                <SearchX />
              ) : (
                <Clapperboard />
              )}
              <strong>
                {highlightHero
                  ? "本局没有符合筛选的片段"
                  : "还没有剪辑片段"}
              </strong>
              <span>
                {highlightHero
                  ? `${heroLabel(
                      highlightHero,
                    )}没有检测到“${selectedRuleLabel}”高光。`
                  : "新增一段后即可设置时间和镜头。"}
              </span>
              {!highlightHero && (
                <button className="primary-button" onClick={onAddClip}>
                  <Plus />
                  <span>新增片段</span>
                </button>
              )}
            </div>
          )}
        </div>
      </section>

      <aside className="result-inspector">
        <div className="inspector-toolbar">
        <div className="display-mode-switch" aria-label="画面显示模式">
          <span>
            <small>画面显示</small>
            <strong>
              {renderSettings.cleanHud ? "干净画面" : "普通画面"}
            </strong>
          </span>
          <div>
            <button
              className={!renderSettings.cleanHud ? "active" : ""}
              type="button"
              aria-pressed={!renderSettings.cleanHud}
              onClick={() => onUpdateRenderSettings({ cleanHud: false })}
            >
              <Video />
              <span>普通画面</span>
            </button>
            <button
              className={renderSettings.cleanHud ? "active" : ""}
              type="button"
              aria-pressed={renderSettings.cleanHud}
              onClick={() => onUpdateRenderSettings({ cleanHud: true })}
            >
              <ShieldCheck />
              <span>干净画面</span>
            </button>
          </div>
        </div>
        <div
          className="inspector-tabs"
        >

          <button
            className={inspectorTab === "hero" ? "active" : ""}
            onClick={() => setInspectorTab("hero")}
          >
            主角
          </button>
          <button
            className={inspectorTab === "edit" ? "active" : ""}
            onClick={() => setInspectorTab("edit")}
          >
            时间
          </button>
          <button
            className={inspectorTab === "camera" ? "active" : ""}
            onClick={() => setInspectorTab("camera")}
          >
            机位
          </button>
        </div>
        </div>

        {selectedClip && inspectorTab === "edit" && (
          <>
            <div className="inspector-section">
              <span className="eyebrow">
                片段 {clipEdits.findIndex((clip) => clip.clipId === selectedClip.clipId) + 1}
              </span>
              <h2>
                {selectedCandidate
                  ? candidateTitle(selectedCandidate)
                  : selectedClip.imported?.title ?? "自定义片段"}
              </h2>
              <div className="template-meta">
                <span>{formatDuration(selectedClip.endSeconds - selectedClip.startSeconds)}</span>
                <span>{formatTimecode(selectedClip.startSeconds)}</span>
                <span>{formatTimecode(selectedClip.endSeconds)}</span>
              </div>
            </div>

            {selectedCandidate?.kind === "hero_kill_sequence" &&
              selectedCandidate.kill_sequence && (
                <div className="interaction-evidence">
                  <div className="interaction-evidence-heading">
                    <span className="interaction-evidence-icon">
                      <Sparkles />
                    </span>
                    <span>
                      <strong>
                        {heroLabel(selectedCandidate.kill_sequence.hero)}
                        击杀连续段
                      </strong>
                      <small>
                        技能/交战起手到最后一次英雄死亡
                      </small>
                    </span>
                  </div>
                  <p>
                    从{" "}
                    {actionLabel(
                      selectedCandidate.kill_sequence.kills[0]?.setup_action,
                    )}{" "}
                    起手，到{" "}
                    {formatTimecode(selectedCandidate.peak_seconds)}
                    完成本段最后一次击杀；相邻击杀保持在同一片段中。
                  </p>
                  <div className="interaction-evidence-tags">
                    <span>
                      本局第{" "}
                      {selectedCandidate.kill_sequence.sequence_index}/
                      {selectedCandidate.kill_sequence.sequence_count} 段
                    </span>
                    <span>
                      本段 {selectedCandidate.kill_sequence.kills.length} 次击杀
                    </span>
                    <span>
                      本局共 {selectedCandidate.kill_sequence.total_kills} 次击杀
                    </span>
                    {selectedCandidate.kill_sequence.kills.map((kill) => (
                      <span key={`${kill.death_tick}-${kill.target_hero}`}>
                        {actionLabel(kill.setup_action)} →{" "}
                        {heroLabel(kill.target_hero)} ·{" "}
                        {formatTimecode(kill.death_time_seconds)}
                      </span>
                    ))}
                  </div>
                </div>
              )}

            {selectedCandidate?.kind === "hero_kill_sequence" &&
              selectedCandidate.kill_sequence && (
                <KillDurationEditor
                  clip={selectedClip}
                  candidate={selectedCandidate}
                  replayDuration={result.replay.playback_time_seconds}
                  onUpdate={(patch) =>
                    onUpdateClip(selectedClip.clipId, patch)
                  }
                />
              )}

            {(selectedCandidate?.kind !== "hero_kill_sequence" ||
              selectedClip.durationMode === "short") && (
              <PrecisionTimeEditor
                clip={selectedClip}
                replayDuration={result.replay.playback_time_seconds}
                peakSeconds={selectedCandidate?.peak_seconds ?? selectedClip.imported?.anchorSeconds ?? null}
                onUpdate={(patch) => onUpdateClip(selectedClip.clipId, patch)}
              />
            )}

            <div className="inspector-section source-anchor">
              <div className="section-row">
                <span className="section-title">时间定位锚点</span>
                <span className="section-note">
                  {anchorCandidates.length} 个事件
                </span>
              </div>
              <select
                value={selectedClip.candidateId}
                onChange={(event) =>
                  onUpdateClip(selectedClip.clipId, {
                    candidateId: event.target.value,
                  })
                }
              >
                {selectedClip.imported && !anchorCandidates.some(candidate => candidate.id === selectedClip.candidateId) && (
                  <option value={selectedClip.candidateId}>
                    {selectedClip.imported.title} · {formatTimecode(selectedClip.imported.anchorSeconds)}
                  </option>
                )}
                {anchorCandidates.map((candidate) => (
                  <option value={candidate.id} key={candidate.id}>
                    {candidate.rank}. {candidateTitle(candidate)} ·{" "}
                    {formatTimecode(candidate.peak_seconds)}
                  </option>
                ))}
              </select>
              <small>
                锚点用于把录像时间换算为 DEM tick，不会自动改动当前入点和出点。
              </small>
            </div>

            <div className="quality-strip">
              <div>
                <SlidersHorizontal />
                <span>30 FPS 帧级步进</span>
              </div>
              <strong>游戏原声</strong>
            </div>
          </>
        )}
        {inspectorTab === "hero" && (
          <>
            <div className="inspector-section settings-heading">
              <span className="eyebrow">
                英雄高光筛选
              </span>
              <h2>选择主角</h2>
              <p>
                选择后，片段清单会重建为该英雄的击杀连续段。
              </p>
            </div>
            <div className="inspector-section">
              <div className="section-row">
                <span className="section-title">整局英雄阵容</span>
                <span className="section-note">
                  {highlightHero
                    ? `${heroLabel(highlightHero)} · ${selectedRuleLabel}`
                    : `已识别 ${rosterPlayers.length}/10`}
                </span>
              </div>
              <div className="hero-chip-list selectable">
                {rosterPlayers.map((player) => (
                  <button
                    className={`hero-chip ${
                      player.hero_name === highlightHero
                        ? "primary"
                        : ""
                    } ${player.game_team === 2 ? "radiant" : ""} ${
                      player.game_team === 3 ? "dire" : ""
                    }`}
                    key={`${player.slot}-${player.hero_name}`}
                    title={`${teamLabel(player.game_team)} · ${
                      player.player_name || `玩家位 ${player.slot + 1}`
                    }`}
                    onClick={() => onSelectHighlightHero(player.hero_name)}
                  >
                    <span>{player.slot + 1}</span>
                    {heroLabel(player.hero_name)}
                  </button>
                ))}
              </div>
            </div>
            <div className="inspector-section highlight-filter-section">
              <div className="section-row">
                <span className="section-title">高光内容</span>
                <button
                  className={`highlight-select-all ${
                    highlightRuleIds.length === HIGHLIGHT_RULES.length
                      ? "selected"
                      : ""
                  }`}
                  type="button"
                  onClick={() => {
                    const allRules = [...DEFAULT_HIGHLIGHT_RULE_IDS];
                    onUpdateHighlightRules(allRules);
                    setHighlightQueryFeedback({
                      kind: "success",
                      message: highlightHero
                        ? `已显示${heroLabel(highlightHero)}的全部已支持高光。`
                        : "已选择全部内容，再选择高光主角。",
                    });
                  }}
                >
                  <ListFilter />
                  <span>全部</span>
                </button>
              </div>
              <div className="highlight-rule-grid">
                {HIGHLIGHT_RULES.map((rule) => {
                  const selected = highlightRuleIds.includes(rule.id);
                  const count = highlightHero
                    ? heroHighlightCandidateCount(
                        result,
                        highlightHero,
                        rule.id,
                      )
                    : null;
                  return (
                    <button
                      className={`highlight-rule-button ${
                        selected ? "selected" : ""
                      }`}
                      key={rule.id}
                      type="button"
                      aria-pressed={selected}
                      title={rule.description}
                      onClick={() => toggleHighlightRule(rule.id)}
                    >
                      <Swords />
                      <span>
                        <strong>{rule.label}</strong>
                        <small>{count === null ? "待选主角" : `${count} 段`}</small>
                      </span>
                      <Check />
                    </button>
                  );
                })}
              </div>
              {highlightQueryFeedback && (
                <div
                  className={`highlight-query-feedback ${highlightQueryFeedback.kind}`}
                  role="status"
                  ref={highlightQueryFeedbackRef}
                >
                  {highlightQueryFeedback.kind === "success" ? (
                    <CheckCircle2 />
                  ) : (
                    <CircleAlert />
                  )}
                  <span>{highlightQueryFeedback.message}</span>
                </div>
              )}
              </div>
          </>
        )}
        {inspectorTab === "camera" && (
          <>
            {selectedClip && (
              <div className="inspector-section">
                <div className="section-row">
                  <span className="section-title">当前片段机位</span>
                  <Camera />
                </div>
                <div className="choice-list camera-choice-list">
                  <button
                    className={
                      selectedClip.cameraMode === "player_perspective"
                        ? "selected"
                        : ""
                    }
                    onClick={() =>
                      onUpdateClip(selectedClip.clipId, {
                        cameraMode: "player_perspective",
                      })
                    }
                  >
                    <Eye />
                    <span>
                      <strong>玩家视角（默认）</strong>
                      <small>Player View · 高光主角玩家本人的操作视角</small>
                    </span>
                    <Check />
                  </button>
                  <button
                    className={
                      selectedClip.cameraMode === "hero_chase"
                        ? "selected"
                        : ""
                    }
                    onClick={() =>
                      onUpdateClip(selectedClip.clipId, {
                        cameraMode: "hero_chase",
                      })
                    }
                  >
                    <UserRound />
                    <span>
                      <strong>英雄跟随</strong>
                      <small>Hero Chase · 跟随当前英雄，适合作为备用机位</small>
                    </span>
                    <Check />
                  </button>
                </div>
                <CameraModeDiagram mode={selectedClip.cameraMode} />
              </div>
            )}
          </>
        )}
        {!selectedClip && inspectorTab === "edit" && (
          <div className="inspector-empty">
            <Clapperboard />
            <strong>选择或新增一个片段</strong>
            <span>片段的精确入点与出点会显示在这里。</span>
          </div>
        )}
        {!selectedClip && inspectorTab === "camera" && (
          <div className="inspector-empty">
            <Camera />
            <strong>先选择一个片段</strong>
            <span>每个片段都可以独立选择玩家视角或英雄跟随机位。</span>
          </div>
        )}
      </aside>
    </div>
  );
}

interface ReplayEventTimelineProps {
  result: AnalysisSummary;
  clipEdits: ClipEdits;
  selectedClip: ClipEditState | null;
  highlightHero: string;
  onSelectClip: (clipId: string) => void;
  onActivateCandidate: (candidateId: string) => void;
}

function ReplayEventTimeline({
  result,
  clipEdits,
  selectedClip,
  highlightHero,
  onSelectClip,
  onActivateCandidate,
}: ReplayEventTimelineProps) {
  const [scope, setScope] = useState<TimelineScope>(
    highlightHero ? "hero" : "all",
  );
  const duration = result.replay.playback_time_seconds;
  const events = useMemo(
    () =>
      buildTimelineEvents(
        result.highlights.candidates,
        highlightHero,
        scope,
      ),
    [highlightHero, result.highlights.candidates, scope],
  );
  const ticks = useMemo(() => timelineTicks(duration, 4), [duration]);
  const candidateById = useMemo(
    () =>
      new Map(
        result.highlights.candidates.map((candidate) => [
          candidate.id,
          candidate,
        ]),
      ),
    [result.highlights.candidates],
  );
  const includedCandidateIds = useMemo(
    () => new Set(clipEdits.map((clip) => clip.candidateId)),
    [clipEdits],
  );

  useEffect(() => {
    setScope(highlightHero ? "hero" : "all");
  }, [highlightHero, result.job_id]);

  return (
    <section className="event-timeline" aria-label="整局事件时间轴">
      <header className="event-timeline-header">
        <span>
          <small>整局事件轴</small>
          <strong>
            {scope === "hero" && highlightHero
              ? `${heroLabel(highlightHero)}相关`
              : "全部英雄"}{" "}
            · {events.length} 个候选
          </strong>
        </span>
        <div className="timeline-scope-control" aria-label="时间轴显示范围">
          <button
            className={scope === "all" ? "active" : ""}
            type="button"
            aria-pressed={scope === "all"}
            onClick={() => setScope("all")}
          >
            整局
          </button>
          <button
            className={scope === "hero" ? "active" : ""}
            type="button"
            aria-pressed={scope === "hero"}
            disabled={!highlightHero}
            onClick={() => setScope("hero")}
          >
            相关
          </button>
        </div>
      </header>

      <div className="timeline-chart">
        <div className="timeline-row clip-track-row">
          <span className="timeline-row-label">片段</span>
          <div className="timeline-rail">
            {clipEdits.map((clip, index) => {
              const style = timelineRangeStyle(
                clip.startSeconds,
                clip.endSeconds,
                duration,
              );
              return (
                <button
                  className={`timeline-clip-range ${
                    selectedClip?.clipId === clip.clipId ? "active" : ""
                  }`}
                  key={clip.clipId}
                  type="button"
                  style={style}
                  title={`片段 ${index + 1} · ${formatTimecode(
                    clip.startSeconds,
                  )} - ${formatTimecode(clip.endSeconds)}`}
                  aria-label={`选择片段 ${index + 1}`}
                  onClick={() => onSelectClip(clip.clipId)}
                />
              );
            })}
          </div>
        </div>

        {TIMELINE_LANES.map((lane) => {
          const laneEvents = events.filter((event) => event.lane === lane.id);
          return (
            <div className="timeline-row" key={lane.id}>
              <span className="timeline-row-label">{lane.label}</span>
              <div className={`timeline-rail ${lane.id}`}>
                {laneEvents.map((event) => {
                  const candidate = candidateById.get(event.candidateId);
                  if (!candidate) {
                    return null;
                  }
                  const included = includedCandidateIds.has(candidate.id);
                  const active =
                    selectedClip?.candidateId === candidate.id;
                  return (
                    <button
                      className={`timeline-event-marker ${
                        included ? "included" : ""
                      } ${active ? "active" : ""}`}
                      key={candidate.id}
                      type="button"
                      style={timelineRangeStyle(
                        event.startSeconds,
                        event.endSeconds,
                        duration,
                      )}
                      title={`${formatTimecode(event.timeSeconds)} · ${candidateTitle(
                        candidate,
                      )}${included ? " · 已在片段清单" : " · 点击添加片段"}`}
                      aria-label={`${formatTimecode(
                        event.timeSeconds,
                      )}，${candidateTitle(candidate)}`}
                      onClick={() => onActivateCandidate(candidate.id)}
                    />
                  );
                })}
              </div>
            </div>
          );
        })}

        <div className="timeline-row timeline-ticks">
          <span className="timeline-row-label" />
          <div className="timeline-rail">
            {ticks.map((tick, index) => (
              <span
                className={`timeline-tick ${
                  index === 0
                    ? "first"
                    : index === ticks.length - 1
                      ? "last"
                      : ""
                }`}
                key={`${tick}-${index}`}
                style={{ left: `${timelinePercent(tick, duration)}%` }}
              >
                {formatTimelineTick(tick)}
              </span>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}

function PrecisionTimeEditor({
  clip,
  replayDuration: rawReplayDuration,
  peakSeconds,
  onUpdate,
}: {
  clip: ClipEditState;
  replayDuration: number;
  peakSeconds: number | null;
  onUpdate: (patch: Partial<ClipEditState>) => void;
}) {
  const replayDuration = replayEndFrame(rawReplayDuration);
  const frame = 1 / 30;
  const minDuration = 1;

  function setStart(value: number) {
    onUpdate({
      startSeconds: roundFrame(
        clamp(value, 0, Math.max(0, clip.endSeconds - minDuration)),
      ),
    });
  }

  function setEnd(value: number) {
    onUpdate({
      endSeconds: roundFrame(
        clamp(value, clip.startSeconds + minDuration, replayDuration),
      ),
    });
  }

  function shift(delta: number) {
    const duration = clip.endSeconds - clip.startSeconds;
    const nextStart = clamp(
      clip.startSeconds + delta,
      0,
      Math.max(0, replayDuration - duration),
    );
    onUpdate({
      startSeconds: roundFrame(nextStart),
      endSeconds: roundFrame(nextStart + duration),
    });
  }

  return (
    <div className="inspector-section precision-editor">
      <div className="section-row">
        <span className="section-title">精确入点 / 出点</span>
        <span className="section-note">HH:MM:SS.mmm</span>
      </div>

      <div className="timecode-grid">
        <TimecodeField
          label="入点"
          value={clip.startSeconds}
          max={Math.max(0, clip.endSeconds - minDuration)}
          onCommit={setStart}
        />
        <TimecodeField
          label="出点"
          value={clip.endSeconds}
          min={clip.startSeconds + minDuration}
          max={replayDuration}
          onCommit={setEnd}
        />
      </div>

      <TimeAdjuster
        label="调整入点"
        value={clip.startSeconds}
        onChange={setStart}
      />
      <TimeAdjuster
        label="调整出点"
        value={clip.endSeconds}
        onChange={setEnd}
      />

      <div className="range-editor">
        <label>
          <span>入点</span>
          <input
            type="range"
            min={0}
            max={Math.max(0, clip.endSeconds - minDuration)}
            step={frame}
            value={clip.startSeconds}
            onChange={(event) => setStart(Number(event.target.value))}
          />
        </label>
        <label>
          <span>出点</span>
          <input
            type="range"
            min={Math.min(replayDuration, clip.startSeconds + minDuration)}
            max={replayDuration}
            step={frame}
            value={clip.endSeconds}
            onChange={(event) => setEnd(Number(event.target.value))}
          />
        </label>
        <div className="range-scale">
          <span>00:00</span>
          {peakSeconds !== null &&
            peakSeconds >= clip.startSeconds &&
            peakSeconds <= clip.endSeconds && (
              <span className="peak-marker">
                事件 {formatTimecode(peakSeconds)}
              </span>
            )}
          <span>{formatTimecode(replayDuration)}</span>
        </div>
      </div>

      <div className="clip-shift-row">
        <span>整体移动</span>
        <button onClick={() => shift(-1)}>
          <SkipBack />
          1 秒
        </button>
        <button onClick={() => shift(1)}>
          1 秒
          <SkipForward />
        </button>
        <strong>
          {formatDuration(clip.endSeconds - clip.startSeconds)}
        </strong>
      </div>
    </div>
  );
}

function KillDurationEditor({
  clip,
  candidate,
  replayDuration,
  onUpdate,
}: {
  clip: ClipEditState;
  candidate: HighlightCandidate;
  replayDuration: number;
  onUpdate: (patch: Partial<ClipEditState>) => void;
}) {
  const storyRange = killSequenceRange(
    candidate,
    clip.storyPreRollSeconds,
    replayDuration,
  );
  const firstKillSeconds =
    candidate.kill_sequence?.kills
      .map((kill) => kill.death_time_seconds)
      .sort((left, right) => left - right)[0] ?? candidate.peak_seconds;
  const actualPreRoll = Math.max(0, firstKillSeconds - clip.startSeconds);
  const wasTrimmed =
    clip.durationMode === "story" &&
    actualPreRoll + 1 / 30 < clip.storyPreRollSeconds;

  function selectShortMode() {
    onUpdate({
      durationMode: "short",
      startSeconds: clip.shortStartSeconds,
      endSeconds: clip.shortEndSeconds,
    });
  }

  function selectStoryMode() {
    onUpdate({
      durationMode: "story",
      storyPreRollSeconds: clip.storyPreRollSeconds,
    });
  }

  function setPreRoll(value: number) {
    onUpdate({
      durationMode: "story",
      storyPreRollSeconds: Math.round(
        clamp(value, 0, MAX_STORY_PRE_ROLL_SECONDS),
      ),
    });
  }

  return (
    <div className="inspector-section duration-editor">
      <div className="section-row">
        <span className="section-title">片段时长</span>
        <span className="section-note">逐段设置</span>
      </div>
      <div className="duration-mode-control" role="group" aria-label="片段时长模式">
        <button
          className={clip.durationMode === "short" ? "selected" : ""}
          onClick={selectShortMode}
        >
          短击杀（默认）
        </button>
        <button
          className={clip.durationMode === "story" ? "selected" : ""}
          onClick={selectStoryMode}
        >
          完整交战
        </button>
      </div>

      {clip.durationMode === "story" && storyRange && (
        <div className="story-duration-controls">
          <label className="story-preroll-field">
            <span>交战铺垫</span>
            <input
              type="range"
              min={0}
              max={MAX_STORY_PRE_ROLL_SECONDS}
              step={5}
              value={clip.storyPreRollSeconds}
              onChange={(event) => setPreRoll(Number(event.target.value))}
            />
            <span className="story-preroll-value">
              <input
                type="number"
                min={0}
                max={MAX_STORY_PRE_ROLL_SECONDS}
                step={5}
                value={clip.storyPreRollSeconds}
                onChange={(event) => setPreRoll(Number(event.target.value))}
              />
              秒
            </span>
          </label>
          <div className="story-duration-summary">
            <span>
              {formatTimecode(clip.startSeconds)} - {formatTimecode(clip.endSeconds)}
            </span>
            <strong>{formatDuration(clip.endSeconds - clip.startSeconds)}</strong>
          </div>
          <small>
            最后一次击杀后固定保留 {STORY_POST_ROLL_SECONDS} 秒。
            {wasTrimmed
              ? ` 为避开录像边界或相邻片段重复，实际前置 ${formatDuration(actualPreRoll)}。`
              : " 连续击杀会保留在同一段内。"}
          </small>
        </div>
      )}
    </div>
  );
}

function TimecodeField({
  label,
  value,
  min = 0,
  max,
  onCommit,
}: {
  label: string;
  value: number;
  min?: number;
  max: number;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(formatTimecode(value));
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) {
      setDraft(formatTimecode(value));
    }
  }, [editing, value]);

  function commit() {
    const parsed = parseTimecode(draft);
    if (parsed === null) {
      setDraft(formatTimecode(value));
    } else {
      onCommit(clamp(parsed, min, max));
    }
    setEditing(false);
  }

  return (
    <label className="timecode-field">
      <span>{label}</span>
      <input
        value={draft}
        inputMode="decimal"
        spellCheck={false}
        onFocus={() => setEditing(true)}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.currentTarget.blur();
          }
          if (event.key === "Escape") {
            setDraft(formatTimecode(value));
            event.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}

function TimeAdjuster({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
}) {
  const controls = [
    { label: "-10", value: -10 },
    { label: "-1", value: -1 },
    { label: "-1f", value: -1 / 30 },
    { label: "+1f", value: 1 / 30 },
    { label: "+1", value: 1 },
    { label: "+10", value: 10 },
  ];
  return (
    <div className="time-adjuster">
      <span>{label}</span>
      <div>
        {controls.map((control) => (
          <button
            key={control.label}
            onClick={() => onChange(value + control.value)}
            title={
              control.label.endsWith("f")
                ? `${control.label} 帧`
                : `${control.label} 秒`
            }
          >
            {control.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function SettingSwitch({
  label,
  note,
  checked,
  disabled = false,
  onChange,
}: {
  label: string;
  note: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <div className={`setting-switch-row ${disabled ? "disabled" : ""}`}>
      <span>
        <strong>{label}</strong>
        <small>{note}</small>
      </span>
      <label className="switch-control">
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(event) => onChange(event.target.checked)}
        />
        <span />
      </label>
    </div>
  );
}

function MovieSetupDialog({
  version,
  result,
  clipEdits,
  capabilities,
  renderSettings,
  saving,
  rendering,
  renderProgress,
  renderResult,
  renderError,
  outputDirectory,
  onOutputDirectoryChange,
  onOutputError,
  resume,
  checkingResume,
  resumeCheckError,
  feedback,
  onClose,
  onStart,
  onResume,
  onCancel,
  onOpenPath,
}: {
  version: string;
  result: AnalysisSummary;
  clipEdits: ClipEdits;
  capabilities: Capabilities;
  renderSettings: RenderSettings;
  saving: boolean;
  rendering: boolean;
  renderProgress: RenderProgress | null;
  renderResult: RenderResult | null;
  renderError: string;
  outputDirectory: string|null;
  onOutputDirectoryChange: (value:string|null)=>void;
  onOutputError: (message:string)=>void;
  resume: RenderResumeSummary | null;
  checkingResume: boolean;
  resumeCheckError: string;
  feedback: string;
  onClose: () => void;
  onStart: () => void;
  onResume: () => void;
  onCancel: () => void;
  onOpenPath: (path: string) => void;
}) {
  const clips = planClips(clipEdits);
  const totalDuration = clips.reduce(
    (total, clip) =>
      total +
      (clip.includeInFinal
        ? Math.max(
            0,
            clip.sourceEndSeconds - clip.sourceStartSeconds,
          )
        : 0),
    0,
  );
  const finalClipCount = clips.filter(
    (clip) => clip.includeInFinal,
  ).length;
  const assetLabels = clipAssetLabels(clipEdits);
  const sceneCount = new Set(
    clips.map((clip) => clip.takeGroupId ?? `standalone:${clip.clipId}`),
  ).size;
  const hasInvalidClip = clips.some((clip) =>
    !isValidClipTimeRange(clip.sourceStartSeconds, clip.sourceEndSeconds, result.replay.playback_time_seconds),
  );

  const [cleaningCache, setCleaningCache] = useState(false);
  const [exportingDiagnostics, setExportingDiagnostics] = useState(false);
  const [diagnosticArchive, setDiagnosticArchive] = useState<RenderDiagnosticArchive | null>(null);
  const [diagnosticExportError, setDiagnosticExportError] = useState("");
  const diagnosticExporter = useRef(new RenderDiagnosticExporter());
  const active = saving || rendering || checkingResume || cleaningCache || exportingDiagnostics;
  const resumeView = resumeAction(resume);
  const hasResumePanel = (!rendering || checkingResume) && (checkingResume || !!resumeView || !!resumeCheckError);
  const errorPresentation = renderError ? presentRenderError(renderError) : null;
  const [diagnosticCopied,setDiagnosticCopied]=useState(false);
  const lastProgress=useRef<RenderProgress|null>(null);
  useEffect(()=>{if(renderProgress)lastProgress.current=renderProgress},[renderProgress]);
  useEffect(()=>{
    setDiagnosticCopied(false);
    setDiagnosticArchive(null);
    setDiagnosticExportError("");
  },[renderError,result.job_id]);
  function diagnosticReport(){
    return renderErrorReport(renderError||"当前界面无失败原文，请参阅包内历史记录。",{version,module:"高光时刻",matchId:result.replay.match_id,jobId:result.job_id,
      stage:lastProgress.current?.stage,currentClip:lastProgress.current?.currentClip,totalClips:clips.length,
      logDirectory:`${capabilities.jobsRoot}\\${result.job_id}\\render\\logs`});
  }
  async function copyDiagnostic(){
    try{
      await navigator.clipboard.writeText(diagnosticReport());
      setDiagnosticCopied(true);
    }catch{setDiagnosticCopied(false)}
  }
  async function exportDiagnostic(){
    if(active||diagnosticExporter.current.busy)return;
    setExportingDiagnostics(true);
    setDiagnosticExportError("");
    const outcome=await diagnosticExporter.current.run(isTauriRuntime,{jobId:result.job_id,report:diagnosticReport()},
      request=>invoke<RenderDiagnosticArchive|null>("export_render_diagnostics",{...request}));
    if(outcome.status==="exported")setDiagnosticArchive(outcome.archive);
    if(outcome.status==="failed")setDiagnosticExportError(outcome.message);
    setExportingDiagnostics(false);
  }
  async function openDiagnosticDirectory(){
    if(!isTauriRuntime||!diagnosticArchive)return;
    try{
      await invoke("open_local_path",{path:parentPath(diagnosticArchive.archivePath)});
      setDiagnosticExportError("");
    }catch(reason:unknown){setDiagnosticExportError(toErrorMessage(reason))}
  }
  const previewSource =
    renderResult && isTauriRuntime
      ? convertFileSrc(renderResult.outputPath)
      : renderResult && developmentFixtureEnabled ? "/export-layout-preview.mp4" : "";
  const expectedOutputName = fileName(result.source.path).replace(
    /\.dem$/i,
    ".mp4",
  );

  useEffect(() => {
    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape" && !active) {
        onClose();
      }
    }
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [active, onClose]);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !active) {
          onClose();
        }
      }}
    >
      <section
        className={`movie-dialog ${renderResult ? "with-preview" : ""} ${hasResumePanel ? "with-resume" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="movie-dialog-title"
      >
        <header className="movie-dialog-header">
          <span className="movie-dialog-icon">
            <Film />
          </span>
          <span>
            <small>默认剪辑</small>
            <h2 id="movie-dialog-title">导出视频</h2>
          </span>
          <button
            className="icon-button"
            onClick={onClose}
            disabled={active}
            title="关闭"
            aria-label="关闭成片设置"
          >
            <X />
          </button>
        </header>

        <div className="movie-dialog-body">
        <div
          className={`render-readiness ${
            renderError ? "failed" : capabilities.renderReady ? "ready" : "waiting"
          }`}
        >
          {capabilities.renderReady && !renderError ? <CheckCircle2 /> : <CircleAlert />}
          <span>
            <strong>
              {renderError
                ? "本次导出未完成"
                : capabilities.renderReady
                ? "本地成片环境已就绪"
                : "当前机器缺少成片环境"}
            </strong>
            <small>
              {renderError
                ? "请查看下方错误说明，处理后可重新导出。"
                : capabilities.renderReady
                ? resumeView && resume?.remainingClips === 0
                  ? "所有片段已保存，可直接继续合成。"
                  : "开始录制后会自动启动离线 Dota 2，完成或取消后自动关闭。"
                : capabilities.renderReason}
            </small>
          </span>
        </div>

        <VideoOutputLocation capabilities={capabilities} jobId={result.job_id} matchId={result.replay.match_id} module="高光时刻" directory={outputDirectory} onChange={onOutputDirectoryChange} disabled={active} native={isTauriRuntime} onError={onOutputError}/>
        <p className="dota-path-help movie-export-note">开始录制前，请先手动打开并登录 Steam，保持 Steam 运行。软件不会自动启动或登录 Steam。</p>
        <div className="movie-plan-summary">
          <div>
            <ListChecks />
            <span>
              <small>源素材</small>
              <strong>{clips.length}</strong>
            </span>
          </div>
          <div>
            <Clock3 />
            <span>
              <small>成片时长</small>
              <strong>{formatDuration(totalDuration)}</strong>
            </span>
          </div>
          <div>
            <Camera />
            <span>
              <small>素材场次</small>
              <strong>{sceneCount}</strong>
            </span>
          </div>
          <div>
            <UserRound />
            <span>
              <small>声音</small>
              <strong>仅游戏原声</strong>
            </span>
          </div>
        </div>

        <div className="movie-output-name">
          <FileVideo2 />
          <span>
            <small>输出文件</small>
            <strong>{expectedOutputName}</strong>
          </span>
          <span>{renderSettings.cleanHud ? "干净画面" : "普通画面"}</span>
          <span>重复导出自动编号</span>
        </div>

        {(!rendering || checkingResume) && (
          checkingResume ? <div className="render-resume-panel" role="status">
            <LoaderCircle className="spin" /><span>正在检查已保存片段，请稍候…</span>
          </div> : resumeView ? <div className="render-resume-panel" role="status">
            <CheckCircle2 />
            <div>
              <strong>{resumeView.title}</strong>
              <p>{resumeView.description}</p>
              {resume!.unverifiedCameraClips > 0 && <p>
                已保存文件检查通过；软件未自动识别目标英雄。请查看镜头，确认正确后点击继续。
              </p>}
              {resume?.completedDirectory && <button className="secondary-button"
                onClick={() => onOpenPath(resume.completedDirectory!)}>
                <FolderOpen />打开已保存片段
              </button>}
            </div>
          </div> : resumeCheckError ? <div className="dialog-feedback error" role="alert">
            <CircleAlert /><span>{resumeCheckError}</span>
          </div> : null
        )}

        {renderResult ? (
          <div className="render-complete-panel">
            <div className="render-complete-summary">
              <span className="render-complete-icon">
                <CheckCircle2 />
              </span>
              <span>
                <strong>高光成片已经完成</strong>
                <small>
                  {formatDuration(renderResult.durationSeconds)} ·{" "}
                  {renderResult.width}x{renderResult.height} ·{" "}
                  {renderResult.segmentCount} 段成片 ·{" "}
                  {renderResult.sourceAssetCount} 份源素材
                </small>
              </span>
            </div>
            {previewSource && (
              <video
                className="render-video-player"
                key={renderResult.outputPath}
                src={previewSource}
                controls
                preload="metadata"
                playsInline
              />
            )}
            <CacheCleanupPanel key={renderResult.qcReportPath} jobId={result.job_id}
              reportPath={renderResult.qcReportPath}
              disabled={saving || rendering || checkingResume || !isTauriRuntime}
              onBusyChange={setCleaningCache} />
            {renderResult.warnings.length > 0 && (
              <div className="render-warning-list">
                {renderResult.warnings.map((warning) => (
                  <span key={warning}>{warning}</span>
                ))}
              </div>
            )}
          </div>
        ) : rendering || (renderProgress && renderProgress.status === "running") ? (
          <div className="render-progress-panel" aria-live="polite">
            <div className="render-progress-copy">
              <LoaderCircle className="spin" />
              <span>
                <strong>{renderProgress?.message ?? "正在准备成片"}</strong>
                <small>
                  {renderProgress?.totalClips
                    ? `片段 ${renderProgress.currentClip}/${renderProgress.totalClips}`
                    : "正在执行本地预检"}
                </small>
              </span>
              <strong>{renderProgress?.percent ?? 0}%</strong>
            </div>
            <div className="render-progress-track">
              <span
                style={{ width: `${Math.max(2, renderProgress?.percent ?? 0)}%` }}
              />
            </div>
            <div className="render-safety-note">
              <ShieldCheck />
              <span>正在逐帧保存原生画面；游戏窗口可能慢于实时播放，成片为 60 FPS。请不要操作 Dota 2，结束后客户端会自动关闭。</span>
            </div>
          </div>
        ) : (
          <div className="movie-plan-list">
            {clips.map((clip, index) => {
              const candidate = result.highlights.candidates.find(
                (item) => item.id === clip.candidateId,
              );
              return (
                <div className="movie-plan-row" key={clip.clipId}>
                  <span className="movie-plan-index">
                    {assetLabels.get(clip.clipId) ?? index + 1}
                  </span>
                  <span className="movie-plan-copy">
                    <strong title={candidate?candidateTitle(candidate):clip.imported?.title??clip.candidateId}>
                      {candidate
                        ? candidateTitle(candidate)
                        : clip.imported?.title ?? clip.candidateId}
                    </strong>
                    <small>
                      {clip.includeInFinal ? "默认入片" : "备用素材"} ·{" "}
                      {formatTimecode(clip.sourceStartSeconds)} -{" "}
                      {formatTimecode(clip.sourceEndSeconds)} ·{" "}
                      {cameraModeShortLabel(clip.cameraMode)}
                    </small>
                  </span>
                  <span className="movie-plan-hero">
                    <UserRound />
                    <span className="movie-plan-hero-name" title={heroLabel(clip.viewHero)}>{heroLabel(clip.viewHero)}</span>
                  </span>
                </div>
              );
            })}
            {clips.length === 0 && (
              <div className="movie-plan-empty">
                <ListChecks />
                <span>当前没有入选片段</span>
              </div>
            )}
          </div>
        )}

        {hasInvalidClip && (
          <div className="dialog-feedback error">
            <CircleAlert />
            <span>
              片段必须位于录像内，且单段时长为 1 到 {MAX_CLIP_DURATION_SECONDS} 秒。
            </span>
          </div>
        )}
        {errorPresentation && (
          <div className="dialog-feedback error export-error" role="alert">
            <CircleAlert />
            <div className="export-error-content">
              <strong>{errorPresentation.title}</strong>
              <small>错误编号：{errorPresentation.code}</small>
              <p>{errorPresentation.message}</p>
              <ul>{errorPresentation.steps.map(step=><li key={step}>{step}</li>)}</ul>
              <div className="diagnostic-export-actions">
                <button className="secondary-button" type="button" onClick={()=>void copyDiagnostic()}>{diagnosticCopied?"排查信息已复制":"复制排查信息"}</button>
                <button className="diagnostic-export-button" type="button" onClick={()=>void exportDiagnostic()} disabled={active||!isTauriRuntime}
                  title={isTauriRuntime?"将本局日志与排查信息保存为 ZIP 文件":"请在桌面软件中导出排查包"}>
                  {exportingDiagnostics?"正在导出排查包…":"导出排查包"}
                </button>
              </div>
              {!isTauriRuntime&&<small className="diagnostic-export-note">浏览器预览无法导出排查包，请在桌面软件中操作。</small>}
              {errorPresentation.details && (
                <details>
                  <summary>技术详情</summary>
                  <pre>{errorPresentation.details}</pre>
                </details>
              )}
            </div>
          </div>
        )}
        {!errorPresentation&&!rendering&&(
          <div className="dialog-feedback diagnostic-export-panel">
            <FileText />
            <div className="diagnostic-export-copy">
              <button className="diagnostic-export-button" type="button" onClick={()=>void exportDiagnostic()} disabled={active||!isTauriRuntime}
                title={isTauriRuntime?"保存本局日志，重新打开任务后也可导出":"请在桌面软件中导出排查包"}>
                {exportingDiagnostics?"正在导出排查包…":"导出排查包"}
              </button>
              <small className="diagnostic-export-note">{isTauriRuntime?"遇到导出问题时，可将本局排查包发给维护者。":"浏览器预览无法导出排查包，请在桌面软件中操作。"}</small>
            </div>
          </div>
        )}
        {diagnosticArchive&&(
          <div className="dialog-feedback" role="status">
            <CheckCircle2 />
            <div className="export-error-content">
              <strong>排查包已保存（{diagnosticArchive.fileCount} 个文件）</strong>
              <p>{diagnosticArchive.archivePath}</p>
              {diagnosticArchive.warnings.map((warning,index)=><p key={`${index}:${warning}`}>{warning}</p>)}
              <button className="secondary-button" type="button" onClick={()=>void openDiagnosticDirectory()} disabled={active}>打开所在文件夹</button>
            </div>
          </div>
        )}
        {diagnosticExportError&&(
          <div className="dialog-feedback error" role="alert">
            <CircleAlert /><span>排查包操作未完成：{diagnosticExportError}</span>
          </div>
        )}
        {feedback && !renderError && (
          <div className="dialog-feedback">
            <CheckCircle2 />
            <span>{feedback}</span>
          </div>
        )}

        </div>
        <footer className="movie-dialog-footer">
          {renderResult ? (
            <>
              <button className="secondary-button" onClick={onClose} disabled={active}>
                关闭
              </button>
              <button
                className="secondary-button"
                onClick={() =>
                  onOpenPath(parentPath(renderResult.outputPath))
                }
              >
                <FolderOpen />
                <span>打开文件夹</span>
              </button>
              {renderResult.sourceAssetsDir && (
                <button
                  className="secondary-button"
                  onClick={() =>
                    onOpenPath(renderResult.sourceAssetsDir)
                  }
                >
                  <FolderOpen />
                  <span>打开源素材</span>
                </button>
              )}
              <button
                className="secondary-button"
                onClick={() => onOpenPath(renderResult.outputPath)}
              >
                <Play />
                <span>外部播放</span>
              </button>
              {resumeView && <button className="primary-button" onClick={onResume}
                disabled={active || !capabilities.renderReady || finalClipCount === 0 || hasInvalidClip}>
                <Play /><span>{resumeView.button}</span>
              </button>}
              <button
                className={resumeView ? "secondary-button" : "primary-button"}
                disabled={
                  active ||
                  !capabilities.renderReady ||
                  finalClipCount === 0 ||
                  hasInvalidClip
                }
                onClick={onStart}
              >
                <RefreshCw />
                <span>{resumeView ? "重新录制全部" : "按当前方案重新生成"}</span>
              </button>
            </>
          ) : rendering ? (
            <button className="danger-button" onClick={onCancel}>
              <Pause />
              <span>取消并关闭 Dota 2</span>
            </button>
          ) : (
            <>
              <button className="secondary-button" onClick={onClose} disabled={active}>
                关闭
              </button>
              {resumeView && <button className="primary-button" onClick={onResume}
                disabled={active || !capabilities.renderReady || finalClipCount === 0 || hasInvalidClip}>
                <Play /><span>{resumeView.button}</span>
              </button>}
              <button
                className={resumeView ? "secondary-button" : "primary-button"}
                disabled={
                  active ||
                  !capabilities.renderReady ||
                  finalClipCount === 0 ||
                  hasInvalidClip
                }
                onClick={onStart}
                title={
                  capabilities.renderReady
                    ? "保存方案并按当前时间与镜头导出"
                    : capabilities.renderReason ?? undefined
                }
              >
                {saving ? <LoaderCircle className="spin" /> : <Film />}
                <span>{checkingResume ? "正在检查已保存片段" : saving ? "正在保存方案" : resumeView ? "重新录制全部" : "开始导出视频"}</span>
              </button>
            </>
          )}
        </footer>
      </section>
    </div>
  );
}

function SummaryStat({
  icon,
  label,
  value,
  tone,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  tone: string;
}) {
  return (
    <div className={`summary-stat ${tone}`}>
      <span className="summary-icon">{icon}</span>
      <span>
        <small>{label}</small>
        <strong>{value}</strong>
      </span>
    </div>
  );
}

function LibraryView({
  snapshot,
  directory,
  busy,
  loading,
  error,
  onOpen,
  onImport,
  onRefresh,
  onChooseDirectory,
}: {
  snapshot: ReplayLibrarySnapshot | null;
  directory: string;
  busy: boolean;
  loading: boolean;
  error: string;
  onOpen: (file: ReplayLibraryFile) => void;
  onImport: () => void;
  onRefresh: () => void;
  onChooseDirectory: () => void;
}) {
  const files = snapshot?.files ?? [];
  const displayDirectory = displayReplayDirectory(snapshot?.directory || directory);
  return (
    <section className="library-view replay-library">
      <div className="library-heading">
        <div>
          <span className="eyebrow">本地录像 · 下载时间从新到旧</span>
          <h2>录像库</h2>
        </div>
        <div className="library-actions">
          <button className="secondary-button" onClick={onChooseDirectory} disabled={busy}>
            <FolderOpen /><span>选择目录</span>
          </button>
          <button className="secondary-button" onClick={onRefresh} disabled={loading}>
            {loading ? <LoaderCircle className="spinning" /> : <RefreshCw />}<span>刷新</span>
          </button>
          <button className="primary-button" onClick={onImport} disabled={busy}>
            <FileUp /><span>导入录像</span>
          </button>
        </div>
      </div>
      <p className="replay-library-directory" title={displayDirectory || undefined}>
        {displayDirectory || "尚未设置录像目录"}
        {snapshot && <span className="library-count"> · {files.length} 个录像</span>}
      </p>
      {error && <p className="replay-library-error" role="alert">{error}</p>}
      <div className="library-table">
        <div className="library-table-head">
          <span>录像</span>
          <span>大小</span>
          <span title="优先使用本机文件创建时间，缺失时使用修改时间。">下载时间 ↓</span>
          <span>操作</span>
          <span />
        </div>
        {files.map((file) => (
          <button
            className="library-row"
            disabled={busy}
            key={file.path}
            onClick={() => onOpen(file)}
            title={`载入 ${file.name}`}
          >
            <span className="library-file">
              <FileVideo2 />
              <span>
                <strong>{file.name}</strong>
                <small>Dota 2 本地录像</small>
              </span>
            </span>
            <span>{formatBytes(file.byteLength)}</span>
            <time title={replayTimeDescription(file.timeBasis)}>
              {formatReplayDownloadTime(file.downloadedAtUnixMs)}
            </time>
            <span>载入高光</span>
            <ChevronRight />
          </button>
        ))}
        {files.length === 0 && (
          <div className="library-empty">
            <Library />
            <span>{loading ? "正在读取本地录像…" : error ? "请检查目录后刷新，或重新选择目录。" : "此目录还没有 .dem 录像。"}</span>
          </div>
        )}
      </div>
    </section>
  );
}

function Statusbar({
  progress,
  hasResult,
  capabilities,
}: {
  progress: AnalysisProgress | null;
  hasResult: boolean;
  capabilities: Capabilities;
}) {
  return (
    <footer className="statusbar">
      <div className="pipeline-status">
        {pipelineStages.map((stage, index) => {
          const active = progress?.stage === stage.id;
          const complete =
            hasResult ||
            progress?.stage === "complete" ||
            pipelineStages.findIndex((item) => item.id === progress?.stage) >
              index;
          return (
            <div className="statusbar-piece" key={stage.id}>
              <span
                className={`pipeline-step ${complete ? "complete" : ""} ${
                  active ? "active" : ""
                }`}
              >
                {complete ? <Check /> : active ? <LoaderCircle /> : <span />}
                {stage.label}
              </span>
              {index < pipelineStages.length - 1 && (
                <span className={`pipeline-line ${complete ? "complete" : ""}`} />
              )}
            </div>
          );
        })}
      </div>
      <div className="status-copy">
        <span>{capabilities.renderReady ? "成片环境可用" : "当前可分析录像"}</span>
        <span className="keyboard-hint">本地模式</span>
      </div>
    </footer>
  );
}

function InlineError({ message }: { message: string }) {
  return (
    <div className="inline-error">
      <CircleAlert />
      <span>{message}</span>
    </div>
  );
}

function fileName(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function parentPath(path: string) {
  const index = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return index > 0 ? path.slice(0, index) : path;
}

function isDemPath(path: string) {
  return path.toLocaleLowerCase("en-US").endsWith(".dem");
}

function replayPlayers(result: AnalysisSummary): ReplayPlayer[] {
  if (result.replay.players?.length) {
    return result.replay.players;
  }
  return Array.from(
    new Set(
      result.highlights.candidates.flatMap(
        (candidate) => candidate.participants,
      ),
    ),
  ).map((hero_name, slot) => ({
    slot,
    hero_name,
    player_name: null,
    game_team: null,
    is_fake_client: false,
  }));
}

function teamLabel(team: number | null) {
  if (team === 2) {
    return "天辉";
  }
  if (team === 3) {
    return "夜魇";
  }
  return "阵容";
}

function createSavedDefaultPlan(
  result: AnalysisSummary,
  saved: LoadedEditPlan,
  fallback: EditorPlanSnapshot,
): EditorPlanSnapshot {
  const slot = saved.plans.find((plan) => plan.mode === "default");
  if (!slot) return fallback;
  const clips = createClipEdits(result, slot);
  const inferred = inferHighlightSelection(result.highlights.candidates, clips);
  const savedRules = slot.highlightRuleIds.filter(isHighlightRuleId);
  const selectedClipId = slot.selectedClipId && clips.some((clip) => clip.clipId === slot.selectedClipId)
    ? slot.selectedClipId : (clips.find((clip) => clip.includeInFinal)?.clipId ?? clips[0]?.clipId ?? "");
  return {
    clipEdits: clips, selectedClipId, highlightHero: slot.selectedHero ?? inferred.hero,
    highlightRuleIds: savedRules.length > 0 ? normalizeHighlightRuleIds(savedRules) : inferred.ruleIds,
    renderSettings: sanitizeRenderSettings(slot.settings),
  };
}

function isHighlightRuleId(value: string): value is HighlightRuleId {
  return HIGHLIGHT_RULES.some((rule) => rule.id === value);
}

function toEditPlanSlot(
  mode: "default",
  snapshot: EditorPlanSnapshot,
): EditPlanSlot {
  return {
    mode,
    selectedHero: snapshot.highlightHero || null,
    highlightRuleIds: snapshot.highlightRuleIds,
    selectedClipId: snapshot.selectedClipId || null,
    sourceStoryId: null,
    clips: planClips(snapshot.clipEdits),
    settings: snapshot.renderSettings,
  };
}

function createClipEdits(
  result: AnalysisSummary,
  savedPlan?: Pick<EditPlanSlot, "clips">,
): ClipEdits {
  if (savedPlan) {
    const clips = savedPlan.clips.map((clip, index) => {
      const range = frameRangeWithinReplay(clip.sourceStartSeconds, clip.sourceEndSeconds,
        result.replay.playback_time_seconds);
      const shortRange = frameRangeWithinReplay(
        clip.shortSourceStartSeconds ?? clip.sourceStartSeconds,
        clip.shortSourceEndSeconds ?? clip.sourceEndSeconds,
        result.replay.playback_time_seconds);
      return {
      clipId: clip.clipId || `clip-saved-${String(index + 1).padStart(2, "0")}`,
      candidateId: clip.candidateId,
      imported: clip.imported ? { ...clip.imported } : null,
      viewHero:
        clip.viewHero ??
        result.replay.players[0]?.hero_name ??
        "",
      cameraMode: normalizeUserCameraMode(clip.cameraMode),
      takeGroupId: clip.takeGroupId ?? null,
      takeRole: clip.takeRole ?? "primary",
      includeInFinal: clip.includeInFinal ?? true,
      durationMode: clip.durationMode ?? "short",
      storyPreRollSeconds:
        clip.storyPreRollSeconds ?? DEFAULT_STORY_PRE_ROLL_SECONDS,
      shortStartSeconds: shortRange.startSeconds,
      shortEndSeconds: shortRange.endSeconds,
      ...range,
      };
    });
    return normalizeStoryDurationClips(
      clips,
      result.highlights.candidates,
      result.replay.playback_time_seconds,
    );
  }

  const sourceCandidates = result.highlights.candidates
    .filter((candidate) => candidate.kind === "hero_kill_sequence")
    .slice(0, 3);

  return sourceCandidates.map((candidate, index) => {
    const start = candidate.start_seconds;
    const unclampedEnd = candidate.end_seconds;
    const end = Math.min(
      replayEndFrame(result.replay.playback_time_seconds),
      start + Math.min(90, Math.max(1, unclampedEnd - start)),
    );
    return {
      clipId: `clip-recommended-${String(index + 1).padStart(2, "0")}`,
      candidateId: candidate.id,
      viewHero:
        candidate.primary_hero ??
        candidate.participants[0] ??
        result.replay.players[0]?.hero_name ??
        "",
      cameraMode: "player_perspective",
      takeGroupId: null,
      takeRole: "primary",
      includeInFinal: true,
      ...shortDurationState(start, end),
      startSeconds: roundFrame(start),
      endSeconds: roundFrame(end),
    };
  });
}

function heroHighlightCandidates(
  result: AnalysisSummary,
  hero: string,
  ruleIds: readonly HighlightRuleId[] = DEFAULT_HIGHLIGHT_RULE_IDS,
) {
  return filterHeroHighlightCandidates(
    result.highlights.candidates,
    hero,
    ruleIds,
  );
}

function createHeroHighlightClips(
  result: AnalysisSummary,
  hero: string,
  ruleIds: readonly HighlightRuleId[] = DEFAULT_HIGHLIGHT_RULE_IDS,
): ClipEdits {
  return heroHighlightCandidates(result, hero, ruleIds).map((candidate, index) => {
    const range = frameRangeWithinReplay(candidate.start_seconds, candidate.end_seconds,
      result.replay.playback_time_seconds);
    return {
      clipId: `clip-hero-${String(index + 1).padStart(2, "0")}`,
      candidateId: candidate.id,
      viewHero: hero,
      cameraMode: "player_perspective",
      takeGroupId: null,
      takeRole: "primary",
      includeInFinal: true,
      ...shortDurationState(range.startSeconds, range.endSeconds),
      ...range,
    };
  });
}

function takeRoleLabel(role: StoryTakeRole) {
  return role === "primary" ? "主机位" : "备用机位";
}

function clipAssetLabels(clips: ClipEdits) {
  return buildAssetLabels(
    clips.map((clip) => ({
      id: clip.clipId,
      groupId: clip.takeGroupId,
      role: clip.takeRole,
    })),
  );
}

function buildAssetLabels(
  assets: Array<{
    id: string;
    groupId: string | null;
    role: StoryTakeRole;
  }>,
) {
  const groups = new Map<
    string,
    Array<{ id: string; role: StoryTakeRole }>
  >();
  for (const asset of assets) {
    const groupKey = asset.groupId ?? `standalone:${asset.id}`;
    const group = groups.get(groupKey) ?? [];
    group.push({ id: asset.id, role: asset.role });
    groups.set(groupKey, group);
  }

  const labels = new Map<string, string>();
  Array.from(groups.values()).forEach((group, sceneIndex) => {
    const ordered = [
      ...group.filter((asset) => asset.role === "primary"),
      ...group.filter((asset) => asset.role === "alternate"),
    ];
    ordered.forEach((asset, takeIndex) => {
      labels.set(
        asset.id,
        `S${String(sceneIndex + 1).padStart(3, "0")}-${takeCode(
          takeIndex,
        )}`,
      );
    });
  });
  return labels;
}

function takeCode(index: number) {
  let value = index + 1;
  let code = "";
  while (value > 0) {
    value -= 1;
    code = String.fromCharCode(65 + (value % 26)) + code;
    value = Math.floor(value / 26);
  }
  return code;
}

function heroKillCount(result: AnalysisSummary, hero: string) {
  return result.highlights.candidates
    .filter(
      (candidate) =>
        candidate.kind === "hero_kill_sequence" &&
        candidate.primary_hero === hero,
    )
    .reduce(
      (total, candidate) =>
        total + (candidate.kill_sequence?.kills.length ?? candidate.hero_deaths),
      0,
    );
}

function heroHighlightCandidateCount(
  result: AnalysisSummary,
  hero: string,
  ruleId: HighlightRuleId,
) {
  return heroHighlightCandidates(result, hero, [ruleId]).length;
}

function heroHighlightSelectionDetail(
  result: AnalysisSummary,
  hero: string,
  ruleIds: readonly HighlightRuleId[],
) {
  const selectedRules = new Set(ruleIds);
  return HIGHLIGHT_RULES.filter((rule) => selectedRules.has(rule.id))
    .map((rule) =>
      rule.id === HERO_KILL_RULE_ID
        ? `${heroKillCount(result, hero)} 次击杀`
        : `${heroHighlightCandidateCount(result, hero, rule.id)} 段${
            rule.label
          }`,
    )
    .join(" · ");
}

function heroHighlightSummary(
  result: AnalysisSummary,
  hero: string,
  ruleIds: readonly HighlightRuleId[],
  clipCount: number,
) {
  const normalizedRules = normalizeHighlightRuleIds(ruleIds);
  if (
    normalizedRules.length === 1 &&
    normalizedRules[0] === HERO_KILL_RULE_ID
  ) {
    return { label: "主角击杀", value: `${heroKillCount(result, hero)} 次` };
  }

  return { label: "高光片段", value: `${clipCount} 段` };
}

function planClips(clipEdits: ClipEdits): EditPlanClip[] {
  return clipEdits.map((clip) => ({
    clipId: clip.clipId,
    candidateId: clip.candidateId,
    imported: clip.imported ? { ...clip.imported } : null,
    viewHero: clip.viewHero || null,
    cameraMode: clip.cameraMode,
    takeGroupId: clip.takeGroupId,
    takeRole: clip.takeRole,
    includeInFinal: clip.includeInFinal,
    durationMode: clip.durationMode,
    storyPreRollSeconds: clip.storyPreRollSeconds,
    shortSourceStartSeconds: roundFrame(clip.shortStartSeconds),
    shortSourceEndSeconds: roundFrame(clip.shortEndSeconds),
    sourceStartSeconds: roundFrame(clip.startSeconds),
    sourceEndSeconds: roundFrame(clip.endSeconds),
  }));
}

function shortDurationState(startSeconds: number, endSeconds: number) {
  return {
    durationMode: "short" as const,
    storyPreRollSeconds: DEFAULT_STORY_PRE_ROLL_SECONDS,
    shortStartSeconds: roundFrame(startSeconds),
    shortEndSeconds: roundFrame(endSeconds),
  };
}

function sanitizeRenderSettings(
  settings: RenderSettings | null | undefined,
): RenderSettings {
  return {
    ...defaultRenderSettings,
    cleanHud: settings?.cleanHud ?? defaultRenderSettings.cleanHud,
  };
}

function createClipId() {
  return `clip-${crypto.randomUUID()}`;
}

function copyImportedMetadata(imported: ImportedClip | null | undefined): ImportedClip | null {
  if (!imported) return null;
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return {
    ...imported,
    sourceId: Array.from(bytes, value => value.toString(16).padStart(2, "0")).join(""),
    candidateSourceId: imported.candidateSourceId ?? imported.sourceId,
  };
}

function normalizeUserCameraMode(
  mode: ClipCameraMode | null | undefined,
): ClipCameraMode {
  return mode === "hero_chase" ? "hero_chase" : "player_perspective";
}

function cameraModeShortLabel(mode: ClipCameraMode) {
  const labels: Record<ClipCameraMode, string> = {
    directed: "玩家视角",
    free_camera: "玩家视角",
    hero_chase: "英雄跟随",
    player_perspective: "玩家视角",
  };
  return labels[mode];
}

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatDuration(seconds: number) {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return "0 秒";
  }
  if (seconds < 60) {
    return `${seconds.toFixed(seconds < 10 ? 1 : 0)} 秒`;
  }
  const minutes = Math.floor(seconds / 60);
  const rest = Math.floor(seconds % 60);
  return `${minutes}:${rest.toString().padStart(2, "0")}`;
}

function formatTimelineTick(seconds: number) {
  const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const minutes = Math.floor(safe / 60);
  const rest = Math.floor(safe % 60);
  return `${minutes}:${rest.toString().padStart(2, "0")}`;
}

function timelineRangeStyle(
  startSeconds: number,
  endSeconds: number,
  durationSeconds: number,
) {
  const left = timelinePercent(startSeconds, durationSeconds);
  const right = timelinePercent(
    Math.max(startSeconds, endSeconds),
    durationSeconds,
  );
  const width = Math.min(100 - left, Math.max(0.8, right - left));
  return {
    left: `${left}%`,
    width: `${width}%`,
  };
}

function formatTimecode(seconds: number) {
  const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const totalMilliseconds = Math.round(safe * 1000);
  const hours = Math.floor(totalMilliseconds / 3_600_000);
  const minutes = Math.floor((totalMilliseconds % 3_600_000) / 60_000);
  const wholeSeconds = Math.floor((totalMilliseconds % 60_000) / 1000);
  const milliseconds = totalMilliseconds % 1000;
  return `${hours.toString().padStart(2, "0")}:${minutes
    .toString()
    .padStart(2, "0")}:${wholeSeconds
    .toString()
    .padStart(2, "0")}.${milliseconds.toString().padStart(3, "0")}`;
}

function parseTimecode(value: string) {
  const text = value.trim().replace(",", ".");
  if (!text) {
    return null;
  }
  const parts = text.split(":");
  if (parts.length > 3 || parts.some((part) => part.trim() === "")) {
    return null;
  }
  const values = parts.map(Number);
  if (values.some((part) => !Number.isFinite(part) || part < 0)) {
    return null;
  }
  if (values.length === 1) {
    return values[0] ?? null;
  }
  if (values.length === 2) {
    return (values[0] ?? 0) * 60 + (values[1] ?? 0);
  }
  return (
    (values[0] ?? 0) * 3600 +
    (values[1] ?? 0) * 60 +
    (values[2] ?? 0)
  );
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

function candidateTitle(candidate: HighlightCandidate) {
  if (candidate.kind === "hero_kill_sequence" && candidate.kill_sequence) {
    const killCount = candidate.kill_sequence.kills.length;
    if (killCount > 1) {
      return `${heroLabel(candidate.kill_sequence.hero)}连续击杀 · ${killCount} 次`;
    }
    const target = candidate.kill_sequence.kills[0]?.target_hero;
    return `${heroLabel(candidate.kill_sequence.hero)}击杀${
      target ? ` · ${heroLabel(target)}` : ""
    }`;
  }

  if (candidate.kind === "first_blood") {
    return "一血爆发";
  }
  if (candidate.kind === "multikill") {
    return `连续交战 · 双方共 ${candidate.hero_deaths} 次英雄阵亡`;
  }
  if (candidate.kind === "team_fight") {
    return `大型团战 · 双方共 ${candidate.hero_deaths} 次英雄阵亡`;
  }
  if (candidate.kind.includes("roshan")) {
    return "肉山争夺与收益";
  }
  if (candidate.kind === "objective") {
    return "关键目标推进";
  }
  return `关键交锋 · 双方共 ${candidate.hero_deaths} 次英雄阵亡`;
}

function actionLabel(action?: string) {
  const known: Record<string, string> = {
    basic_attack: "普通攻击",
    mirana_arrow: "月神之箭",
    mirana_starfall: "群星风暴",
    mirana_leap: "跳跃",
    mirana_celestial_quiver: "月神箭效果",
    item_mjollnir: "雷神之锤",
  };
  if (!action) {
    return "交战";
  }
  return (
    known[action] ??
    action
      .replace(/^npc_dota_hero_/, "")
      .replace(/^item_/, "")
      .replaceAll("_", " ")
  );
}

function toErrorMessage(reason: unknown) {
  if (reason instanceof Error) {
    return reason.message;
  }
  return String(reason);
}

export default App;
