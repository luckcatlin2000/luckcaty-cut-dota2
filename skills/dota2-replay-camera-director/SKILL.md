---
name: dota2-replay-camera-director
description: Plan and validate evidence-driven Dota 2 replay stories, WTF DirectorProfile decisions, joke-point markers, rough cuts, and synchronized Player View, Hero Chase close, high-aerial, or push-track takes. Use when Codex needs to preserve Precision Edit behavior, compile a validated WTF profile into a local DEM director plan, define S001-A/B/C timing and switch windows, prepare controlled VConsole rendering, or review camera coverage without online-game automation.
---

# Dota 2 Replay Camera Director

将可验证的录像事件转换为剧情节拍、梗点、粗剪顺序和同时间码多机位素材计划。
保持玩家视角为叙事主线，只在动作、空间关系或反应确实需要时增加备用镜头。

## 选择产品模式

- `precision`（精准剪辑）是默认模式，沿用现有英雄、规则、时间和镜头方案。
- `wtf_director` 只能读取通过验证的 `DirectorProfile`，生成独立 WTF 方案；不得覆盖
  精准剪辑方案。
- `candidate` Profile 或 `pilot_only` 故事原型只用于研究，不能进入普通用户运行时。
- 两种模式的行为和文件隔离见 `references/wtf-director-contract.md`。

## 建立 WTF 导演方案

1. 读取 Timeline、Highlight、Story 证据和用户选择的主角。
2. 运行 `validate-wtf-director.ps1 -ProfilePath <profile>`；只有
   `runtimeEligible=true` 才继续。
3. 用 Profile 的 `signal` 匹配证据，并按 `beatProgram` 选择剧情节拍。
4. 把可自动完成的干净剪辑写为 `automatic_clean_edit`；需要外部素材的梗点只写
   `marker_only` 语义槽位。
5. 按 `coverageProgram` 生成最少同步机位，并沿用 `S001-A/B/C` 编号。
6. 生成独立 `WtfDirectorPlan`、相机计划和粗剪顺序，不改写精准方案。
7. 同时运行 WTF 方案验证器和现有相机计划验证器。

证据不足、Profile 未验证、模式引用错误或镜头不可见时保持 HOLD，并回到精准剪辑，
不得用随机候选或模型猜测补齐。

## 建立计划

1. 先读取项目的 `AGENTS.md`、`docs/REPLAY_CONTROL.md` 和故事证据。
2. 明确主角、事件因果、动作起点、结果点和需要保留的连续性。
3. 把一个连续事件定义为一个场次，源入点/出点只保存在场次层。
4. 固定 `S001-A` 为玩家视角主机位；只有明确的镜头目的才能增加 B/C/D。
5. 使用类型化字段描述镜头，不在计划 JSON 中保存原始控制台命令。
6. 为备用机位写明用途、证据和建议切换窗口，并始终保留 A 作为后备。
7. 使用 `references/plan-contract.md` 的合同生成 JSON。
8. 运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File `
  .\skills\dota2-replay-camera-director\scripts\validate-camera-plan.ps1 `
  -Path <camera-plan.json>
```

验证失败时先修正计划，不绕过验证器。

## 选择镜头

按叙事目的选择最少的镜头：

| 目的 | 镜头类型 |
|---|---|
| 保留玩家判断、操作和连续性 | `player-view` |
| 强调英雄动作、命中或受击反馈 | `hero-chase-close` |
| 交代双方位置、追逃路线或团战空间 | `high-aerial` |
| 从环境推进到动作主体并跟随事件 | `push-track` |

读取 `references/camera-recipes.md` 获取当前客户端已经验证的参数和限制。不要把
Hero Chase 称作角色正面的低机位特写；当前它只是跟随英雄的俯视近景。

## 控制素材数量

- 普通操作只保留 A。
- 技术动作通常增加一个备用近景。
- 空间关系对理解事件有必要时才增加高空镜头。
- 只有事件存在明确起点、方向和结果时才增加推进跟移。
- 不因“画面可能更丰富”而机械生成四机位。
- 同一场次的所有机位必须使用完全一致的源时间窗。
- 备用机位不得自动顺序追加到默认成片。

## 执行客户端渲染

把分析、规划和渲染严格分开：

1. DEM 分析、故事判断和计划验证不得启动 Dota 2。
2. 只有用户明确批准客户端画面测试或点击导出后才启动项目自有离线客户端。
3. 启动前告知用户测试内容，并请其暂时不要操作 Dota 2。
4. 记录项目启动的 PID，只控制该 PID 对应的 localhost VConsole。
5. 只通过项目 Rust 白名单把类型化配方编译为命令。
6. 每个机位独立执行原生 `startmovie/endmovie`，统一输出编号。
7. 成功、失败或取消后都关闭项目启动的 Dota 2，并确认 TCP `29000` 不再监听。

不得接管用户已经运行的客户端，不得向在线比赛注入命令，不得使用 OBS、桌面录
屏或任意控制台字符串。

## 质量门禁

每个场次至少核对：

- 所有机位的源时间、时长、帧率和帧数一致。
- 主角、关键动作和结果在建议切换窗口内可见。
- 推进镜头没有把主体移出画面；近景没有被树木或 UI 长时间遮挡。
- 高空镜头仍能看清事件，不把地图外雾区当成有效构图。
- 画面无 HUD、鼠标、黑屏和冻结，音轨存在。
- 文件名、清单和计划中的 `S001-A/B/C/D` 完全一致。

镜头不可见或遮挡时使用 A，不靠猜测继续自动剪辑。

## 暂停条件

遇到以下情况保持 HOLD：

- 用户要求正面低机位、绕拍、固定 pitch/yaw 或当前未注册的镜头参数。
- Dota 2 更新后，已验证命令的行为发生变化。
- 故事没有足够证据解释为什么需要备用镜头。
- 计划需要不同时间码却仍试图放在同一场次。
- 视觉检查无法确认主体和动作。

先做一个短校准片段并更新 `references/camera-recipes.md`，通过后再扩展新配方。

## 资源

- `references/camera-recipes.md`：当前客户端实测镜头参数和适用边界。
- `references/plan-contract.md`：类型化镜头计划合同。
- `references/mirana-arrow-example.json`：不含真实比赛编号的米拉娜示例。
- `references/wtf-director-contract.md`：WTF Profile、运行时方案和模式隔离合同。
- `references/wtf-director-profile-example.json`：合成的已验证 Profile 结构示例。
- `references/wtf-director-plan-example.json`：合成的运行时方案示例。
- `scripts/validate-camera-plan.ps1`：计划验证与自检。
- `scripts/validate-wtf-director.ps1`：Profile、WTF 方案和运行门禁验证。
