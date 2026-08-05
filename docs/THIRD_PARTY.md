# 第三方组件

本项目自有源码采用根目录的 Cat Cut Assistant Source-Available License 1.0。第三方组件不受该许可证重新许可，仍适用各自条款；锁文件是实际依赖版本的最终依据。

## 主要运行时组件

| 组件 | 许可证 | 用途 | 仓库分发状态 |
|---|---|---|---|
| source2-demo | MIT OR Apache-2.0 | Dota 2 Source 2 DEM 解析 | Cargo 依赖，不内嵌源码 |
| Tauri | MIT OR Apache-2.0 | Windows 桌面外壳与 IPC | Cargo/npm 依赖 |
| React / React DOM | MIT | 桌面 UI | npm 依赖 |
| Lucide React | ISC | UI 图标 | npm 依赖 |
| FFmpeg / FFprobe | LGPL 或 GPL，取决于具体构建 | 编码、拼接、媒体探测和 QC | 源码仓库不分发二进制；官方 1.8.0 及后续安装包包含已记录的参考构建 |

前端构建链还包含 MPL-2.0、Apache-2.0、MIT、ISC、BSD 等许可证的软件包。发布者应从 `Cargo.lock` 和 `package-lock.json` 为每个正式版本生成完整的第三方 notices。

## FFmpeg 发布边界

源码仓库不跟踪 FFmpeg/FFprobe 二进制。开发者可以使用系统提供的工具；官方 1.8.0 及后续安装包使用 `tools/ffmpeg/SOURCE.txt` 标明的参考构建，并随安装资源提供 GPLv3 许可证和来源记录。

维护者如果发布捆绑 FFmpeg 的安装包，必须针对实际二进制重新核验：

- 构建是否启用了 GPL 组件；
- 对应源码是否与二进制完全一致；
- 构建配置和修改是否可获得；
- 许可证、版权声明和源码下载位置是否随发布物提供；
- 下载页面和应用内说明是否与实际分发一致。

`tools/ffmpeg/SOURCE.txt` 描述官方 1.8.0 及后续版本使用的参考构建。后续版本如更换二进制，必须同步更新该记录和正式发布的逐版本合规材料。

## 协议实现参考

- [`theokyr/CS2RemoteConsole`](https://github.com/theokyr/CS2RemoteConsole)：MIT，Copyright (c) 2024 H7perus and theokyr。用于交叉核对 Source 2 VConsole2 的 12 字节包头、`CMND` 标记、网络字节序和命令结尾 NUL；不作为运行时依赖，仓库未复制其 C++ 源文件。
- [`oxijoined/vconsole-python`](https://github.com/oxijoined/vconsole-python)：MIT，Copyright (c) 2026 oxijoined。用于再次核对 `CMND` 版本值 `0x00D40000`、长度字段和测试向量；不作为运行时依赖，仓库未复制其 Python 源文件。

本项目的 Rust 实现位于 `crates/d2-highlights-replay-control/src/lib.rs`，另行加入了命令白名单、换行/NUL 拒绝、长度错误和 localhost 离线回放约束。上述参考项目的 MIT 声明全文见根目录 [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md)。
