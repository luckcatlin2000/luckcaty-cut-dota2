# 贡献指南

感谢你帮助改进猫猫的剪辑小助手。

## 开始前

1. 阅读 `README.md` 和 `docs/REPLAY_CONTROL.md`。
2. 不要提交 DEM、成片、任务缓存、FFmpeg 二进制或安装包。
3. 测试数据必须匿名，不包含真实 SteamID、玩家姓名或可追溯的本机路径。
4. 不要提交、宣传或链接与“猫猫的剪辑小助手”无关的软件。
5. 阅读并同意 `CONTRIBUTOR_LICENSE_AGREEMENT.md`。

## 许可证与贡献授权

本项目是源码公开项目，不采用 MIT 等允许任意销售和再分发的许可证。Fork 仅可用于学习、个人或内部修改，以及向官方仓库提交贡献；不得发布安装包、改名换皮、套壳分发或收费提供。

每个 Pull Request 的描述必须包含：

```text
I have read and agree to CONTRIBUTOR_LICENSE_AGREEMENT.md.
```

缺少该声明的代码贡献不会合并。提交贡献不代表贡献者获得销售、套壳、商业分发或使用项目品牌的授权。

## 本地验证

```powershell
cd .\apps\d2-highlights-desktop
npm ci
cd ..\..
.\scripts\verify.ps1
```

提交应保持范围单一，并说明：

- 修改了什么；
- 为什么需要修改；
- 运行了哪些验证；
- 是否改变 DEM schema、回放命令白名单或发布依赖。

涉及回放控制、进程管理、FFmpeg 参数或输出清理的改动，需要增加失败路径测试。
