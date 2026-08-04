# WTF 导演模式合同

## 用户模式

工作台使用分段控件显示两个互斥模式：

`[ 精准剪辑 ] [ WTF 导演 ]`

| 显示名 | 内部 ID | 默认 | 行为 |
|---|---|---|---|
| 精准剪辑 | `precision` | 是 | 保留当前英雄、内容规则、精确时间和镜头编辑流程 |
| WTF 导演 | `wtf_director` | 否 | 在既有 DEM 分析结果上运行故事原型、梗点和镜头编排 |

不得使用“普通模式”作为显示名。精准剪辑不是降级路径，只是比 WTF 导演更可控、
更少自动推断。

## 切换语义

- 新任务默认 `precision`。
- 切换到 `wtf_director` 不重新解析同一份 DEM；只有时间线、高光或故事 schema
  失效时才重算对应阶段。
- 两个模式分别保存方案。切换模式不得删除、覆盖或隐式改写另一份方案。
- 旧任务没有模式字段时按 `precision` 打开，继续读取现有
  `director/edit-plan.json`。
- WTF 模式无法产生合格方案时显示证据和原因，并允许返回精准剪辑；不得用随机片段
  冒充结果。

## 数据流

```text
Timeline + Highlight + Story evidence
  -> WTF DirectorProfile
  -> deterministic pattern matching
  -> WtfDirectorPlan (story beats + joke points + rough cut)
  -> camera skill plan (S001-A/B/C)
  -> existing Edit Plan / Replay Controller / Capture / Editor
```

`SKILL.md` 是蒸馏、审计和维护入口，普通用户的软件不会直接运行 Codex Skill。
只有通过验证的规则才会编译或移植到本地 Rust 导演引擎。第一版不得依赖云端账号、
Codex 会话或用户机器上的生成式 AI 算力。

## 文件隔离

```text
jobs/<job-id>/director/
  edit-plan.json             # 现有精准剪辑方案，兼容旧任务
  wtf-director-plan.json     # WTF 故事、梗点和粗剪顺序
  wtf-camera-plan.json       # WTF 同步机位计划
```

任务清单未来增加 `active_edit_mode`，只决定 UI 当前显示和导出来源，不改变另一模式
的文件。

## 梗点边界

WTF 导演首先自动执行干净游戏素材内部的剪切、停顿、回到证据、重复呼应和镜头切换。
外部表情、动画、BGM 或音效需要独立的合法素材库和授权合同。在素材库落地前，
`reaction_slot`、`mechanic_card` 等只生成语义槽位，不下载或复制第三方素材。

## 客户端门禁

- 模式切换、故事分析、梗点规划和方案复核不得启动 Dota 2。
- 只有用户点击导出后，WTF 方案才编译为现有类型化相机计划并进入 Replay Controller。
- WTF 模式复用现有 PID、VConsole 白名单、原生 `startmovie/endmovie` 和自动关闭门禁。
- 任一备用机位不可见或失败时回退同场 `A` 玩家视角，不影响精准剪辑方案。

## 首版验收

1. 默认打开精准剪辑，现有米拉娜和风行者回归结果不变。
2. 两种模式来回切换后，各自片段、顺序和时间仍保持。
3. 风行者录像在 WTF 模式把重复砍树组织为建立、升级、收尾，而不是七个孤立候选。
4. 米拉娜录像在 WTF 模式能区分连续击杀、技术动作和普通单杀的镜头预算。
5. WTF 分析不启动 Dota 2；导出完成或失败后项目启动的客户端被关闭。
6. 未验证的蒸馏规则不能进入普通用户运行时。
