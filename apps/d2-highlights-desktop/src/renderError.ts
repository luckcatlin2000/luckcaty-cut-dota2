export interface RenderErrorPresentation {
  code: string;
  title: string;
  message: string;
  steps: string[];
  details: string;
}

function shown(code:string,title:string,message:string,steps:string[],error:string):RenderErrorPresentation {
  return {code,title,message,steps,details:error};
}

// Stable codes are indexed in docs/ERROR_CATALOG.md. Classify the observed
// failure, never turn a possible trigger into an asserted root cause.
export function presentRenderError(error:string):RenderErrorPresentation {
  const value=error.replace(/^Error:\s*/,"");
  if(value.startsWith("Steam 未运行。")) {
    return shown("D2H-STEAM-001","请先打开 Steam","开始录制前需要 Steam 保持运行。请手动打开并登录 Steam，等待连接稳定后重新导出。",["手动打开并登录 Steam，再开始导出。"],error);
  }
  if(value.startsWith("Dota 2 已在运行。")) {
    return shown("D2H-GAME-001","游戏客户端正在运行","软件检测到已有 Dota 2 客户端，无法开始自己的离线录制。",["保存当前游戏进度，并自行正常关闭客户端后再导出。"],error);
  }
  if(value.startsWith("Dota 2 在离线导出期间意外退出")) {
    return shown("D2H-GAME-002","录制时游戏意外退出","录制所用游戏进程提前结束，当前片段尚未完成。退出的触发原因需要结合本局日志确认。",["保存本局日志和排查信息。","核对可恢复的片段后，再决定续录或重录。"],error);
  }
  if(value.includes("回放跳转未确认")) {
    const missing=value.includes("未能读取录像当前播放位置")||/最后实际 tick\s+None/.test(value);
    return shown(missing?"D2H-SEEK-001":"D2H-SEEK-002","未能确认录像播放位置",missing
      ?"软件没有收到当前播放位置的有效反馈，为避免录错片段，本次导出已停止。"
      :"录像尚未稳定到达指定位置，为避免录错片段，本次导出已停止。",
      ["保存控制日志，核对目标位置和最近收到的位置。","反馈异常的原因尚未确认，请复制排查信息供定位。"],error);
  }
  if(value.includes("VConsole")&&/10054|connection reset|远程主机强迫关闭/i.test(value)) {
    return shown("D2H-CTRL-054","游戏回放控制失败","软件与游戏的控制连接被断开，当前导出已停止。断开的触发原因需要日志确认。",["保存本局控制日志和排查信息。","核对已录片段后选择续录或重录。"],error);
  }
  if(value.startsWith("Dota 2 启动后未能建立离线控制连接")) {
    return shown("D2H-CTRL-001","未能连接游戏回放控制","游戏已启动，但软件未能完成控制连接确认，本次导出已停止。",["展开技术详情查看最近一次连接反馈。","保存控制日志，确认是等待超时、协议异常还是连接关闭。"],error);
  }
  if(value.includes("镜头位置反馈不可识别")) {
    return shown("D2H-CAMERA-001","无法确认镜头反馈","软件未能识别游戏返回的镜头信息，本次录制已停止。",["保留本局控制日志和诊断信息，核对当前客户端的镜头反馈。"],error);
  }
  if(value.startsWith("Dota 2 回放控制失败")) {
    return shown("D2H-CTRL-000","游戏回放控制失败","软件未能完成回放控制，本次导出已停止。具体失败操作见技术详情。",["复制排查信息并保留控制日志，按报错编号查阅排查文档。"],error);
  }
  if(/os error 3\b/.test(value)) {
    return shown("D2H-FS-003","文件路径无法访问","软件读写文件时无法找到所需路径，导出已停止。具体失败路径和操作需结合技术详情与本局日志确认。",["展开技术详情，核对失败操作和路径；旧版原文可能没有给出具体路径。","复制排查信息，并通过“打开文件夹”保留本局日志。"],error);
  }
  if(/os error 2\b/.test(value)||value.startsWith("录像文件不存在")) {
    return shown("D2H-FS-002","所需文件不存在","软件找不到所需文件。请根据技术详情核对录像、工具或素材的实际位置。",["检查报错中指出的文件是否存在。","保留原素材和日志，复制排查信息。"],error);
  }
  if(/os error (5|13)\b/.test(value)) {
    return shown("D2H-FS-005","文件访问被拒绝","系统拒绝了这次文件访问。访问权限或文件占用情况需要进一步核对。",["核对所选目录是否允许写入，以及目标文件是否正在被其他程序使用。","保留原文件并复制排查信息。"],error);
  }
  if(/os error (112|28)\b/.test(value)) {
    return shown("D2H-FS-112","磁盘空间不足","当前写入磁盘没有足够空间，导出已停止。",["检查录制缓存盘和输出盘的剩余空间。","保留已有素材，准备足够空间后再导出。"],error);
  }
  if(value.startsWith("文件操作失败")) {
    return shown("D2H-FS-000","文件保存或读取失败","软件未能完成文件操作。失败的操作、路径和系统反馈见技术详情。",["复制排查信息，核对报错路径和操作。"],error);
  }
  if(value.startsWith("找不到或无法运行")) {
    return shown("D2H-TOOL-001","所需工具无法运行","录制或处理视频所需的工具不存在或无法启动。",["在全局设置中核对 Dota 2、FFmpeg 和 FFprobe 的路径。"],error);
  }
  if(value.includes("有效帧序列")||value.includes("没有生成 WAV 音轨")) {
    return shown("D2H-CAPTURE-001","游戏未生成完整录制素材","游戏原生导出的画面或音轨不完整，当前片段尚不可用。",["保留本局日志和诊断，核对原生录制反馈与已生成文件。"],error);
  }
  if(value.startsWith("媒体处理失败")) {
    return shown("D2H-MEDIA-001","视频处理未完成","视频编码、合成或质量检查失败。具体处理阶段和工具反馈见技术详情。",["保留原始切片、工具日志和排查信息。"],error);
  }
  if(value.startsWith("剪辑方案无效")||value.startsWith("任务数据失败")) {
    return shown("D2H-PLAN-001","剪辑方案或任务数据无效","当前任务的数据或片段方案未通过校验，本次导出已停止。",["根据技术详情核对录像编号、时间范围、英雄与事件锚点。","保留原方案和排查信息。"],error);
  }
  if(/成片任务已取消|导出已取消/.test(value)) {
    return shown("D2H-CANCEL-001","导出已取消","本次任务已取消，可以核对已保存的片段后决定是否继续。",["核对已保存片段，再选择继续或重新导出。"],error);
  }
  return shown("D2H-EXP-000","本次导出未完成","软件收到尚未分类的错误，原始信息已保留。请复制排查信息，按日志定位具体原因。",["保存本局日志，并复制排查信息；新错误需加入报错排查文档。"],error);
}

export interface RenderDiagnosticContext {
  version:string;module:string;matchId:number|null;jobId:string;
  stage?:string;currentClip?:number;totalClips?:number;logDirectory?:string;
}

export function renderErrorReport(error:string,context:RenderDiagnosticContext):string {
  const info=presentRenderError(error);
  return ["猫猫的剪辑小助手 · 导出排查信息",`错误编号：${info.code}`,`错误说明：${info.title}`,
    `软件版本：${context.version}`,`模块：${context.module}`,`录像编号：${context.matchId??"未提供"}`,
    `任务编号：${context.jobId}`,`失败阶段：${context.stage??"未收到阶段反馈"}`,
    `片段进度：${context.currentClip??"未收到"}/${context.totalClips??"未提供"}`,
    `日志文件夹：${context.logDirectory??"通过软件的打开文件夹入口查看"}`,
    `处理建议：${info.steps.join(" ")}`,"排查文档：docs/ERROR_CATALOG.md（按错误编号查找）","原始错误：",error].join("\n");
}
