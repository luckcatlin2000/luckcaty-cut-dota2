import { useEffect, useMemo, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowDown, ArrowRight, ArrowUp, CheckCircle2, ChevronDown, CircleAlert, FileDown, FileUp, Film, Layers3, LoaderCircle, Maximize2, Minus, Save, Search, ShieldCheck, Sparkles, Trash2, X } from "lucide-react";
import type { AnalysisProgress, AnalysisSummary, HighlightStory } from "./types";
import { heroLabel } from "./heroes";
import { adoptWtfStory, createWtfDevelopmentFixture, editWtfSceneTime, filterWtfStories, moveWtfScene,
  validateWtfClips, validateWtfDraft, WtfDraftSequence, wtfDefaultDuration, wtfSceneGroups, WTF_CATEGORY_LABELS } from "./wtfWorkbenchModel";
import type { WtfCategoryFilter, WtfClipInput, WtfDraft, WtfHandoffReceipt, WtfPlanExport } from "./wtfWorkbenchModel";
import "./wtfWorkbench.css";

const storageKey = "maomao-wtf-workbench-v1";
const stageLabels: Record<string, string> = { ingest: "校验录像", parse: "读取完整事件", detect: "筛选候选", direct: "整理故事", story: "整理故事" };
const beatLabels: Record<string, string> = { setup: "铺垫", development: "发展", turn: "转折", payoff: "结果", reaction: "反应" };
const confidenceLabels = { high: "高", medium: "中", low: "低" };
const errorText = (reason: unknown) => reason instanceof Error ? reason.message : String(reason);
const leafName = (path: string) => path.split(/[\\/]/).pop() || path;
const secondsText = (seconds: number) => {
  const value = Math.max(0, seconds);
  return `${String(Math.floor(value / 60)).padStart(2, "0")}:${(value % 60).toFixed(3).padStart(6, "0")}`;
};

