# 回放控制合同

## 安全边界

- 只连接 `127.0.0.1:29000`。
- 只控制项目以 `-insecure -vconsole -console` 启动的离线 Dota 2。
- 不修改游戏二进制、Steam 永久启动项或用户视频配置。
- 不读取、复制、解析或修改 Steam/Dota 2 账户与用户配置，不启动、登录或控制 Steam。
- 命令必须通过 Rust 白名单，禁止换行、NUL 和任意命令拼接。
- 拒绝控制用户已经运行的 Dota 2。
- 任务结束后只关闭本项目启动的 PID，并确认 TCP `29000` 不再监听。
- 需要客户端测试时，启动前告知用户，结束后报告关闭结果。

## 时间定位

1. 每段保存一个候选 `candidateId` 作为事件锚点。
2. 从候选取得真实 `anchor_tick` 和 `peak_seconds`。
3. 入点、出点按回放 tickrate 相对锚点换算。
4. 先发送 `demo_resume`，再单独发送 `demo_goto <tick> absolute pause`。
5. 等待客户端稳定到达目标 tick 后再设置镜头和开始原生导出。
6. 使用 `demo_pauseatservertick <end_tick>` 精确停止该段。

`resume` 和 `goto` 不得在同一无间隔批次发送。单段当前限制为 1 到 100 秒；剧情模式前置请求不超过 90 秒，末杀后固定保留 10 秒。

## 镜头映射

解析器保存 10 名玩家的有序 `slot` 与 `hero_name`。每段的英雄映射到 `slot 0..9`。

| UI 模式 | Dota 命令 |
|---|---|
| 玩家视角 | `dota_spectator_hero_index <slot>`，`dota_spectator_mode 3`，`dota_camera_focus_player <slot>` |
| 英雄跟随近景 | `dota_spectator_hero_index <slot>`，`dota_spectator_mode 2`，`dota_camera_focus_player <slot>` |

`1.2.2` 不再使用 `dota_camera_set_lookatpos` 战斗坐标来宣称英雄视角，也不会在爆点和结尾强制跳到其他地图坐标。

选择英雄会自动绑定该玩家位并使用玩家视角。只有用户明确选择“英雄跟随”时才使用 Hero Chase。旧方案中的 Directed 和 Free Cam 在加载、保存和渲染前都会归一为玩家视角，当前 UI 不再暴露自由相机或滚轮缩放。

同一故事场次可以包含玩家视角和一个或多个备用近景。它们必须使用同一 `candidateId`、完全一致的源入点/出点和统一 `takeGroupId`，分别执行原生导出并按 `S001-A/B/C` 保存。是否增加近景及其目标由故事证据决定，不使用固定双机位模板。

## 当前客户端镜头参数校准

2026-07-30 在当前安装的 Dota 2 回放中通过 VConsole 实测：

- `dota_spectator_mode 0/1/2/3` 分别对应 Directed、Free Cam、Hero Chase 和 Player View。
- `dota_camera_distance <number>` 控制俯视距离，当前默认值为 `1200`。
- `dota_camera_set_lookatpos <x> <y>` 设置自由镜头注视点。
- `dota_camera_lerp_position <x> <y>` 使自由镜头向新注视点平滑移动。
- `dota_camera_get_pos` 与 `dota_camera_get_lookatpos` 用于只读校准。
- `dota_camera_allow_freecam 1` 允许受控自由镜头配方。

旧资料中的 `spec_goto`、`thirdperson`、`cam_ideal*` 以及 pitch/yaw/FOV
变量没有在当前回放相机中产生可靠效果，因此不进入正式命令白名单。现阶段的
“英雄跟随近景”是缩短距离的 Hero Chase，不能宣称为角色正面的低机位电影镜头。

开发用四镜头校准脚本：

```powershell
pwsh -NoProfile -File .\scripts\camera-showcase.ps1 -DotaPid <app-owned-pid>
```

脚本对同一事件时间窗分别导出玩家视角、英雄跟随近景、高空俯视和推进跟移，
并生成同步对照、顺序展示及参数清单。它只接受项目自有 Dota 2 PID，不属于用户
界面的生产入口。

## 画面与声音

- 普通画面只发送 `dota_spectator_options_enabled 0` 收起右上角回放控制模块，不发送小地图位置、大小、HUD 翻转或其他用户偏好命令。
- 普通画面使用 Dota 2 原生昵称和击杀栏，不用 ASS、`drawtext`、遮罩或其他后期图层替换。导出前用户需手动打开并登录 Steam，软件只检查 `steam.exe` 是否运行。
- 干净画面在普通画面命令之上使用 HUD、鼠标和 Panorama 隐藏命令，且不需要昵称上下文。
- 当前渲染器调用 Dota 2 原生 `startmovie/endmovie` 生成固定帧率 JPG 和 WAV。
- FFmpeg 只负责编码、按顺序拼接、游戏原声处理和 QC。
- 不添加 BGM、配音、音效、自动慢动作或结尾回看。

## 当前验证状态

- VConsole 命令白名单和 `CMND` 数据包测试通过。
- 玩家视角与英雄追踪的玩家位命令序列有单元测试。
- 手工秒数到候选锚点 tick 的换算测试通过。
- 桌面 UI 已验证每段英雄和镜头选择会进入导出预览。
- `1.8.0` 已完成米拉娜同场双机位真实回归：Player View 与 Hero Chase 均为相同 5.000 秒时间窗、1920x1080、30 FPS、H.264/AAC 双声道，抽帧确认构图不同。
- 米拉娜神箭事件 `1107.500-1114.500` 已完成 A/B/C/D 四镜头校准：四份独立素材均为相同 7.000 秒时间窗，另有同步四宫格和 28.074 秒顺序展示。
- 默认成片只包含 `S001-A` 主机位，时长 5.021 秒；`S001-B` 作为独立备用素材存在，0 黑屏、0 冻结、0 QC warning。
- 测试结束后项目启动的 Dota 2 已关闭，FFmpeg/FFprobe/VConsole2 无残留，TCP `29000` 未监听。
