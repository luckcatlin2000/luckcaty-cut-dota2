# WTF DirectorProfile 与运行时方案合同

## 职责

- `d2h.wtf-director-profile/1.0`：保存从视频案例和 DEM 回归中验证出的导演规则。
- `d2h.wtf-director-plan/1.0`：保存某一份 DEM 应用规则后得到的故事、梗点和粗剪顺序。
- `d2h.camera-skill-plan/1.0`：继续负责同时间码的类型化同步机位。

Profile 不保存真实比赛编号、用户本地路径、任意控制台命令或第三方媒体文件。Plan
只引用该任务已有的证据 ID 和独立相机计划。

## Profile 顶层

| 字段 | 规则 |
|---|---|
| `schemaVersion` | 固定 `d2h.wtf-director-profile/1.0` |
| `profileId` | 稳定小写连字符 ID |
| `profileVersion` | 三段语义版本 |
| `status` | `candidate` 或 `validated` |
| `mode` | 固定 `wtf_director` |
| `displayName` | 用户可读名称 |
| `sourceRefs` | 可审计来源；正式 Profile 至少两个独立来源 |
| `runtimePolicy` | 本地运行、后备模式和外部素材边界 |
| `storyPatterns` | 可执行故事原型 |
| `episodePolicy` | 多故事排序、时长和结尾偏好 |

`candidate` 可以通过结构验证，但不得进入普通用户运行时。只有 `validated` 且每个原型
均为 `validated` 时，Profile 才具备 `runtimeEligible=true`。

## 故事原型

每个 `storyPatterns[]` 包含：

- `signal`：需要哪些证据、如何分组、最少出现次数、最大跨度和最低置信度；
- `beatProgram`：从证据选择建立、识别、升级、转折、机制证明、反应、收尾或结果；
- `jokePointProgram`：在某一节拍建立回调、反应槽、机制卡、冲击停顿或干净回看；
- `coverageProgram`：每个节拍的主机位、可选机位和启用条件；
- `selectionPolicy`：一组重复证据最多进入多少次、是否强制保留第一次和最后一次。

梗点的 `execution` 只允许：

- `automatic_clean_edit`：可由现有干净游戏素材自动完成；
- `marker_only`：需要授权素材或用户后期决定，只输出语义位置。

## 运行时 Plan

Plan 顶层必须包含 Profile 引用、源 DEM 哈希、主角、故事、相机计划引用、粗剪顺序和
复核状态。每个故事必须引用一个 Profile 原型，并保存：

- 置信度与判定理由；
- 来自 Timeline/Highlight/Story 的证据 ID；
- 有源入点/出点的剧情节拍；
- 有绝对时间的梗点；
- 对应相机计划中的 `S001/S002...` 场次。

`roughCut[]` 只引用已经存在的场次和 `Sxxx-A/B/C` 机位。首版默认使用 A；备用机位
只有在相机计划存在建议切换窗且通过媒体 QC 后才能进入自动粗剪。

## 编译边界

```text
validated DirectorProfile
  + d2h.story evidence
  -> WtfDirectorPlan
  -> d2h.camera-skill-plan/1.0
  -> existing d2h.edit-plan
```

任何一层缺少证据、Profile 未验证、时间窗不合法或镜头不可见时保持 HOLD，并允许用户
回到精准剪辑。不得用模型猜测补齐结构化证据。

## 验证

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File `
  .\skills\dota2-replay-camera-director\scripts\validate-wtf-director.ps1 `
  -ProfilePath <profile.json>

powershell -NoProfile -ExecutionPolicy Bypass -File `
  .\skills\dota2-replay-camera-director\scripts\validate-wtf-director.ps1 `
  -ProfilePath <profile.json> -PlanPath <plan.json>
```

结构通过不等于故事质量通过；正式 Profile 还需要跨视频证据和保留样本盲测。
