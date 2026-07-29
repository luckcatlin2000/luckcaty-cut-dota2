# 源码与许可证审计

审计日期：2026-07-28
目标仓库：<https://github.com/luckcatlin2000/luckcaty-cut-dota2>

## 结论

在本次可执行的静态检查、依赖元数据检查、公开代码特征检索和人工
抽查范围内：

- 未发现被直接复制进仓库的第三方源码目录、Git 子模块、压缩源码包、
  大段版权头、预编译库、外部 EXE 或安装包；
- Rust 与 npm 第三方依赖均通过官方包注册表和锁文件声明，未发现 Git
  来源依赖或缺少许可证元数据的依赖；
- Source 2 VConsole2 包格式曾参考两个 MIT 项目，现已明确列出来源、
  权利人和完整 MIT 声明；
- 猫咪主图由项目权利人确认是 OpenAI Codex / GPT 生成且没有外部
  参考图，仓库图标为该图的尺寸变体；
- 自带 BGM 由 Rust 代码实时合成，没有内嵌音频文件，相关角色标识已
  明确为 `original_comic_hype`；
- 未发现无关软件或其他产品的介绍、宣传、下载地址或发布物。

因此，本仓库目前没有发现阻止源码公开的明确版权或许可证缺口。

## 审计范围与方法

1. 枚举 Git 跟踪文件，检查源码、脚本、文档、图片、二进制和大文件。
2. 搜索 `copyright`、`copied`、`based on`、`inspired`、外部仓库 URL、
   嵌入字节、私钥、Base64 和可疑生成物标记。
3. 对主要 Rust、TypeScript、CSS 和 PowerShell 模块做人工抽查。
4. 对若干项目特有字符串做精确公开检索；未找到相同公开代码结果。
5. 从 Windows 目标的 Cargo 锁定依赖图读取许可证和来源。
6. 从 `package-lock.json` 读取 npm 包来源和许可证。
7. 对猫咪主图做视觉、尺寸、哈希和元数据检查。

精确字符串检索只能发现公开且可索引的相同文本，不能证明不存在任何
相似实现。原始开发目录没有可用于逐行追溯的完整历史提交链，因此本
审计不能提供数学意义上的“绝无借鉴”证明。

## 第一方实现

第一方源码主要位于：

- `apps/d2-highlights-desktop/`
- `apps/d2-highlights-cli/`
- `crates/d2-highlights-*`
- `scripts/`

仓库没有 `vendor/` 或复制进来的第三方源码树。应用的本地回放逻辑只
连接 `127.0.0.1:29000`，使用项目自身启动的 `dota2.exe -insecure
-vconsole -console` 离线回放进程和命令白名单，不注入游戏进程。

## 第三方包依赖

### Rust

Windows 目标的锁定依赖图包含 312 个第三方注册表包：

- 来源均为 Cargo 注册表；
- 没有第三方 Git 或本地路径来源；
- 没有缺少许可证元数据的包；
- 识别到的许可证为 MIT、Apache-2.0、BSD、ISC、MPL-2.0、Unicode、
  Zlib、Unlicense、CC0 和其他宽松或文件级弱 copyleft 条款；
- 未发现 GPL 或 AGPL 包进入 Rust 依赖图。

### npm

`package-lock.json` 包含 91 个第三方包：

- 均来自 `https://registry.npmjs.org/`；
- 没有 Git 或其他外部源码来源；
- 没有缺少许可证元数据的包；
- 识别到 MIT、Apache-2.0、BSD-3-Clause、ISC、MPL-2.0 和 0BSD。

锁文件是实际版本的最终依据。正式发布二进制时仍应为该版本重新生成
完整 notices，因为依赖版本和许可证可能变化。

## Source 2 VConsole2 协议参考

`crates/d2-highlights-replay-control/src/lib.rs` 中的 `build_cmnd_packet`
按公开协议事实构造 12 字节网络序包头、`CMND` 标记、命令正文和 NUL
结尾，并增加本项目自己的命令校验、错误处理与离线安全边界。

核对过的参考实现：

- <https://github.com/theokyr/CS2RemoteConsole> — MIT，
  Copyright (c) 2024 H7perus and theokyr；
- <https://github.com/oxijoined/vconsole-python> — MIT，
  Copyright (c) 2026 oxijoined。

仓库未复制两者的 C++ 或 Python 文件。为了避免“只参考不说明”的
争议，完整 MIT 文本已保守收录到 `THIRD_PARTY_NOTICES.md`。

## FFmpeg

源码仓库不分发 FFmpeg 或 FFprobe 二进制。`tools/ffmpeg/LICENSE.txt`
与 `SOURCE.txt` 只是参考构建的许可证和来源记录，不会把 GPL 自动
施加到本项目第一方源码。

如果以后把某个 FFmpeg 构建放进安装包，必须针对那个确切二进制检查
LGPL/GPL 配置、源码提供、修改、构建参数和 notices；当前仓库不能替
未来的捆绑发布自动完成这些义务。

## Dota 2 与 Valve

仓库使用 Dota 2、Steam、英雄标识和控制台命令来描述兼容对象与实现
本地回放功能，没有分发 Dota 2 客户端、模型、贴图、音频或提取的游戏
资产。`NOTICE.md` 已声明 Valve 商标和非官方关系。

## 图片与音频

- 猫咪图与图标：见 `docs/ASSET_PROVENANCE.md`。
- BGM：`crates/d2-highlights-renderer/src/audio.rs` 通过振荡器、包络、
  确定性噪声和音符频率数组实时写 WAV，没有内嵌第三方歌曲或录音。

## 许可证边界

第一方内容使用 Cat Cut Assistant Source-Available License 1.0。它
禁止未经授权的售卖、二进制分发、套壳、换皮和托管商业化，但允许用户
正常使用软件并变现自己剪辑的视频。

因为限制销售和再分发，该许可证不是 OSI 认可的开源许可证。对外描述
应使用“源码公开”或 “source-available”，不能标注为 MIT、Apache、
GPL 或 OSI Open Source。

第三方组件仍只受各自许可证约束，第一方许可证不能收回第三方许可证
已经授予的权利。

## 每次公开发布前的门禁

- `git status` 只包含预期文件；
- 搜索并确认没有账号、令牌、私钥、本机绝对路径、DEM、成片或任务数据；
- 核对 README、LICENSE、中文说明、NOTICE 和商业使用规则一致；
- 核对没有重新出现 MIT 作为“项目许可证”；
- 更新依赖审计与第三方 notices；
- 运行 Rust 格式、测试、Clippy、前端测试、前端构建和 Tauri 发布构建；
- 确认 GitHub About、Topics、Issues、Releases 和附件只介绍本软件；
- 若新增图片、音频、字体、代码片段或数据集，先记录来源、作者、链接、
  许可证、修改内容和允许的分发方式。
