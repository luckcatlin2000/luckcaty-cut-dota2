import { lazy, Suspense, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const HighlightApp = lazy(() => import("./App"));
const WtfWorkbench = lazy(() => import("./WtfWorkbench"));
type DesktopModule = "highlights" | "wtf";
const native = "__TAURI_INTERNALS__" in window;

export default function DesktopEntry() {
  const [module, setModule] = useState<DesktopModule | null>(() => native ? null :
    import.meta.env.DEV && new URLSearchParams(window.location.search).get("module") === "wtf"
      ? "wtf" : "highlights");
  const [error, setError] = useState("");
  useEffect(() => {
    if (!native) return;
    let active = true;
    void invoke<DesktopModule>("get_desktop_module").then(value => {
      if (value !== "highlights" && value !== "wtf") throw new Error("无法识别当前软件模块。");
      if (active) setModule(value);
    }).catch(reason => { if (active) setError(String(reason)); });
    return () => { active = false; };
  }, []);
  if (error) return <main role="alert">模块启动失败：{error}</main>;
  if (!module) return <main role="status">正在打开本地工作台…</main>;
  return <Suspense fallback={<main role="status">正在载入工作台…</main>}>
    {module === "wtf" ? <WtfWorkbench /> : <HighlightApp />}
  </Suspense>;
}
