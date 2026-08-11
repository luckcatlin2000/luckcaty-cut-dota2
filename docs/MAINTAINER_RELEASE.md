# 维护者发布流程

## 源码门禁

```powershell
.\scripts\verify.ps1
```

确认 Git 中不存在 DEM、媒体、任务缓存、二进制、真实比赛编号、玩家标识、本机绝对路径、密钥或内部规划记录。公开仓库必须继续保留根目录许可证、品牌规则和第三方声明。

## 签名候选

正式安装包使用被 Git 忽略的 `tools\ffmpeg\bin\` 和 `.release-secrets\`。签名目录包含私钥、公钥与当前 Windows 用户可解密的密码文件，不得复制到候选目录或 Git。

```powershell
.\scripts\desktop-release.ps1
```

隔离工作树可以显式引用维护者主工作树的签名目录：

```powershell
.\scripts\desktop-release.ps1 -SigningRootOverride "[PROJECT_ROOT]\.release-secrets"
```

脚本先执行完整验证，再生成 `release\candidate\<版本>\`，其中包括便携版、安装包、更新安装包、签名、`latest.json` 和绑定源码提交的候选清单。候选不会自动覆盖根目录正式版。

## 正式提升

候选完成冷启动和适用的真实录像回归后，确认 `main`、`v<版本>` 标签和候选清单指向同一提交，再执行：

```powershell
.\scripts\promote-release.ps1 -Version <版本> -ConfirmPromotion
```

提升脚本会再次验证哈希与源码，把上一版 EXE 保存到 `release\history\`，再原子替换根目录启动文件和正式安装包。

## GitHub Release

GitHub Actions 只验证源码，不持有更新签名私钥，也不重建已经验收的安装包。维护者在推送前必须再次确认官方仓库、`main`、标签和以下附件：

- `latest.json`
- `luckcaty-cut-dota2_<版本>_x64-setup.exe`
- `luckcaty-cut-dota2_<版本>_x64-setup.exe.sig`
- 可选的便携版与面向用户的安装包

Release 说明包含功能变化、SHA-256、FFmpeg 许可证与来源链接，不把静态作者信息写成更新项。

客户端更新策略固定为只检查和提示；用户点击后才下载、验签与安装，分析或导出期间禁止安装。只有著作权人或持有明确书面授权的发布者可以对外发布安装包。

## 缓存失效边界

渲染器的 `CAPTURE_PIPELINE_VERSION` 参与片段缓存指纹。修改 Dota 2 HUD、镜头、
`startmovie/endmovie` 或帧/音频编码行为时必须同步升级该值，并运行缓存指纹回归测试；
纯 UI 或文案调整不需要清空片段缓存。
