import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";
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
  Eye,
  FileUp,
  FileVideo2,
  Film,
  FolderOpen,
  Library,
  ListFilter,
  ListChecks,
  LoaderCircle,
  Minus,
  Pause,
  Play,
  Plus,
  RefreshCw,
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
  Trees,
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
import mascot from "./assets/cat-editor-mascot.png";
import { heroLabel } from "./heroes";
import {
  DEFAULT_HIGHLIGHT_RULE_IDS,
  HERO_KILL_RULE_ID,
  HIGHLIGHT_RULES,
  VERIFIED_TREE_CUT_RULE_ID,
  filterHeroHighlightCandidates,
  formatHighlightRuleSelection,
  inferHighlightSelection,
  normalizeHighlightRuleIds,
  parseHighlightRuleQuery,
} from "./highlightRules";
import type { HighlightRuleId } from "./highlightRules";
import type {
  AnalysisProgress,
  AnalysisSummary,
  Capabilities,
  ClipCameraMode,
  EditPlanClip,
  HighlightCandidate,
  LoadedEditPlan,
  RecentJob,
  ReplayLookupResult,
  ReplayPlayer,
  RenderProgress,
  RenderResult,
  RenderSettings,
  SaveEditPlanResult,
  StageStatus,
} from "./types";

type View = "workbench" | "library";
type TitlebarPanel = "notifications" | "settings" | null;
type InspectorTab = "edit" | "camera";

interface AppNotice {
  kind: "success" | "error";
  title: string;
  message: string;
}

interface ClipEditState {
  clipId: string;
  candidateId: string;
  viewHero: string;
  cameraMode: ClipCameraMode;
  startSeconds: number;
  endSeconds: number;
}

type ClipEdits = ClipEditState[];

const isTauriRuntime = "__TAURI_INTERNALS__" in window;
const appWindow = isTauriRuntime ? getCurrentWindow() : null;
const replayDirectoryStorageKey = "cat-cut-replay-directory";

