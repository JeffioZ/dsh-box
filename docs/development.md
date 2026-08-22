# 开发指南

## 环境

- Rust 1.85 或更高版本。
- Node.js 与 npm；Node 仅用于项目检查、图标和构建辅助，不参与 UI 打包。
- 对应平台的 Tauri v2 系统依赖。Windows 需要 Visual Studio 2022 C++ 工具链，Linux 需要 WebKitGTK 4.1 开发包。
- Windows 命令推荐 PowerShell 7（`pwsh`）。

## 日常命令

```powershell
npm install
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

```powershell
# 正式 Windows 构建 → dist\DSHBox.exe
pwsh -NoLogo -NoProfile -File .\build.ps1

# UI 热更新开发模式
pwsh -NoLogo -NoProfile -File .\dev-build.ps1
pwsh -NoLogo -NoProfile -File .\dev-run.ps1
```

`dev-run.ps1` 在 4321 端口提供 `ui/`；只改 UI 可刷新页面，改 Rust 后需重新执行 `dev-build.ps1`。开发版使用独立的 `src-tauri/target/dev/` 构建缓存，不会覆盖或锁住正式版产物。

## 修改规则

- 命令层只校验 IPC 来源与参数，然后调用业务模块。
- 新增替换文件/目录的更新流程必须复用更新事务。
- 写 `settings.yaml` 或 `.credentials.yaml` 必须通过 `update_text_file`；不要直接覆盖整个用户文件。
- UI 新文案同时添加中英文，并在所有主题与 `prefers-reduced-motion` 下检查。
- 右键菜单涉及本地文件高频操作。除非有明确产品理由和回归验证，不删除项目、改变顺序或增加日常确认弹窗。
- 新增纯逻辑必须有单测；跨平台 `cfg` 分支最终以 CI 为准。

## 图标

唯一品牌源是 `assets/brand/deepseek-mark.svg`。修改后运行：

```powershell
npm run icons
```

生成物包括 Tauri PNG/ICO/ICNS、托盘图标和 `ui/assets/app-icon.svg`。不要单独手改某个生成图标。

## 版本与发布

1. 更新 `RELEASE_NOTES.md`。
2. 执行 `pwsh -File scripts/bump-version.ps1`，确认 `Cargo.toml`、`Cargo.lock`、`package.json` 和 `tauri.conf.json` 一致。
3. 运行全部本地检查并提交 `chore: release x.y.z——…`。
4. 创建并推送 `vx.y.z` tag。
5. GitHub Actions 先创建 draft Release、构建五个平台、校验附件与 digest，全部成功后发布。

不要手工上传本地产物替代 CI 产物。当前本地验证不能覆盖所有目标平台，功能变更应通过 PR 等待完整矩阵。

## 故障定位

- 应用日志：数据根目录 `logs/dshbox.log`。
- dsh 服务日志：同目录 `logs/dsh.log`。
- `npm run check` 失败时先处理其给出的具体文件；该脚本不会抽样。
- 更新失败时不要手工删除 `*-old`、`.part` 或事务标记，先让下一次启动执行恢复。
