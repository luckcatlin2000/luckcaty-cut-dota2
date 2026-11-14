# 猫猫的剪辑小助手

## 目标

维护一个 Windows 本地 Dota 2 DEM 高光精确剪辑工具：

`只读击杀解析 -> 高光/人工片段 -> 编号同步机位 -> Dota 2 离线导出 -> 游戏原声 MP4`

当前桌面主程序仅内置高光，普通/干净画面保留。WTF剪辑为独立模块，与录像分析分开，高光中不再提供默认/WTF切换；剧情与录像分析通过外接入口衔接。通用接口约定见 `docs/RECORDING_INTEGRATION.md`，不得把占位按钮描述为已安装、已连接或已完成全部自动规则迁移。

2.5.0 R3已由用户确认并批准上传GitHub；线上状态按实际发布结果记录，原入口不因候选构建自动替换。当前GitHub交付范围只有高光时刻；WTF、剧情和录像分析均未正式提供，主程序按钮统一提示，不因本机存在内部候选而启动。发布状态由`apps/d2-highlights-desktop/module-catalog.json`统一管理；正式提供并开放对应状态后才按`maomao.module/1`发现已安装模块并打开，未安装时提示下载。既往高光验收不扩大为本轮所有机位、新分析场景或外部机器故障均已解决。

默认构建和安装包仅包含高光主程序。内部WTF入口位于`src-tauri/src/wtf_entry.rs`，须显式启用`wtf-module`特性单独构建，不放入`src/bin`自动发现目录。发布脚本必须核对实际生成的NSIS载荷白名单；不能只依据资源配置或便携目录声称安装包没有其它模块。

## 入口

- 用户与开发入口：`README.md`
- Rust 工作区：`Cargo.toml`
- 桌面前端：`apps/d2-highlights-desktop`
- Tauri 后端：`apps/d2-highlights-desktop/src-tauri`
- 回放控制：`crates/d2-highlights-replay-control`
- 渲染与 FFmpeg：`crates/d2-highlights-renderer`
- 验证：`scripts/verify.ps1`
- 当前基线：`docs/CURRENT_STATE.md`；本地归档与目录规则：`docs/LOCAL_WORKSPACE.md`
- 本仓库为唯一日常开发入口；`.local/` 为本机恢复资料，禁止提交。

## 边界

- 原始 `.dem` 只读；不得提交用户录像、任务目录或成片。
- DEM 导入、解析、候选检测、故事生成、时间编辑和方案保存不得启动 Dota 2。
- 只有用户点击“开始导出视频”后，Replay Controller 才可启动自己的 `-insecure` 离线 Dota 2。
- 不控制在线比赛，不接管已有 Dota 2，不注入进程，不绕过 VAC。
- 回放控制只连接 localhost，命令必须保持白名单。
- 不引入 OBS、云端账号、AI 文案、AI 音频或外部 BGM。
- 同一故事场次的多个机位必须保存相同候选和时间窗，统一编号为 `S001-A/B/C`；备用机位不得增加默认成片时长。
- 机位数量和目标由故事证据决定，不得把所有场次固定为同一种模板。
- FFmpeg 二进制、EXE 和安装包不得进入 Git 历史。
- 不使用真实比赛编号、玩家姓名、SteamID 或本机绝对路径作为公开测试数据。
- 项目自有代码使用根目录源码公开许可证；不得重新加入允许未经授权商业分发的许可。
- 修改许可、品牌或商业规则时，同步更新 `README.md`、`LICENSE.zh-CN.md`、`CONTRIBUTING.md` 和 `docs/BRAND_AND_COMMERCIAL_USE.md`。

## Dota 2 测试门禁

- 启动 Dota 2 前必须说明测试内容和预计时长。
- 记录本次启动 PID，结束时只关闭本项目启动的进程。
- 测试后确认该 PID 不存在、TCP `29000` 不再监听。
- 不得关闭用户自行启动的 Dota 2。

## 完成定义

- `cargo fmt --all -- --check` 通过。
- `cargo test --workspace` 通过。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 全部前端测试与生产构建通过。
- 默认高光、旧 WTF 草稿保全、同步素材编号、片段编辑、10 人英雄选择、外接提示和导出预览有自动测试覆盖。
- 正式发布通过版本标签、候选哈希、完整验证和冷启动门禁。

```powershell
.\scripts\verify.ps1
```