export function WtfWorkbench({ version = "2.5.1" }: { version?: string } = {}) {
  const native = "__TAURI_INTERNALS__" in window;
  const fixture = useMemo(() => import.meta.env.DEV && new URLSearchParams(window.location.search).get("fixture") === "wtf"
    ? createWtfDevelopmentFixture() : null, []);
  const [summary, setSummary] = useState<AnalysisSummary | null>(fixture);
  const initialClips = useMemo(() => fixture ? adoptWtfStory(fixture.stories.stories[0]!, [], fixture.replay.playback_time_seconds) : [], [fixture]);
  const [clips, setClips] = useState<WtfClipInput[]>(initialClips);
  const [category, setCategory] = useState<WtfCategoryFilter>("all");
  const [heroFilter, setHeroFilter] = useState("");
  const [progress, setProgress] = useState<AnalysisProgress[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [closing, setClosing] = useState(false);
  const [draftReady, setDraftReady] = useState(!!fixture);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [lastReplay, setLastReplay] = useState(() => { try { return localStorage.getItem(`${storageKey}.lastReplay`) ?? ""; } catch { return ""; } });
  const [, setSavedRevision] = useState(0);
  const [handoff, setHandoff] = useState<WtfHandoffReceipt | null>(null);
  const summaryRef = useRef(summary), clipsRef = useRef(clips), readyRef = useRef(draftReady);
  const closingRef = useRef(false);
  const pendingAction = useRef<Promise<void> | null>(null);
  const queue = useRef(new WtfDraftSequence());
  const saveCount = useRef(0);
  const initialized = useRef(false);
  if (!initialized.current) {
    if (fixture) queue.current.markLoaded(fixture.job_id, initialClips);
    initialized.current = true;
  }
  const dirty = !!summary && draftReady && queue.current.isDirty(summary.job_id, clips);
  const locked = !!busy || closing;
  const groups = useMemo(() => wtfSceneGroups(clips), [clips]);
  const stories = summary?.stories.stories ?? [];
  const filtered = useMemo(() => filterWtfStories(stories, category, heroFilter), [stories, category, heroFilter]);
  const heroes = summary?.replay.players.filter(player => player.hero_name).map(player => player.hero_name) ?? [];
  const storyById = useMemo(() => new Map(stories.map(story => [story.id, story])), [stories]);

  function updateClips(value: WtfClipInput[]) {
    clipsRef.current = value;
    setClips(value);
    setHandoff(null);
    setNotice("");
  }

  async function saveCurrent(showNotice = false) {
    const current = summaryRef.current;
    if (!current || !readyRef.current) return;
    validateWtfClips(clipsRef.current, current.replay.playback_time_seconds);
    saveCount.current++;
    setSaving(true);
    try {
      await queue.current.save(current.job_id, clipsRef.current, async request => {
        if (native) return invoke<WtfDraft>("save_wtf_draft", { ...request });
        if (!fixture) throw new Error("请在桌面版 WTF 剪辑中保存草稿。");
        localStorage.setItem(`${storageKey}.fixtureDraft`, JSON.stringify(request.clips));
      });
      setSavedRevision(value => value + 1);
      if (showNotice) setNotice("草稿已保存，下次选择同一份录像可继续编辑。");
    } finally {
      saveCount.current--;
      setSaving(saveCount.current > 0);
    }
  }

  async function runAction(label: string, action: () => Promise<void>) {
    if (pendingAction.current || closingRef.current) return;
    setBusy(label); setError(""); setNotice("");
    const task = action();
    pendingAction.current = task;
    try { await task; } catch (reason: unknown) { setError(errorText(reason)); }
    finally { if (pendingAction.current === task) pendingAction.current = null; setBusy(null); }
  }

  async function analyze(path: string) {
    await runAction("正在分析录像", async () => {
      await saveCurrent();
      setProgress([]); setHandoff(null);
      if (!native) throw new Error("浏览器仅展示合成演示，真实 DEM 请在桌面版中选择。");
      const channel = new Channel<AnalysisProgress>();
      channel.onmessage = value => setProgress(current => [...current.filter(item => item.stage !== value.stage), value]);
      const next = await invoke<AnalysisSummary>("analyze_wtf_dem", { path, onProgress: channel });
      // Loading a damaged draft must never turn into an autosavable empty replacement.
      readyRef.current = false; setDraftReady(false);
      summaryRef.current = next; setSummary(next); updateClips([]);
      const draft = await invoke<WtfDraft | null>("get_wtf_draft", { jobId: next.job_id });
      const restored = draft ? validateWtfDraft(next, draft) : [];
      queue.current.markLoaded(next.job_id, restored);
      updateClips(restored);
      readyRef.current = true; setDraftReady(true);
      setCategory("all"); setHeroFilter("");
      try { localStorage.setItem(`${storageKey}.lastReplay`, path); } catch { /* Drafts are saved by the backend. */ }
      setLastReplay(path);
      setNotice(draft ? `已恢复 ${restored.length} 个机位的独立草稿。` : "分析完成，请核对证据后采用故事。");
    });
  }

  async function chooseReplay() {
    if (locked) return;
    if (!native) { setNotice("当前为合成界面演示。真实录像请在桌面版 WTF 剪辑中打开。"); return; }
    await runAction("正在选择录像", async () => {
      const path = await open({ multiple: false, directory: false, title: "选择 WTF 剪辑录像", filters: [{ name: "Dota 2 录像", extensions: ["dem"] }] });
      if (typeof path === "string") {
        // Finish this dialog operation before the separately tracked analysis starts.
        pendingSelection.current = path;
      }
    });
    const path = pendingSelection.current;
    pendingSelection.current = null;
    if (path && !closingRef.current) await analyze(path);
  }
  const pendingSelection = useRef<string | null>(null);

  async function requestClose() {
    if (!native || closingRef.current) return;
    closingRef.current = true; setClosing(true); setError("");
    try {
      if (pendingAction.current) await pendingAction.current;
      await queue.current.waitForIdle().catch(() => undefined);
      await saveCurrent();
      await invoke("close_app_after_cleanup");
    } catch (reason: unknown) {
      setError(`尚未关闭：${errorText(reason)}。请处理后重试，当前编辑仍保留在窗口中。`);
      closingRef.current = false; setClosing(false);
    }
  }
  const requestCloseRef = useRef(requestClose);
  requestCloseRef.current = requestClose;

  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const stops: Array<() => void> = [];
    const register = (promise: Promise<() => void>) => {
      void promise.then(stop => disposed ? stop() : stops.push(stop)).catch(reason => setError(errorText(reason)));
    };
    register(getCurrentWindow().onCloseRequested(event => { event.preventDefault(); void requestCloseRef.current(); }));
    register(listen("app-close-requested", () => void requestCloseRef.current()));
    return () => { disposed = true; stops.forEach(stop => stop()); };
  }, [native]);

  useEffect(() => {
    if (!summary || !draftReady || !queue.current.isDirty(summary.job_id, clips)) return;
    const timer = window.setTimeout(() => {
      if (!pendingAction.current && !closingRef.current) void saveCurrent().catch(reason => setError(`自动保存未完成：${errorText(reason)}`));
    }, 800);
    return () => window.clearTimeout(timer);
  }, [clips, summary?.job_id, draftReady]);

  async function exportPlan() {
    await runAction("正在导出方案", async () => {
      await saveCurrent();
      if (!summaryRef.current || !clipsRef.current.length) return;
      if (!native) { setNotice("合成演示不会调用本机保存对话框。桌面版可导出时间表 JSON。"); return; }
      const result = await invoke<WtfPlanExport | null>("export_wtf_plan", { jobId: summaryRef.current.job_id, clips: clipsRef.current });
      if (result) setNotice(`方案已导出：${result.planPath}（${result.clipCount} 个机位）`);
    });
  }

  async function handoffPlan() {
    await runAction("正在交给高光剪辑", async () => {
      await saveCurrent();
      if (!summaryRef.current || !clipsRef.current.length) return;
      if (!native) { setNotice("合成演示不会启动其他软件。桌面版交接后仍需手动开始导出。"); return; }
      const receipt = await invoke<WtfHandoffReceipt>("handoff_wtf_plan", { jobId: summaryRef.current.job_id, clips: clipsRef.current });
      setHandoff(receipt); setNotice(receipt.message);
    });
  }

  function adopt(story: HighlightStory) {
    if (locked || !draftReady || !summary) return;
    try { updateClips(adoptWtfStory(story, clipsRef.current, summary.replay.playback_time_seconds)); setError(""); }
    catch (reason: unknown) { setError(errorText(reason)); }
  }

  return <div className="wtf-app">
    <header className="wtf-titlebar" data-tauri-drag-region>
      <div className="wtf-brand"><span><Sparkles /></span><strong>WTF 剪辑</strong><small>{version}</small></div>
      <span className="wtf-titlebar-note"><ShieldCheck />独立工作台 · 只读录像</span>
      <div className="wtf-window-actions">
        <button aria-label="最小化" disabled={!native} onClick={() => void getCurrentWindow().minimize()}><Minus /></button>
        <button aria-label="最大化或还原" disabled={!native} onClick={() => void getCurrentWindow().toggleMaximize()}><Maximize2 /></button>
        <button aria-label="保存并关闭 WTF 剪辑" disabled={!native || closing} onClick={() => void requestClose()}><X /></button>
      </div>
    </header>

    <section className="wtf-heading">
      <div><span className="wtf-eyebrow">从事件证据到剪辑方案</span><h1>好玩的片段，先把来龙去脉看清楚。</h1>
        <p>筛选趣味、精彩与失误故事，核对证据、安排主备机位，再交给高光剪辑录制。</p></div>
      <button className="wtf-button wtf-button-primary" disabled={locked} onClick={() => void chooseReplay()}><FileUp />选择 DEM 录像</button>
    </section>
    {fixture && <div className="wtf-fixture-note"><CircleAlert />合成演示数据，仅供检查界面与编辑流程，不代表真实录像识别结果。</div>}
    <section className="wtf-source-strip">
      <Film /><div><strong>{summary ? leafName(summary.source.path) : "尚未选择录像"}</strong><small>{summary ? `${summary.event_count} 个事件 · ${stories.length} 个故事候选 · ${secondsText(summary.replay.playback_time_seconds)} 总时长` : "原始 .dem 保持只读，不启动游戏。"}</small></div>
      <span className={`wtf-save-state ${dirty ? "dirty" : ""}`}>{closing ? "等待保存并关闭…" : saving ? "正在保存草稿…" : dirty ? "有未保存的修改" : summary && draftReady ? "草稿已保存" : "等待录像"}</span>
    </section>
    {progress.length > 0 && <div className="wtf-analysis-progress" aria-live="polite">{progress.map(item => <span key={item.stage} className={item.status}>
      {item.status === "running" ? <LoaderCircle className="wtf-spin" /> : item.status === "failed" ? <CircleAlert /> : <CheckCircle2 />}<b>{stageLabels[item.stage] ?? item.stage}</b><small>{item.message}</small></span>)}</div>}
    {error && <div className="wtf-feedback error" role="alert"><CircleAlert /><span>{error}</span></div>}
    {notice && <div className="wtf-feedback" role="status"><CheckCircle2 /><span>{notice}</span></div>}

    <main className="wtf-columns">
      <section className="wtf-panel wtf-story-panel" aria-label="故事候选">
        <div className="wtf-panel-heading"><div><h2>故事候选 <span>{filtered.length}</span></h2><p>按证据挑选；置信度与人工核对提示始终保留。</p></div><Search /></div>
        <div className="wtf-filters"><div className="wtf-category-tabs" aria-label="故事类别">{(["all", "comedy", "skill", "mistake"] as const).map(value =>
          <button key={value} aria-pressed={category === value} className={category === value ? "selected" : ""} onClick={() => setCategory(value)}>{value === "all" ? "全部" : WTF_CATEGORY_LABELS[value]}</button>)}</div>
          <select aria-label="按参与英雄筛选" value={heroFilter} onChange={event => setHeroFilter(event.target.value)}><option value="">全部英雄</option>{Array.from(new Set(heroes)).map(hero => <option key={hero} value={hero}>{heroLabel(hero)}</option>)}</select>
        </div>
        <div className="wtf-story-list">
          {filtered.map(story => <StoryCard key={story.id} story={story} locked={locked || !draftReady} clips={clips} onAdopt={() => adopt(story)} />)}
          {!summary && <div className="wtf-empty"><Sparkles /><h3>先选一份录像</h3><p>完整分析后，这里会列出有事件证据的故事。</p>{lastReplay && native && <button className="wtf-button" disabled={locked} onClick={() => void analyze(lastReplay)}>继续上次录像：{leafName(lastReplay)}</button>}</div>}
          {summary && filtered.length === 0 && <div className="wtf-empty"><Search /><h3>{stories.length ? "没有符合当前筛选的故事" : "本局未找到受支持的故事"}</h3><p>{stories.length ? "可以切换类别或英雄，已有录制清单会保留。" : "目前只展示规则识别且保留证据的片段；没有候选时不会编造剧情或反转。普通击杀可在高光时刻中继续处理。"}</p></div>}
        </div>
      </section>

      <section className="wtf-panel wtf-plan-panel" aria-label="独立录制清单">
        <div className="wtf-panel-heading"><div><h2>录制清单 <span>{groups.length} 场</span></h2><p>{clips.length} 个机位 · 默认成片 {wtfDefaultDuration(clips).toFixed(1)} 秒</p></div><Layers3 /></div>
        <div className="wtf-plan-explanation">同场机位共用时间窗。A 为主机位；其余只保存备用素材，不重复计入成片时长。</div>
        <div className="wtf-scene-list">
          {groups.map((group, index) => { const first = group[0]!; const story = storyById.get(first.storyId); return <article className="wtf-scene" key={first.takeGroupId}>
            <div className="wtf-scene-heading"><span className="wtf-scene-number">S{String(index + 1).padStart(3, "0")}</span><strong>{story?.title ?? first.storyId}</strong>
              <div><button aria-label={`上移场次 ${index + 1}`} disabled={locked || index === 0} onClick={() => updateClips(moveWtfScene(clipsRef.current, first.takeGroupId, -1))}><ArrowUp /></button>
                <button aria-label={`下移场次 ${index + 1}`} disabled={locked || index === groups.length - 1} onClick={() => updateClips(moveWtfScene(clipsRef.current, first.takeGroupId, 1))}><ArrowDown /></button>
                <button aria-label={`移除场次 ${index + 1}`} disabled={locked} onClick={() => updateClips(clipsRef.current.filter(clip => clip.takeGroupId !== first.takeGroupId))}><Trash2 /></button></div></div>
            <SceneTimeEditor index={index + 1} clip={first} disabled={locked || !draftReady} onApply={(start, end) => {
              updateClips(editWtfSceneTime(clipsRef.current, first.takeGroupId, start, end, summary!.replay.playback_time_seconds));
            }} />
            <div className="wtf-take-list">{group.map((clip, takeIndex) => <div className="wtf-take" key={clip.clipId}>
              <span className={clip.takeRole === "primary" ? "primary" : "alternate"}>{String.fromCharCode(65 + takeIndex)} · {clip.takeRole === "primary" ? "默认入片" : "备用素材"}</span>
              <select aria-label={`场次 ${index + 1} 机位 ${takeIndex + 1} 英雄`} disabled={locked || !draftReady} value={clip.hero}
                onChange={event => updateClips(clipsRef.current.map(item => item.clipId === clip.clipId ? { ...item, hero: event.target.value } : item))}>
                {Array.from(new Set([clip.hero, ...heroes])).map(hero => <option key={hero} value={hero}>{heroLabel(hero)}</option>)}</select>
              <select aria-label={`场次 ${index + 1} 机位 ${takeIndex + 1} 镜头`} disabled={locked || !draftReady} value={clip.cameraMode}
                onChange={event => updateClips(clipsRef.current.map(item => item.clipId === clip.clipId ? { ...item, cameraMode: event.target.value as WtfClipInput["cameraMode"] } : item))}>
                <option value="player_perspective">玩家视角</option><option value="hero_chase">英雄跟随</option></select>
            </div>)}</div>
          </article>; })}
          {groups.length === 0 && <div className="wtf-empty"><Layers3 /><h3>先采用一个故事</h3><p>故事里的场次与机位会进入这里，可以排序、修改时间和目标英雄。</p></div>}
        </div>
        <footer className="wtf-plan-actions">
          <button className="wtf-button" disabled={locked || !summary || !draftReady} onClick={() => void runAction("正在保存草稿", () => saveCurrent(true))}><Save />保存草稿</button>
          <button className="wtf-button" disabled={locked || !clips.length || !draftReady} onClick={() => void exportPlan()}><FileDown />导出方案 JSON</button>
          <button className="wtf-button wtf-button-primary" disabled={locked || !clips.length || !draftReady} onClick={() => void handoffPlan()}><ArrowRight />交给高光剪辑</button>
          <small>{busy ?? "交接只准备录制会话，不会自动开始导出视频。"}</small>
        </footer>
      </section>
    </main>
    {handoff && <div className="wtf-handoff-receipt"><CheckCircle2 /><div><strong>{handoff.launched ? "已打开高光剪辑会话" : "录制会话已准备"}</strong><p>{handoff.sessionDirectory}</p><small>请核对镜头与时间，再在高光剪辑中手动开始导出。</small></div></div>}
  </div>;
}

