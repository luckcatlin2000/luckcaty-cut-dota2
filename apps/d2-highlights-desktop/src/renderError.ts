export interface RenderErrorPresentation {
  title: string;
  message: string;
  details: string | null;
}

export function presentRenderError(error: string): RenderErrorPresentation {
  if (error.startsWith("Dota 2 启动后未能建立离线控制连接")) {
    return {
      title: "未能连接游戏回放控制",
      message: "游戏已启动，但软件未能完成控制连接确认，本次导出已停止。请展开技术详情查看最近一次反馈。",
      details: error,
    };
  }
  if (error.includes("回放跳转未确认")) {
    const missingPosition = error.includes("未能读取录像当前播放位置") ||
      /最后实际 tick\s+None/.test(error);
    return {
      title: "未能确认录像播放位置",
      message: missingPosition
        ? "软件没有收到当前播放位置的有效反馈，为避免录错片段，本次导出已停止。请保留控制日志以便排查。"
        : "录像尚未稳定到达指定位置，为避免录错片段，本次导出已停止。请保留控制日志以便排查。",
      details: error,
    };
  }
  if (error.startsWith("Dota 2 回放控制失败")) {
    return {
      title: "游戏回放控制失败",
      message: "软件未能完成回放控制，本次导出已停止。请展开技术详情查看原因，并保留控制日志。",
      details: error,
    };
  }
  return { title: "本次导出未完成", message: error, details: null };
}
