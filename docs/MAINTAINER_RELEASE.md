# 维护者发布流程

## 源码门禁

```powershell
.\scripts\verify.ps1
```

确认 Git 中不存在：

- `.dem`、成片、WAV 或任务缓存；
- EXE、安装包、FFmpeg 二进制；
- 真实 SteamID、玩家姓名、比赛编号；
- 开发机绝对路径、密钥或本地配置。

同时确认：

- 仓库、About、Topics、Issues、Releases、Wiki 和附件只包含“猫猫的剪辑小助手”信息；
- 没有无关软件或其他产品的名称、宣传、链接或发布物；
- 根目录 `LICENSE`、`LICENSE.zh-CN.md`、README 和商业使用文档一致；
- 没有重新出现 MIT 等允许未经授权销售或再分发本项目自有代码的声明。

## 构建

源码版要求 FFmpeg/FFprobe 位于 PATH、环境变量指定位置或 `tools/ffmpeg/bin/`。

```powershell
.\scripts\desktop-release.ps1
```

构建结果进入被忽略的 `release/`，不要把二进制提交到 Git 历史。

## GitHub Release

每个发布版本建议附带：

- 最新安装包或便携包；
- SHA-256；
- 版本说明；
- 第三方 notices；
- 如果捆绑 FFmpeg，附带该构建对应的许可证、源码和构建信息。

只有著作权人或持有明确书面授权的发布者可以对外发布安装包。不得授权第三方改名换皮、套壳销售或删除项目来源信息，除非书面授权文件明确允许这些行为。

发布到 GitHub 是外部操作，必须在复核仓库、目标账号、仓库名称、标签和附件后单独执行。