function StoryCard({ story, clips, locked, onAdopt }: { story: HighlightStory; clips: WtfClipInput[]; locked: boolean; onAdopt: () => void }) {
  const adopted = story.shots.length > 0 && story.shots.every(shot => clips.some(clip => clip.storyId === story.id && clip.shotId === shot.id));
  const scenes = new Set(story.shots.map(shot => shot.take_group_id)).size;
  return <article className="wtf-story-card">
    <div className="wtf-story-badges"><span className={`category-${story.category}`}>{WTF_CATEGORY_LABELS[story.category]}</span><span>置信度 {confidenceLabels[story.confidence.level]} · {Math.round(story.confidence.score * 100)}%</span>
      {story.review_required && <span className="review">需人工核对</span>}</div>
    <h3>{story.title}</h3><p className="wtf-story-meta">{heroLabel(story.primary_hero)} · {secondsText(story.source_start_seconds)}–{secondsText(story.source_end_seconds)}</p>
    <div className="wtf-story-stats"><span>{story.evidence.length} 条证据</span><span>{story.beats.length} 个节拍</span><span>{scenes} 场 / {story.shots.length} 机位</span></div>
    {story.reasons.length > 0 && <p className="wtf-story-reason">{story.reasons[0]}</p>}
    <details className="wtf-story-details"><summary>核对证据与镜头建议 <ChevronDown /></summary>
      <h4>识别依据</h4>{story.confidence.reasons.map((reason, index) => <p key={`confidence-${index}`}>{reason}</p>)}
      <ol>{story.evidence.map(evidence => <li key={evidence.id}><b>{secondsText(evidence.time_seconds)}</b> {evidence.detail}</li>)}</ol>
      <h4>故事节拍</h4>{story.beats.map(beat => <p key={beat.id}><b>{beatLabels[beat.kind] ?? beat.kind}</b> · {secondsText(beat.source_start_seconds)}–{secondsText(beat.source_end_seconds)} · {beat.summary}</p>)}
      <h4>场次与多机位</h4>{story.shots.map(shot => <p key={shot.id}><b>{shot.take_role === "primary" ? "主机位" : "备用"}</b> · {heroLabel(shot.target_hero ?? story.primary_hero)} · {shot.purpose}
        {shot.camera_mode === "directed" && <em>；交接使用受支持的备用玩家/跟随镜头。</em>}</p>)}
      <h4>建议切换窗</h4>{story.switch_windows.length ? story.switch_windows.map((window, index) => <p key={index}>{secondsText(window.source_start_seconds)}–{secondsText(window.source_end_seconds)} · {window.reason}</p>) : <p>未给出切换建议，不自动添加回看。</p>}
    </details>
    <button className={`wtf-button ${adopted ? "wtf-button-adopted" : ""}`} disabled={locked || adopted || story.shots.length === 0} onClick={onAdopt}>{adopted ? <CheckCircle2 /> : <Layers3 />}{adopted ? "已加入录制清单" : "采用故事与机位"}</button>
  </article>;
}

