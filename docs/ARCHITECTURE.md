# 系统架构

## 主数据流

```text
Tauri Desktop UI
  -> Import / Progress / Precise Clip Workspace
  -> .dem
  -> Ingest（校验、哈希、版本、任务目录）
  -> Parse（成熟 Dota 2 DEM 解析器）
  -> Timeline（完整 10 人阵容、统一事件、实体、位置和状态 JSON）
  -> Highlight Engine（只作为时间与 tick 定位锚点）
  -> Local Highlight Rules（高光主角、击杀/砍树内容筛选）
  -> Manual Edit Plan（片段顺序、入点、出点、英雄、镜头）
  -> Replay Controller（启动、精确跳转、官方观战模式）
  -> Capture（视频、游戏音频、时间码、完整性检测）
  -> Editor（按顺序拼接、游戏原声编码）
  -> QC（黑屏、冻结、响度、时长和编码）
  -> Deliver（MP4、时间轴、manifest、日志）
```

## 模块边界

### 1. Ingest

只读原始 DEM，创建内容哈希和任务 ID。所有派生文件进入独立任务目录，禁止覆盖源文件。

### 2. Parser Adapter

第三方 Dota 2 解析框架只能出现在适配器后方。适配器把框架特有对象转换为项目自己的版本化 JSON Schema。

### 3. Highlight Engine

采用可解释规则生成事件候选。`1.2.0` 中候选不是自动成片决定，只提供标题、峰值秒数和真实 `anchor_tick`，用户可自由调整片段时间和顺序。

### 4. Manual Edit Plan

`d2h.edit-plan/1.3` 保存有序片段。每段包含稳定 `clipId`、`candidateId`、入点、出点、目标英雄和镜头模式。当前用户合同只允许 `player_perspective` 和 `hero_chase`；旧方案中的 `directed/free_camera` 会归一为 `player_perspective`。当候选检测合同升级时，旧 schema 的方案不会覆盖新英雄筛选结果。同一候选可复制成多段，缓存不再因重复候选冲突。

### 5. Replay Controller

只控制本地回放。所有客户端命令必须列入白名单并可恢复；不得注入在线进程。当前实现通过 Valve VConsole2 的本机 TCP 控制面发送 `CMND`，只允许 `127.0.0.1`。

片段时间使用检测器保存的真实 `anchor_tick` 相对换算，不从比赛秒数猜 DEM 零点。英雄由完整阵容的 `slot 0..9` 映射到 Dota 观战玩家位。选择英雄默认生成 Player View 命令，只有用户明确选择英雄近景时才生成 Hero Chase 命令；不再用战斗坐标冒充玩家视角。

### 6. Capture

离线捕获与游戏控制分离。正式路径是 Dota 2 原生 `startmovie/endmovie` 输出固定帧率帧序列和 WAV，再由 FFmpeg 按帧数/时间码裁切编码；真实回放已完成 1080p、30 FPS、同步 WAV 和 MP4 验证。原生命令会记录暂停等待和命令往返产生的额外帧，渲染服务必须保留预滚后按稳定帧边界裁切，不能把墙钟等待直接当作成片时长。当前产品不依赖 OBS 或独立桌面录制程序。

### 7. Editor

`d2-highlights-renderer` 按用户片段顺序拼接原生导出，保留游戏原声并生成 1920x1080 H.264/AAC MP4。`1.2.0` 在桌面后端强制禁用 BGM、旁白、音效、自动慢动作和重点回看。FFprobe、blackdetect、freezedetect 和 volumedetect 生成 QC JSON。

每个片段以 DEM、时间窗、镜头和声音设置的指纹作为缓存键。失败或取消后重试可以复用已经完成的原生导出；设置变化时缓存自动失效。

### 8. Local App

桌面界面采用 Tauri 2 + React/TypeScript，只调用稳定的 Rust 任务 API，不包含解析或剪辑业务逻辑。CLI 与 GUI 共用同一服务层，便于自动测试和批处理。

UI 提供拖放/文件选择、完整 10 人阵容、高光主角与内容规则筛选、片段增删复制排序、毫秒时间码、30 FPS 单帧微调、逐段英雄与镜头、导出预览、应用内 MP4 播放、失败恢复和输出定位。内容筛选使用本地版本化规则 ID，文本输入只做确定性关键词解析；未知内容整体拒绝，不进行云端推断或部分猜测。长任务由后台 worker 执行，通过有类型的命令和进度通道更新界面。

## 运行模式与依赖

| 模式 | Dota 2 客户端 | 可用能力 |
|------|---------------|----------|
| 分析模式 | 不需要 | DEM 导入、解析、检测、评分、导演计划、时间轴复核 |
| 本地成片模式 | 需要安装，但平时不运行 | 选中片段离线回放、镜头渲染、FFmpeg 编码与后期 |
| 云端渲染模式 | 第一版不提供 | 未来可把已验证的控制计划发送到专用渲染节点 |

解析器不得调用、启动或探测 Dota 2 进程。Replay Controller 是唯一允许启动客户端的模块，并且只能在成片任务进入渲染阶段后执行。

## 工件目录约定

```text
jobs/<job-id>/
  input/
  timeline/
  director/
  capture/
  edit/
  qc/
  logs/
  manifest.json
```

## 已核验基线

- `source2-demo 0.5.8`：当前 Rust 解析适配器。
- Valve VConsole2：本机回放控制面，项目自有窄适配器。
- Dota 2 `startmovie/endmovie`：已验证的主离线素材输出路径。
- FFmpeg/FFprobe：帧序列编码、编辑、媒体探测、黑屏和冻结 QC。

Windows UI 使用 Tauri 2 + React/TypeScript。公开源码构建不捆绑 FFmpeg/FFprobe，运行时从环境变量、`tools/ffmpeg/bin` 或 PATH 发现；发布者如需捆绑，必须为实际二进制提供对应许可证和源码材料。录像目录由用户输入或原生目录选择器提供并保存在本机，检测到的 Dota 安装目录只作为首次建议；编号查找由 Rust 后端限制为纯数字并校验 Source 2 DEM 文件头。检测器一方面把技能/交战起手、击杀归属和英雄死亡组成按英雄索引的连续击杀段，另一方面继续把对地橡栗、临时树实体生命周期、指定树指令、补刀斧动作和玩家打赏组成可验证的非击杀互动证据。旧候选和旧剪辑方案会按 schema、检测器名称和版本失效后重算。Windows 正式 EXE 必须通过 PE GUI 子系统门禁，避免伴随终端窗口。真实 DEM 全流程已在维护环境通过，公开贡献仍应在干净 Windows 环境完成冷启动验证。
