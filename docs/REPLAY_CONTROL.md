# 回放控制合同

## 安全边界

- 只连接 `127.0.0.1:29000`。
- 只控制项目以 `-insecure -vconsole -console` 启动的离线 Dota 2。
- 不修改游戏二进制、Steam 永久启动项或用户视频配置。
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

`resume` 和 `goto` 不得在同一无间隔批次发送。单段当前限制为 1 到 90 秒。

## 镜头映射

解析器保存 10 名玩家的有序 `slot` 与 `hero_name`。每段的英雄映射到 `slot 0..9`。

| UI 模式 | Dota 命令 |
|---|---|
| 玩家视角 | `dota_spectator_hero_index <slot>`，`dota_spectator_mode 3`，`dota_camera_focus_player <slot>` |
| 英雄近景 | `dota_spectator_hero_index <slot>`，`dota_spectator_mode 2`，`dota_camera_focus_player <slot>` |

`1.2.2` 不再使用 `dota_camera_set_lookatpos` 战斗坐标来宣称英雄视角，也不会在爆点和结尾强制跳到其他地图坐标。

选择英雄会自动绑定该玩家位并使用玩家视角。只有用户明确选择“英雄近景”时才使用 Hero Chase。旧方案中的 Directed 和 Free Cam 在加载、保存和渲染前都会归一为玩家视角，当前 UI 不再暴露自由相机或滚轮缩放。

## 画面与声音

- 干净画面使用 HUD、鼠标和 Panorama 隐藏命令。
- 当前渲染器调用 Dota 2 原生 `startmovie/endmovie` 生成固定帧率 JPG 和 WAV。
- FFmpeg 只负责编码、按顺序拼接、游戏原声处理和 QC。
- 不添加 BGM、配音、音效、自动慢动作或结尾回看。

## 当前验证状态

- VConsole 命令白名单和 `CMND` 数据包测试通过。
- 玩家视角与英雄追踪的玩家位命令序列有单元测试。
- 手工秒数到候选锚点 tick 的换算测试通过。
- 桌面 UI 已验证每段英雄和镜头选择会进入导出预览。
- 用户已确认当前 Dota 2 Clip Builder 可用，并要求本轮不重复客户端验证；因此本轮没有把观战模式的实际输出画面重新标记为通过。