function SceneTimeEditor({ index, clip, disabled, onApply }: { index: number; clip: WtfClipInput; disabled: boolean; onApply: (start: number, end: number) => void }) {
  const [start, setStart] = useState(String(Number(clip.startSeconds.toFixed(3))));
  const [end, setEnd] = useState(String(Number(clip.endSeconds.toFixed(3))));
  const [error, setError] = useState("");
  useEffect(() => { setStart(String(Number(clip.startSeconds.toFixed(3)))); setEnd(String(Number(clip.endSeconds.toFixed(3)))); setError(""); }, [clip.startSeconds, clip.endSeconds]);
  const edited = Math.abs(Number(start) - clip.startSeconds) > 0.0006 || Math.abs(Number(end) - clip.endSeconds) > 0.0006;
  return <form className="wtf-time-editor" onSubmit={event => { event.preventDefault(); try {
    if (!start.trim() || !end.trim()) throw new Error("请输入入点与出点。");
    onApply(Number(start), Number(end)); setError("");
  } catch (reason: unknown) { setError(errorText(reason)); } }}>
    <label>入点（秒）<input aria-label={`场次 ${index} 入点秒数`} inputMode="decimal" value={start} disabled={disabled} onChange={event => setStart(event.target.value)} /></label>
    <label>出点（秒）<input aria-label={`场次 ${index} 出点秒数`} inputMode="decimal" value={end} disabled={disabled} onChange={event => setEnd(event.target.value)} /></label>
    <button className="wtf-button" type="submit" disabled={disabled || !edited}>应用时间</button>
    <small>{error || (edited ? "时间尚未应用；应用后同步全部机位。" : `${(clip.endSeconds - clip.startSeconds).toFixed(1)} 秒 · 同步全部机位 · 限 1–100 秒`)}</small>
  </form>;
}

export default WtfWorkbench;
