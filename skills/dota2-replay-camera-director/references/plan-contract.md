# 类型化镜头计划合同

## 顶层

```json
{
  "schemaVersion": "d2h.camera-skill-plan/1.0",
  "replayRef": "local-example",
  "protagonist": {
    "hero": "npc_dota_hero_mirana",
    "slot": 9
  },
  "scenes": []
}
```

- `replayRef` 只保存本地引用或脱敏示例，不在公开 Skill 中写真实用户比赛编号。
- `protagonist.slot` 必须为 `0..9`。
- `scenes` 至少包含一个场次。

## 场次

每个场次包含：

| 字段 | 规则 |
|---|---|
| `sceneId` | `S001` 格式，按时间顺序递增 |
| `candidateId` | 可追溯到故事证据的稳定事件 ID |
| `storyPurpose` | 说明该场次在剧情中的作用 |
| `evidenceIds` | 至少一个事实证据 ID，不得重复 |
| `source` | 唯一源时间窗，所有机位共同使用 |
| `takes` | `S001-A/B/C...` 类型化机位 |
| `suggestedSwitchWindows` | 可选备用机位切换窗口 |
| `fallbackTakeId` | 必须指向主机位 A |

`source` 同时保存秒数和 tick：

```json
{
  "startSeconds": 110.5,
  "endSeconds": 117.5,
  "startTick": 3250,
  "endTick": 3460
}
```

时长必须为 `1..90` 秒。机位不能各自改时间；需要不同时间时建立新场次。

## 机位

通用字段：

```json
{
  "takeId": "S001-A",
  "cameraType": "player-view",
  "primary": true,
  "targetSlot": 9,
  "distance": 1200
}
```

规则：

- A 必须是唯一主机位和 `player-view`。
- B/C/D 按数组顺序连续编号。
- `cameraType` 只允许 `player-view`、`hero-chase-close`、`high-aerial`、
  `push-track`。
- `player-view` 和 `hero-chase-close` 必须有 `targetSlot`。
- 自由镜头必须有 `lookAt: {"x": number, "y": number}`。
- `distance` 必须位于项目校准边界 `400..3000`。
- 计划中不允许 `command`、pitch、yaw、FOV 或任意额外字段。

`push-track` 使用按场次偏移秒数排序的 `cues`：

```json
{
  "atSeconds": 2.5,
  "distance": 1450,
  "lookAt": {
    "x": 1720,
    "y": -60
  }
}
```

每个 cue 至少改变距离或注视点，必须严格位于场次内部。

## 建议切换窗口

```json
{
  "takeId": "S001-B",
  "startOffsetSeconds": 2.2,
  "endOffsetSeconds": 4.6,
  "reason": "神箭命中与击杀反馈"
}
```

- 只能引用备用机位。
- 时间是相对场次入点的偏移。
- 窗口必须完整位于场次时长内。
- 它只是后期建议，不会让备用素材自动进入默认成片。

## 验证

运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File `
  .\scripts\validate-camera-plan.ps1 -Path <plan.json>
```

退出码 `0` 表示结构合同通过；它不替代真实画面的可见性检查。
