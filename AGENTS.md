# 猫猫的剪辑小助手

## 目标

维护一个仅处理本地 Dota 2 DEM 和离线回放导出的 Windows 剪辑工具。

## 入口

- 用户与开发入口：`README.md`
- Rust 工作区：`Cargo.toml`
- 桌面前端：`apps/d2-highlights-desktop`
- Tauri 后端：`apps/d2-highlights-desktop/src-tauri`
- DEM 解析：`crates/d2-highlights-parser-source2`
- 回放控制：`crates/d2-highlights-replay-control`
- 渲染与 FFmpeg：`crates/d2-highlights-renderer`
- 验证：`scripts/verify.ps1`

## 边界

- 原始 `.dem` 只读；不得提交用户录像、任务目录或成片。
- 不控制在线比赛，不接管已有 Dota 2，不注入进程，不绕过 VAC。
- 只有用户确认导出后才允许启动项目自己的 `-insecure` 离线客户端。
- 回放控制只连接 localhost，命令必须保持白名单。
- FFmpeg 二进制、EXE 和安装包不得进入 Git 历史。
- 不使用真实比赛编号、玩家姓名、SteamID 或本机绝对路径作为公开测试数据。
- 仓库只维护“猫猫的剪辑小助手”，不得加入或宣传无关软件或其他产品。
- 项目自有代码使用根目录源码公开许可证；不得重新加入 MIT 等允许未经授权销售的许可。
- 修改许可、品牌、商业使用或分发规则时，必须同步更新 `README.md`、`LICENSE.zh-CN.md`、`CONTRIBUTING.md` 和 `docs/BRAND_AND_COMMERCIAL_USE.md`。

## 验证

```powershell
.\scripts\verify.ps1
```

涉及真实 Dota 2 客户端的端到端测试必须由维护者明确启动，并且只关闭本次测试创建的 PID。
