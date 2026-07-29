# 猫猫的剪辑小助手

一个面向 Windows 的本地 Dota 2 DEM 精确剪辑工具。

官方仓库：<https://github.com/luckcatlin2000/luckcaty-cut-dota2>

第一次了解本项目，可以先阅读[软件介绍](软件介绍.md)。

它只读解析用户选择的 `.dem`，把事件转换成可人工调整的片段清单；只有用户确认导出后，才会启动项目自己的 `-insecure` 离线 Dota 2 回放进程，使用官方观战模式导出画面与原声，再交给 FFmpeg 编码和质量检查。

本项目不控制联网比赛，不注入游戏进程，不绕过 VAC。默认导出只保留游戏原声，不自动添加文案、BGM、配音或音效。

> **许可证提醒：源码公开不等于允许售卖。** 未经著作权人事先书面授权，禁止销售、收费分发、发布安装包、改名换皮、套壳发布或将本软件作为托管/商业产品提供。普通用户可以使用本软件剪辑、发布和变现自己的视频。

## 下载与启动

普通用户不需要安装 Rust、Node.js 或自行编译。请前往
[GitHub Releases](https://github.com/luckcatlin2000/luckcaty-cut-dota2/releases/latest)
下载：

- `luckcaty-cut-dota2_1.6.0_x64-setup.exe`：推荐，双击后按提示安装；
- `luckcaty-cut-dota2_1.6.0_portable.exe`：无需安装，下载后直接运行。

当前安装包和便携版未包含 FFmpeg/FFprobe。首次导出前请按下方“环境要求”
配置 FFmpeg。发布附件由本仓库对应版本源码构建，SHA-256 会列在 Release
说明中供核对。

安装包目前没有商业代码签名证书。Windows 如果显示 SmartScreen 提醒，
请先核对下载地址确实属于本官方仓库，并核对 Release 中的 SHA-256；不要
从第三方网盘、店铺或重新打包的网站下载。

## 功能

- 本地解析 Dota 2 DEM、比赛信息、10 人阵容和事件时间轴。
- 生成可解释的击杀和已验证技术互动候选。
- 增删、复制、排序片段并精确编辑 `HH:MM:SS.mmm` 时间。
- 每段选择玩家视角或英雄近景。
- 使用 Dota 2 原生 `startmovie/endmovie` 导出帧序列与 WAV。
- 使用 FFmpeg 拼接、编码并检查黑屏、冻结、时长和音频。
- 所有任务数据保存在本机，原始 DEM 始终只读。

## 安全边界

- DEM 分析、候选检测和时间编辑不会启动 Dota 2。
- 回放控制只连接 `127.0.0.1:29000`。
- 软件拒绝接管用户已经运行的 Dota 2。
- 导出进程使用 `-insecure -vconsole -console`，任务结束后只关闭本项目启动的 PID。
- 客户端命令经过白名单，不允许任意命令拼接。

详细设计见 [回放控制合同](docs/REPLAY_CONTROL.md)。

## 环境要求

- Windows 10/11 x64
- Rust 1.89 或更高兼容版本
- 当前 Node.js LTS 与 npm
- FFmpeg 和 FFprobe
- 仅在导出真实画面时需要安装 Steam 版 Dota 2

源码仓库不包含 FFmpeg 二进制。请将 `ffmpeg.exe` 和 `ffprobe.exe` 放入 `PATH`，或设置：

```powershell
$env:FFMPEG_EXE = "[FFMPEG_DIR]\ffmpeg.exe"
$env:FFPROBE_EXE = "[FFMPEG_DIR]\ffprobe.exe"
```

也可以放到：

```text
tools\ffmpeg\bin\
```

该目录已被 Git 忽略。

## 开发

安装前端依赖：

```powershell
cd .\apps\d2-highlights-desktop
npm ci
cd ..\..
```

运行完整验证：

```powershell
.\scripts\verify.ps1
```

启动桌面开发版：

```powershell
.\scripts\desktop-dev.ps1
```

构建 Windows 安装包：

```powershell
.\scripts\desktop-release.ps1
```

默认公开构建不捆绑 FFmpeg；运行机器需要自行提供 FFmpeg/FFprobe。发布者如需捆绑二进制，必须单独履行相应许可证和源码提供义务。

## CLI

```powershell
.\scripts\analyze.ps1 -DemPath "[DEM_PATH]\match.dem"
.\scripts\control-plan.ps1 -JobId "d2h-xxxxxxxxxxxxxxxx"
```

运行数据写入 `jobs/`，该目录不会进入 Git。

## 仓库边界

仓库只包含源码、锁文件、构建脚本和维护文档。以下内容不会提交：

- DEM、成片、WAV 和任务缓存
- `target/`、`node_modules/` 和前端构建结果
- EXE、安装包和历史发布物
- FFmpeg/FFprobe 二进制
- 本机路径、账号配置和私有素材

## 仓库唯一主题

本仓库只用于“猫猫的剪辑小助手”。README、About、Topics、Issues、Releases、Wiki 和附件不得混入、宣传或链接无关软件或其他产品。

## 许可证、商业使用与商标

本项目自有源码采用 [Cat Cut Assistant Source-Available License 1.0](LICENSE)，中文说明见 [LICENSE.zh-CN.md](LICENSE.zh-CN.md)。

允许查看、学习、编译、正常使用、制作并变现自己的视频，以及为官方仓库贡献代码。未经著作权人事先书面授权，禁止：

- 售卖、转售、出租、收费下载或收费授权本软件；
- 分发 EXE、安装包、修改版或应用商店版本；
- 改名、换皮、换图标、重新包装或冒充原创后套壳发布；
- 将本软件主要代码并入其他产品、付费合集、商业服务或托管服务；
- 删除版权、许可证、作者、来源或官方仓库信息。

完整说明见 [品牌与商业使用规则](docs/BRAND_AND_COMMERCIAL_USE.md)。商业分发、OEM、捆绑或托管必须另行取得著作权人的明确书面授权。Issue、Fork、Pull Request、下载、私信或未收到回复都不代表已经获得授权。

此许可证属于“源码公开 / source-available”，不是 OSI 认可的开源许可证。

FFmpeg、Tauri、React、source2-demo 等第三方组件仍适用各自许可证，详见 [第三方组件说明](docs/THIRD_PARTY.md) 和 [第三方声明](THIRD_PARTY_NOTICES.md)。

发布前的源码、依赖、协议参考和图片来源核查记录见 [源码与许可证审计](docs/SOURCE_AND_LICENSE_AUDIT.md)；猫咪主图的来源和哈希见 [资产来源记录](docs/ASSET_PROVENANCE.md)。

Dota、Dota 2、Steam 及相关标志是 Valve Corporation 的商标。本项目是非官方社区工具，与 Valve Corporation 无隶属、赞助或认可关系；仓库不分发 Dota 2 客户端或 Valve 游戏资产。
