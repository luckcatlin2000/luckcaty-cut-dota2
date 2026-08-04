# 当前镜头配方

## 适用范围

这些结论来自 2026-07-30 对当前安装版 Dota 2 离线回放的实际校准。Dota 更新后
必须先用短片段复测，不能把旧命令的存在当作当前行为保证。

计划只保存类型化参数。实际执行时由项目 Replay Controller 编译为 Rust 白名单
命令。

## 已验证控制面

| 参数 | 当前行为 |
|---|---|
| `dota_spectator_mode 0` | Directed |
| `dota_spectator_mode 1` | Free Cam |
| `dota_spectator_mode 2` | Hero Chase |
| `dota_spectator_mode 3` | Player View |
| `dota_spectator_hero_index <slot>` | 选择 `0..9` 玩家位 |
| `dota_camera_focus_player <slot>` | 聚焦对应玩家位 |
| `dota_camera_distance <number>` | 调整俯视距离，当前默认 `1200` |
| `dota_camera_set_lookatpos <x> <y>` | 设置自由镜头注视点 |
| `dota_camera_lerp_position <x> <y>` | 平滑移动自由镜头注视点 |
| `dota_camera_get_pos` | 只读输出相机位置，用于校准 |
| `dota_camera_get_lookatpos` | 只读输出注视点，用于校准 |
| `dota_camera_allow_freecam 1` | 允许受控自由镜头配方 |

## 配方表

### `player-view`

- 模式：Player View，`3`
- 必需：`targetSlot`
- 建议距离：`1200`
- 用途：保留玩家当时的正常操作、判断和镜头移动。
- 默认：每个场次的 `Sxxx-A` 主机位。

### `hero-chase-close`

- 模式：Hero Chase，`2`
- 必需：`targetSlot`
- 建议距离：`480..800`，已验证案例为 `520`
- 用途：强调英雄动作、技能命中和结果反馈。
- 风险：仍是俯视跟随；树林、地形和大型特效可能遮挡主体。
- 后备：`player-view`

### `high-aerial`

- 模式：Free Cam，`1`
- 必需：`lookAt` 和 `distance`
- 建议距离：`1800..2600`，已验证案例为 `2400`
- 用途：说明双方距离、追逃路径和团战空间。
- 风险：过高会进入地图外雾区，英雄和技能可能太小。
- 后备：`player-view`

### `push-track`

- 模式：Free Cam，`1`
- 必需：初始 `lookAt`、初始 `distance` 和一个以上 `cues`
- 建议起始距离：`1400..2000`
- 建议结束距离：`650..1100`
- 已验证案例：`1700 -> 1450 -> 1150 -> 900 -> 720`
- 用途：从空间交代推进到关键动作，并沿追击或弹道方向移动。
- 风险：注视点错误会丢失主体；每次都必须做逐帧或多抽帧检查。
- 后备：`player-view`

## 当前不支持

以下命令或变量没有在当前回放相机中产生可靠效果，不得进入正式配方：

- `spec_goto`
- `thirdperson` / `firstperson`
- `cam_idealdist` / `cam_idealpitch` / `cam_idealyaw`
- 旧 `dota_camera_pitch_*`、yaw 和 FOV 变量

用户参考图中的正面低机位近景目前属于 HOLD。除非未来短片段实测证明新的受控
入口有效，否则不要把 Hero Chase 包装成这种镜头。

## 重新校准

1. 使用项目自有离线 Dota 2 和一个 5 到 8 秒固定事件。
2. 用 `find dota_camera` 和 `cvarlist dota_camera` 读取当前注册项。
3. 先运行只读位置查询，再逐个测试一个类型化参数。
4. 每次只改变一个变量，保存参数、帧图和结果。
5. 对同一时间窗保留 Player View 对照。
6. 通过可见性、运动和清理检查后才更新本文件与 Rust 白名单。