const emptyCapabilities: Capabilities = {
  analysisReady: true,
  renderReady: false,
  ffmpegFound: false,
  ffprobeFound: false,
  dota2Found: false,
  renderReason: null,
  jobsRoot: "",
  recommendedReplayDirectory: null,
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
  { id: "direct", label: "编排" },
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
  const [renderProgress, setRenderProgress] = useState<RenderProgress | null>(
    null,
  );
  const [renderResult, setRenderResult] = useState<RenderResult | null>(null);
  const [renderError, setRenderError] = useState("");
  const [completionNoticeEnabled, setCompletionNoticeEnabled] = useState(
    () => localStorage.getItem("cat-cut-completion-notice") !== "disabled",
  );
  const [importDialogOpen, setImportDialogOpen] = useState(false);
  const [replayDirectory, setReplayDirectory] = useState(
    () => localStorage.getItem(replayDirectoryStorageKey) ?? "",
  );
  const [replayId, setReplayId] = useState("");
  const [replayLookupBusy, setReplayLookupBusy] = useState(false);
  const [replayLookupError, setReplayLookupError] = useState("");

  useEffect(() => {
    if (!isTauriRuntime) {
      return;
    }
    void Promise.all([
      invoke<Capabilities>("get_capabilities"),
      invoke<RecentJob[]>("get_recent_jobs"),
    ])
      .then(([capabilityResult, jobs]) => {
        setCapabilities(capabilityResult);
        setRecentJobs(jobs);
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
        if (busy) {
          setError("当前录像仍在分析，请完成后再导入下一份。");
          return;
        }

        setSelectedPath(path);
        rememberReplayPath(path);
        setImportDialogOpen(false);
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
  }, [busy]);

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

  useEffect(() => {
    if (!result) {
      setClipEdits([]);
      setHighlightHero("");
      setHighlightRuleIds([...DEFAULT_HIGHLIGHT_RULE_IDS]);
      setMovieSetupOpen(false);
      setPlanFeedback("");
      setRenderSettings(defaultRenderSettings);
      setRenderProgress(null);
      setRenderResult(null);
      setRenderError("");
      return;
    }

    let disposed = false;
    const initialClips = createClipEdits(result);
    const initialSelection = inferHighlightSelection(
      result.highlights.candidates,
      initialClips,
    );
    setClipEdits(initialClips);
    setHighlightHero(initialSelection.hero);
    setHighlightRuleIds(initialSelection.ruleIds);
    setRenderSettings(defaultRenderSettings);
    setPlanFeedback("");
    setRenderProgress(null);
    setRenderResult(null);
    setRenderError("");
    if (isTauriRuntime) {
      void invoke<LoadedEditPlan | null>("get_edit_plan", {
        jobId: result.job_id,
      })
        .then((saved) => {
          if (!disposed && saved) {
            const savedClips = createClipEdits(result, saved);
            const savedSelection = inferHighlightSelection(
              result.highlights.candidates,
              savedClips,
            );
            setClipEdits(savedClips);
            setHighlightHero(savedSelection.hero);
            setHighlightRuleIds(savedSelection.ruleIds);
            setRenderSettings(sanitizeRenderSettings(saved.settings));
          }
        })
        .catch(() => {
          // A missing or stale optional edit plan must not block replay analysis.
        });
      void invoke<RenderResult | null>("get_latest_render", {
        jobId: result.job_id,
      })
        .then((latestRender) => {
          if (!disposed) {
            setRenderResult(latestRender);
          }
        })
        .catch(() => {
          // An optional previous render must not block editing or a new render.
        });
    }

    return () => {
      disposed = true;
    };
  }, [result]);

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

  function openImportDialog() {
    if (busy) {
      return;
    }
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
      }
    } catch (reason: unknown) {
      setReplayLookupError(toErrorMessage(reason));
    }
  }

  async function chooseReplayFile() {
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
        rememberReplayPath(picked);
        setSelectedPath(picked);
        setImportDialogOpen(false);
        await runAnalysis(picked);
      }
    } catch (reason: unknown) {
      setReplayLookupError(toErrorMessage(reason));
    }
  }

  async function loadReplayById() {
    if (busy || replayLookupBusy) {
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
      setReplayDirectory(resolved.replayDirectory);
      setReplayId(resolved.replayId);
      setSelectedPath(resolved.path);
      setImportDialogOpen(false);
      await runAnalysis(resolved.path);
    } catch (reason: unknown) {
      setReplayLookupError(toErrorMessage(reason));
    } finally {
      setReplayLookupBusy(false);
    }
  }

  async function runAnalysis(path = selectedPath) {
    if (!path || busy) {
      return;
    }
    setView("workbench");
    setBusy(true);
    setError("");
    setResult(null);
    setCompletedStages([]);
    setProgress({
      stage: "ingest",
      status: "running",
      message: "正在准备分析",
    });

    try {
      const channel = new Channel<AnalysisProgress>();
      channel.onmessage = (message) => {
        setProgress(message);
        if (message.status === "complete") {
          setCompletedStages((current) =>
            current.includes(message.stage)
              ? current
              : [...current, message.stage],
          );
        }
      };
      const summary = await invoke<AnalysisSummary>("analyze_dem", {
        demPath: path,
        onProgress: channel,
      });
      setResult(summary);
      setProgress({
        stage: "complete",
        status: "complete",
        message: summary.reused_existing_job
          ? "已载入之前的分析结果"
          : "录像分析完成",
      });
      setRecentJobs(await invoke<RecentJob[]>("get_recent_jobs"));
      if (completionNoticeEnabled) {
        const heroSequences = summary.highlights.candidates.filter(
          (candidate) => candidate.kind === "hero_kill_sequence",
        ).length;
        setNotice({
          kind: "success",
          title: "录像分析完成",
          message: `${fileName(path)} · ${heroSequences} 段英雄击杀 · 可按英雄筛选`,
        });
      }
    } catch (reason: unknown) {
      const message = toErrorMessage(reason);
      setError(message);
      if (completionNoticeEnabled) {
        setNotice({
          kind: "error",
          title: "录像分析失败",
          message,
        });
      }
      setProgress({
        stage: progress?.stage ?? "parse",
        status: "failed",
        message: "分析没有完成",
      });
    } finally {
      setBusy(false);
    }
  }

  async function openRecent(job: RecentJob) {
    setSelectedPath(job.sourcePath);
    await runAnalysis(job.sourcePath);
  }

  function updateClipEdit(clipId: string, patch: Partial<ClipEditState>) {
    setClipEdits((current) => {
      return current.map((clip) =>
        clip.clipId === clipId ? { ...clip, ...patch } : clip,
      );
    });
    setPlanFeedback("");
  }

  function applyHeroHighlightSelection(
    hero: string,
    ruleIds: readonly HighlightRuleId[],
    announce: boolean,
  ) {
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
    applyHeroHighlightSelection(hero, highlightRuleIds, true);
  }

  function updateHighlightRules(ruleIds: HighlightRuleId[]) {
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
    if (!result) {
      return;
    }
    const selected = clipEdits.find((clip) => clip.clipId === selectedClipId);
    const fallbackCandidate = result.highlights.candidates[0] ?? null;
    if (!selected && !fallbackCandidate) {
      return;
    }
    const replayDuration = result.replay.playback_time_seconds;
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
      viewHero: hero,
      cameraMode: "player_perspective",
      startSeconds: roundFrame(baseStart),
      endSeconds: roundFrame(Math.min(replayDuration, baseStart + 12)),
    };
    setClipEdits([...clipEdits, clip]);
    setSelectedClipId(clip.clipId);
    setPlanFeedback("");
  }

  function duplicateClip(clipId: string) {
    const index = clipEdits.findIndex((clip) => clip.clipId === clipId);
    const source = clipEdits[index];
    if (!source) {
      return;
    }
    const copy = { ...source, clipId: createClipId() };
    const next = [...clipEdits];
    next.splice(index + 1, 0, copy);
    setClipEdits(next);
    setSelectedClipId(copy.clipId);
    setPlanFeedback("");
  }

  function deleteClip(clipId: string) {
    const index = clipEdits.findIndex((clip) => clip.clipId === clipId);
    if (index < 0) {
      return;
    }
    const next = clipEdits.filter((clip) => clip.clipId !== clipId);
    const fallback = next[Math.min(index, Math.max(0, next.length - 1))];
    setClipEdits(next);
    setSelectedClipId(fallback?.clipId ?? "");
    setPlanFeedback("");
  }

  function moveClip(clipId: string, direction: -1 | 1) {
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
    setRenderSettings((current) => ({ ...current, ...patch }));
    setPlanFeedback("");
    setRenderError("");
  }

  async function saveEditPlan(showNotice = true) {
    if (!result || savingPlan) {
      return null;
    }
    const clips = planClips(clipEdits);
    if (clips.length === 0) {
      setPlanFeedback("请至少启用一个高光片段。");
      return null;
    }

    setSavingPlan(true);
    setPlanFeedback("");
    try {
      const saved = await invoke<SaveEditPlanResult>("save_edit_plan", {
        request: {
          jobId: result.job_id,
          mode: "manual",
          clips,
          settings: renderSettings,
        },
      });
      const message = `已保存 ${saved.selectedClipCount} 个片段，共 ${formatDuration(
        saved.totalDurationSeconds,
      )}`;
      setPlanFeedback(message);
      if (showNotice) {
        setNotice({
          kind: "success",
          title: "剪辑方案已保存",
          message,
        });
      }
      return saved;
    } catch (reason: unknown) {
      const message = toErrorMessage(reason);
      setPlanFeedback(message);
      setNotice({
        kind: "error",
        title: "剪辑方案保存失败",
        message,
      });
      return null;
    } finally {
      setSavingPlan(false);
    }
  }

  async function startMovieRender() {
    if (!result || rendering) {
      return;
    }
    if (!capabilities.renderReady) {
      setRenderError(
        capabilities.renderReason ?? "当前成片环境不完整，请检查 Dota 2 和媒体工具。",
      );
      return;
    }

    setRenderError("");
    setRenderResult(null);
    setRenderProgress({
      stage: "preflight",
      status: "running",
      message: "正在保存方案并准备成片",
      percent: 1,
      currentClip: 0,
      totalClips: 0,
    });
    const saved = await saveEditPlan(false);
    if (!saved) {
      return;
    }

    setRendering(true);
    try {
      const channel = new Channel<RenderProgress>();
      channel.onmessage = (message) => {
        setRenderProgress(message);
        if (message.status === "failed") {
          setRenderError(message.message);
        }
      };
      const completed = await invoke<RenderResult>("start_render", {
        jobId: result.job_id,
        onProgress: channel,
      });
      setRenderResult(completed);
      setRenderProgress({
        stage: "complete",
        status: "complete",
        message: "成片已完成，Dota 2 已关闭",
        percent: 100,
        currentClip: completed.segmentCount,
        totalClips: completed.segmentCount,
      });
      setNotice({
        kind: "success",
        title: "高光成片已完成",
        message: `${formatDuration(completed.durationSeconds)} · ${completed.width}x${completed.height}`,
      });
    } catch (reason: unknown) {
      const message = toErrorMessage(reason);
      setRenderError(message);
      setNotice({
        kind: "error",
        title: "成片任务没有完成",
        message,
      });
    } finally {
      setRendering(false);
      const refreshed = await invoke<Capabilities>("get_capabilities").catch(
        () => null,
      );
      if (refreshed) {
        setCapabilities(refreshed);
      }
    }
  }

  async function cancelMovieRender() {
    if (!rendering) {
      return;
    }
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

  return (
    <div className={`app-shell ${dragActive ? "drag-active" : ""}`}>
      <Titlebar
        notice={notice}
        completionNoticeEnabled={completionNoticeEnabled}
        onCompletionNoticeChange={setCompletionNoticeEnabled}
        onClearNotice={() => setNotice(null)}
      />
      <Sidebar
        activeView={view}
        capabilities={capabilities}
        recentJobs={recentJobs}
        busy={busy}
        onImport={openImportDialog}
        onViewChange={setView}
        onOpenRecent={(job) => void openRecent(job)}
      />

      <main className="workspace">
        <ProjectHeader
          selectedPath={selectedPath}
          result={result}
          busy={busy}
          renderReady={capabilities.renderReady}
          onImport={openImportDialog}
          onAnalyze={() => void runAnalysis()}
          onOpenMovieSetup={() => setMovieSetupOpen(true)}
        />

        {view === "library" ? (
          <LibraryView
            jobs={recentJobs}
            onOpen={(job) => void openRecent(job)}
            onImport={openImportDialog}
          />
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
          result={result}
          clipEdits={clipEdits}
          capabilities={capabilities}
          renderSettings={renderSettings}
          saving={savingPlan}
          rendering={rendering}
          renderProgress={renderProgress}
          renderResult={renderResult}
          renderError={renderError}
          feedback={planFeedback}
          onClose={() => {
            if (!rendering) {
              setMovieSetupOpen(false);
            }
          }}
          onStart={() => void startMovieRender()}
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
  onCompletionNoticeChange: (enabled: boolean) => void;
  onClearNotice: () => void;
}

function Titlebar({
  notice,
  completionNoticeEnabled,
  onCompletionNoticeChange,
  onClearNotice,
}: TitlebarProps) {
  const [panel, setPanel] = useState<TitlebarPanel>(null);
  const actionsRef = useRef<HTMLDivElement>(null);

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
            <div className="settings-version">版本 1.6.0</div>
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
  busy: boolean;
  onImport: () => void;
  onViewChange: (view: View) => void;
  onOpenRecent: (job: RecentJob) => void;
}

function Sidebar({
  activeView,
  capabilities,
  recentJobs,
  busy,
  onImport,
  onViewChange,
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
          label="制作台"
          active={activeView === "workbench"}
          onClick={() => onViewChange("workbench")}
        />
        <NavButton
          icon={<Library />}
          label="录像库"
          count={recentJobs.length}
          active={activeView === "library"}
          onClick={() => onViewChange("library")}
        />
      </nav>

      <div className="sidebar-section">
        <div className="section-label">最近任务</div>
        <div className="sidebar-jobs">
          {recentJobs.slice(0, 2).map((job, index) => (
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
            <div className="no-recent">还没有分析记录</div>
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
          disabled={!result}
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
          <span className="eyebrow">本地录像分析</span>
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
            <h2>最近分析</h2>
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
                placeholder="输入纯数字录像编号"
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
  onDuplicateClip,
  onDeleteClip,
  onMoveClip,
}: ResultsViewProps) {
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("edit");
  const [highlightQuery, setHighlightQuery] = useState("");
  const [highlightQueryFeedback, setHighlightQueryFeedback] = useState<{
    kind: "success" | "error";
    message: string;
  } | null>(null);
  const highlightQueryFeedbackRef = useRef<HTMLDivElement>(null);
  const rosterPlayers = replayPlayers(result);
  const totalDuration = clipEdits.reduce(
    (total, clip) =>
      total + Math.max(0, clip.endSeconds - clip.startSeconds),
    0,
  );
  const selectedRuleLabel = formatHighlightRuleSelection(highlightRuleIds);
  const selectedHighlightSummary = highlightHero
    ? heroHighlightSummary(
        result,
        highlightHero,
        highlightRuleIds,
        clipEdits.length,
      )
    : null;
  const anchorCandidates = highlightHero
    ? heroHighlightCandidates(result, highlightHero, highlightRuleIds)
    : result.highlights.candidates.filter(
        (candidate) => candidate.kind !== "hero_kill_sequence",
      );

  useEffect(() => {
    setHighlightQuery("");
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

  function applyHighlightQuery(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const parsed = parseHighlightRuleQuery(highlightQuery);
    if (parsed.status === "empty") {
      setHighlightQueryFeedback({
        kind: "error",
        message: "请输入想保留的内容。",
      });
      return;
    }
    if (parsed.status === "unknown") {
      setHighlightQueryFeedback({
        kind: "error",
        message: `暂不认识“${parsed.unknownText}”，本次没有更改筛选。`,
      });
      return;
    }
    if (parsed.status === "none") {
      setHighlightQueryFeedback({
        kind: "error",
        message: "没有选出可用类型，请至少保留击杀或砍树。",
      });
      return;
    }
    onUpdateHighlightRules(parsed.ruleIds);
    setHighlightQueryFeedback({
      kind: "success",
      message: highlightHero
        ? `已按“${formatHighlightRuleSelection(
            parsed.ruleIds,
          )}”更新${heroLabel(highlightHero)}高光。`
        : `已识别“${formatHighlightRuleSelection(
            parsed.ruleIds,
          )}”，再选择高光主角。`,
    });
  }

  return (
    <div className="results-workspace precision-workspace">
      <section className="candidate-panel">
        <div className="result-summary">
          <SummaryStat
            icon={<ListChecks />}
            label="剪辑片段"
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

        <div className="candidate-table-header">
          <div>
            <span className="eyebrow">
              {highlightHero
                ? `${heroLabel(highlightHero)} · ${selectedRuleLabel} · 按时间排列`
                : "按导出顺序排列"}
            </span>
            <h2>片段清单</h2>
          </div>
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
                  isSelected ? "selected" : ""
                }`}
                key={clip.clipId}
                role="button"
                tabIndex={0}
                aria-label={`片段 ${index + 1}，${
                  candidate ? candidateTitle(candidate) : "自定义片段"
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
                    {candidate
                      ? candidateTitle(candidate)
                      : `自定义片段 ${index + 1}`}
                  </strong>
                  <small>
                    {formatTimecode(clip.startSeconds)} -{" "}
                    {formatTimecode(clip.endSeconds)} ·{" "}
                    {highlightHero ? "主角：" : "视角："}
                    {heroLabel(clip.viewHero)}
                  </small>
                </span>
                <span className="candidate-kind">
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
                    title="删除片段"
                    aria-label={`删除片段 ${index + 1}`}
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
              {highlightHero ? <SearchX /> : <Clapperboard />}
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
        <div className="inspector-tabs">
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
            镜头
          </button>
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
                  : "自定义片段"}
              </h2>
              <div className="template-meta">
                <span>{formatDuration(selectedClip.endSeconds - selectedClip.startSeconds)}</span>
                <span>{formatTimecode(selectedClip.startSeconds)}</span>
                <span>{formatTimecode(selectedClip.endSeconds)}</span>
              </div>
            </div>

            {selectedCandidate?.interaction?.pattern_id ===
              "hoodwink_ground_acorn_quelling_blade" && (
              <div className="interaction-evidence">
                <div className="interaction-evidence-heading">
                  <span className="interaction-evidence-icon">
                    <Sparkles />
                  </span>
                  <span>
                    <strong>已验证砍树</strong>
                    <small>临时树实体已删除</small>
                  </span>
                </div>
                <p>
                  森海飞霞对地种树后{" "}
                  {selectedCandidate.interaction.response_delay_seconds.toFixed(
                    2,
                  )}{" "}
                  秒，{heroLabel(selectedCandidate.primary_hero)}
                  {selectedCandidate.interaction.verification
                    ? "用补刀斧砍掉了这棵临时树。"
                    : "使用了补刀斧。"}
                </p>
                <div className="interaction-evidence-tags">
                  <span>
                    本局第 {selectedCandidate.interaction.occurrence_index}/
                    {selectedCandidate.interaction.occurrence_count} 次
                  </span>
                  {selectedCandidate.interaction.verification
                    ?.first_fifteen_occurrence_index && (
                    <span>
                      前 15 分钟第{" "}
                      {
                        selectedCandidate.interaction.verification
                          .first_fifteen_occurrence_index
                      }
                      /
                      {
                        selectedCandidate.interaction.verification
                          .first_fifteen_occurrence_count
                      }{" "}
                      次
                    </span>
                  )}
                  {(selectedCandidate.interaction.verification
                    ?.source_to_responder_salute_count ?? 0) > 0 && (
                    <span>
                      森海飞霞打赏{" "}
                      {
                        selectedCandidate.interaction.verification
                          ?.source_to_responder_salute_count
                      }{" "}
                      次
                    </span>
                  )}
                  {selectedCandidate.interaction.related_action ===
                    "hoodwink_bushwhack" && <span>附近有捆人施法</span>}
                </div>
              </div>
            )}

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

            <PrecisionTimeEditor
              clip={selectedClip}
              replayDuration={result.replay.playback_time_seconds}
              peakSeconds={selectedCandidate?.peak_seconds ?? null}
              onUpdate={(patch) => onUpdateClip(selectedClip.clipId, patch)}
            />

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
        {inspectorTab === "camera" && (
          <>
            <div className="inspector-section settings-heading">
              <span className="eyebrow">英雄高光筛选</span>
              <h2>选择高光主角</h2>
              <p>
                选择后，片段清单会重建为该英雄的击杀连续段与已验证技术互动。
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
                    title={`${teamLabel(player.game_team)} · 玩家位 ${
                      player.slot + 1
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
                      {rule.id === HERO_KILL_RULE_ID ? <Swords /> : <Trees />}
                      <span>
                        <strong>{rule.label}</strong>
                        <small>{count === null ? "待选主角" : `${count} 段`}</small>
                      </span>
                      <Check />
                    </button>
                  );
                })}
              </div>
              <form
                className="highlight-query-form"
                onSubmit={applyHighlightQuery}
              >
                <label htmlFor={`highlight-query-${result.job_id}`}>
                  内容描述
                </label>
                <div>
                  <input
                    id={`highlight-query-${result.job_id}`}
                    value={highlightQuery}
                    onChange={(event) => {
                      setHighlightQuery(event.target.value);
                      setHighlightQueryFeedback(null);
                    }}
                    placeholder="例如：只要击杀 / 砍树和击杀"
                  />
                  <button type="submit" title="应用内容筛选">
                    <Search />
                    <span>应用</span>
                  </button>
                </div>
              </form>
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
            {selectedClip && (
              <div className="inspector-section">
                <div className="section-row">
                  <span className="section-title">当前片段镜头</span>
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
                      <strong>英雄近景</strong>
                      <small>Hero Chase · 偶尔用于突出英雄动作</small>
                    </span>
                    <Check />
                  </button>
                </div>
              </div>
            )}
            <div className="inspector-section setting-switches">
              <SettingSwitch
                label="干净画面"
                note="隐藏 HUD 与鼠标指针"
                checked={renderSettings.cleanHud}
                onChange={(cleanHud) => onUpdateRenderSettings({ cleanHud })}
              />
            </div>
          </>
        )}
        {!selectedClip && inspectorTab === "edit" && (
          <div className="inspector-empty">
            <Clapperboard />
            <strong>选择或新增一个片段</strong>
            <span>时间与镜头设置会显示在这里。</span>
          </div>
        )}
      </aside>
    </div>
  );
}

function PrecisionTimeEditor({
  clip,
  replayDuration,
  peakSeconds,
  onUpdate,
}: {
  clip: ClipEditState;
  replayDuration: number;
  peakSeconds: number | null;
  onUpdate: (patch: Partial<ClipEditState>) => void;
}) {
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
  result,
  clipEdits,
  capabilities,
  renderSettings,
  saving,
  rendering,
  renderProgress,
  renderResult,
  renderError,
  feedback,
  onClose,
  onStart,
  onCancel,
  onOpenPath,
}: {
  result: AnalysisSummary;
  clipEdits: ClipEdits;
  capabilities: Capabilities;
  renderSettings: RenderSettings;
  saving: boolean;
  rendering: boolean;
  renderProgress: RenderProgress | null;
  renderResult: RenderResult | null;
  renderError: string;
  feedback: string;
  onClose: () => void;
  onStart: () => void;
  onCancel: () => void;
  onOpenPath: (path: string) => void;
}) {
  const clips = planClips(clipEdits);
  const totalDuration = clips.reduce(
    (total, clip) =>
      total +
      Math.max(0, clip.sourceEndSeconds - clip.sourceStartSeconds),
    0,
  );
  const hasInvalidClip = clips.some(
    (clip) =>
      !Number.isFinite(clip.sourceStartSeconds) ||
      !Number.isFinite(clip.sourceEndSeconds) ||
      clip.sourceStartSeconds < 0 ||
      clip.sourceEndSeconds <= clip.sourceStartSeconds ||
      clip.sourceEndSeconds > result.replay.playback_time_seconds ||
      clip.sourceEndSeconds - clip.sourceStartSeconds < 1 ||
      clip.sourceEndSeconds - clip.sourceStartSeconds > 90,
  );
  const active = saving || rendering;
  const previewSource =
    renderResult && isTauriRuntime
      ? convertFileSrc(renderResult.outputPath)
      : "";

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
        className={`movie-dialog ${renderResult ? "with-preview" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="movie-dialog-title"
      >
        <header className="movie-dialog-header">
          <span className="movie-dialog-icon">
            <Film />
          </span>
          <span>
            <small>精确剪辑方案</small>
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

        <div
          className={`render-readiness ${
            capabilities.renderReady ? "ready" : "waiting"
          }`}
        >
          {capabilities.renderReady ? <CheckCircle2 /> : <CircleAlert />}
          <span>
            <strong>
              {capabilities.renderReady
                ? "本地成片环境已就绪"
                : "当前机器缺少成片环境"}
            </strong>
            <small>
              {capabilities.renderReady
                ? "开始后会自动启动离线 Dota 2，完成或取消后自动关闭。"
                : capabilities.renderReason}
            </small>
          </span>
        </div>

        <div className="movie-plan-summary">
          <div>
            <ListChecks />
            <span>
              <small>入选片段</small>
              <strong>{clips.length}</strong>
            </span>
          </div>
          <div>
            <Clock3 />
            <span>
              <small>预计时长</small>
              <strong>{formatDuration(totalDuration)}</strong>
            </span>
          </div>
          <div>
            <Camera />
            <span>
              <small>镜头方案</small>
              <strong>{new Set(clips.map((clip) => clip.cameraMode)).size} 种</strong>
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
                  {renderResult.segmentCount} 段
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
              <span>请不要操作 Dota 2；任务结束后客户端会自动关闭。</span>
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
                  <span className="movie-plan-index">{index + 1}</span>
                  <span className="movie-plan-copy">
                    <strong>
                      {candidate
                        ? candidateTitle(candidate)
                        : clip.candidateId}
                    </strong>
                    <small>
                      {formatTimecode(clip.sourceStartSeconds)} -{" "}
                      {formatTimecode(clip.sourceEndSeconds)} ·{" "}
                      {cameraModeShortLabel(clip.cameraMode)}
                    </small>
                  </span>
                  <span className="movie-plan-hero">
                    <UserRound />
                    {heroLabel(clip.viewHero)}
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
            <span>片段必须位于录像内，且单段时长为 1 到 90 秒。</span>
          </div>
        )}
        {renderError && (
          <div className="dialog-feedback error">
            <CircleAlert />
            <span>{renderError}</span>
          </div>
        )}
        {feedback && !renderError && (
          <div className="dialog-feedback">
            <CheckCircle2 />
            <span>{feedback}</span>
          </div>
        )}

        <footer className="movie-dialog-footer">
          {renderResult ? (
            <>
              <button className="secondary-button" onClick={onClose}>
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
              <button
                className="secondary-button"
                onClick={() => onOpenPath(renderResult.outputPath)}
              >
                <Play />
                <span>外部播放</span>
              </button>
              <button
                className="primary-button"
                disabled={
                  saving ||
                  !capabilities.renderReady ||
                  clips.length === 0 ||
                  hasInvalidClip
                }
                onClick={onStart}
              >
                <RefreshCw />
                <span>按当前方案重新生成</span>
              </button>
            </>
          ) : rendering ? (
            <button className="danger-button" onClick={onCancel}>
              <Pause />
              <span>取消并关闭 Dota 2</span>
            </button>
          ) : (
            <>
              <button className="secondary-button" onClick={onClose}>
                关闭
              </button>
              <button
                className="primary-button"
                disabled={
                  saving ||
                  !capabilities.renderReady ||
                  clips.length === 0 ||
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
                <span>{saving ? "正在保存方案" : "开始导出视频"}</span>
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
  jobs,
  onOpen,
  onImport,
}: {
  jobs: RecentJob[];
  onOpen: (job: RecentJob) => void;
  onImport: () => void;
}) {
  return (
    <section className="library-view">
      <div className="library-heading">
        <div>
          <span className="eyebrow">本地任务</span>
          <h2>录像库</h2>
        </div>
        <button className="primary-button" onClick={onImport}>
          <FileUp />
          <span>导入录像</span>
        </button>
      </div>
      <div className="library-table">
        <div className="library-table-head">
          <span>录像</span>
          <span>大小</span>
          <span>候选</span>
          <span>规划时长</span>
          <span />
        </div>
        {jobs.map((job) => (
          <button
            className="library-row"
            key={job.jobId}
            onClick={() => onOpen(job)}
          >
            <span className="library-file">
              <FileVideo2 />
              <span>
                <strong>{job.sourceName}</strong>
                <small>{job.jobId}</small>
              </span>
            </span>
            <span>{formatBytes(job.byteLength)}</span>
            <span>{job.candidateCount}</span>
            <span>{formatDuration(job.durationSeconds)}</span>
            <ChevronRight />
          </button>
        ))}
        {jobs.length === 0 && (
          <div className="library-empty">
            <Library />
            <span>还没有本地分析任务</span>
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

function roundFrame(value: number) {
  return Math.round(value * 30) / 30;
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

function createClipEdits(
  result: AnalysisSummary,
  savedPlan?: LoadedEditPlan,
): ClipEdits {
  if (savedPlan?.clips.length) {
    return savedPlan.clips.map((clip, index) => ({
      clipId: clip.clipId || `clip-saved-${String(index + 1).padStart(2, "0")}`,
      candidateId: clip.candidateId,
      viewHero:
        clip.viewHero ??
        result.replay.players[0]?.hero_name ??
        "",
      cameraMode: normalizeUserCameraMode(clip.cameraMode),
      startSeconds: roundFrame(clip.sourceStartSeconds),
      endSeconds: roundFrame(clip.sourceEndSeconds),
    }));
  }

  const segments = new Map(
    result.director.segments.map((segment) => [
      segment.candidate_id,
      segment,
    ]),
  );
  const sourceCandidates = result.director.segments.length
    ? result.director.segments
        .map((segment) =>
          result.highlights.candidates.find(
            (candidate) => candidate.id === segment.candidate_id,
          ),
        )
        .filter((candidate): candidate is HighlightCandidate => Boolean(candidate))
    : result.highlights.candidates.slice(0, 3);

  return sourceCandidates.map((candidate, index) => {
    const segment = segments.get(candidate.id);
    const start = segment?.source_start_seconds ?? candidate.start_seconds;
    const unclampedEnd = segment?.source_end_seconds ?? candidate.end_seconds;
    const end = Math.min(
      result.replay.playback_time_seconds,
      start + Math.min(90, Math.max(1, unclampedEnd - start)),
    );
    return {
      clipId: `clip-recommended-${String(index + 1).padStart(2, "0")}`,
      candidateId: candidate.id,
      viewHero:
        segment?.primary_hero ??
        candidate.primary_hero ??
        candidate.participants[0] ??
        result.replay.players[0]?.hero_name ??
        "",
      cameraMode: "player_perspective",
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
  return heroHighlightCandidates(result, hero, ruleIds).map(
    (candidate, index) => ({
      clipId: `clip-hero-${String(index + 1).padStart(2, "0")}`,
      candidateId: candidate.id,
      viewHero: hero,
      cameraMode: "player_perspective",
      startSeconds: roundFrame(candidate.start_seconds),
      endSeconds: roundFrame(candidate.end_seconds),
    }),
  );
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
  if (
    normalizedRules.length === 1 &&
    normalizedRules[0] === VERIFIED_TREE_CUT_RULE_ID
  ) {
    return { label: "砍树高光", value: `${clipCount} 段` };
  }
  return { label: "高光片段", value: `${clipCount} 段` };
}

function planClips(clipEdits: ClipEdits): EditPlanClip[] {
  return clipEdits.map((clip) => ({
    clipId: clip.clipId,
    candidateId: clip.candidateId,
    viewHero: clip.viewHero || null,
    cameraMode: clip.cameraMode,
    sourceStartSeconds: roundFrame(clip.startSeconds),
    sourceEndSeconds: roundFrame(clip.endSeconds),
  }));
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

function normalizeUserCameraMode(
  mode: ClipCameraMode | null | undefined,
): ClipCameraMode {
  return mode === "hero_chase" ? "hero_chase" : "player_perspective";
}

function cameraModeShortLabel(mode: ClipCameraMode) {
  const labels: Record<ClipCameraMode, string> = {
    directed: "玩家视角",
    free_camera: "玩家视角",
    hero_chase: "英雄近景",
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
  if (
    candidate.kind === "mechanical_counterplay" &&
    candidate.interaction?.pattern_id ===
      "hoodwink_ground_acorn_quelling_blade"
  ) {
    const { occurrenceIndex, occurrenceCount } = {
      occurrenceIndex: candidate.interaction.occurrence_index,
      occurrenceCount: candidate.interaction.occurrence_count,
    };
    return occurrenceCount > 1
      ? `补刀斧砍树 · ${occurrenceIndex}/${occurrenceCount}`
      : "补刀斧砍树";
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
