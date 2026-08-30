# 开发指南

## 环境

- Rust 1.85 或更高版本。
- Node.js `^22.19.0` 或 `>=24.0.0` 与 npm；Node 仅用于项目检查、图标和构建辅助，不参与 UI 打包。
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

`dev-run.ps1` 在 4321 端口提供 `ui/`；只改 UI 可刷新页面，改 Rust 后需重新执行 `dev-build.ps1`。开发版使用独立的 `src-tauri/target/dev/` 构建缓存，不会覆盖或锁住正式版产物。UI 服务器在脚本退出后仍常驻，PID 记录在 `%TEMP%\dshbox-dev-ui-server.pid`，可用 `Stop-Process -Id <PID>` 停止（重复运行 `dev-run.ps1` 时也会提示）。
`build.ps1` 与 `dev-build.ps1` 会先运行完整项目一致性检查，避免把 UI 语法、资源引用或配置错误编进产物。

## CI 与跨平台检查

GitHub Actions 在 Windows x64、macOS arm64、Linux x64/arm64 原生任务上运行 Clippy 与单测；macOS x64 另做交叉编译检查和 Clippy。格式检查与项目一致性在单独的 `version-check` 任务只跑一次（与平台无关）。`main` push 与 PR 只做验证；只有格式为 `vX.Y.Z` 且与项目版本一致的 tag 才执行 release 构建并上传附件。

本地 Windows 编译不会检查 Unix 专属分支。修改 `#[cfg(windows)]`、`#[cfg(target_os = "macos")]` 或 Unix 代码时，同时检查以下内容：

- 专属函数的 `use` 也使用相同 `cfg`，避免其他平台出现未解析导入。
- 共享模块不要无条件引用只在单个平台定义的符号。
- 新增平台分支后让 PR 完整跑完 CI 矩阵，再合并或打 tag。

如果某个平台在“单测”阶段编译失败，先处理第一条 Rust 编译错误。后续矩阵错误通常是同一根因的重复结果。

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
2. 执行 `pwsh -File scripts/bump-version.ps1`，确认 `Cargo.toml`、`Cargo.lock`、`package.json`、`package-lock.json` 和 `tauri.conf.json` 一致。
3. 运行全部本地检查并提交 `chore: release x.y.z——…`。
4. 创建并推送 `vx.y.z` tag。
5. GitHub Actions 先创建 draft Release、构建五个平台、校验附件与 digest，全部成功后发布。

不要手工上传本地产物替代 CI 产物。当前本地验证不能覆盖所有目标平台，功能变更应通过 PR 等待完整矩阵。

发布前再确认 tag 为严格的 `vX.Y.Z`，并与 `Cargo.toml`、`package.json`、`tauri.conf.json` 和两份锁文件一致。普通 `main` push 不生成发布附件。

## 故障定位

- 应用日志：数据根目录 `logs/dshbox.log`。
- dsh 服务日志：同目录 `logs/dsh.log`。
- `npm run check` 失败时先处理其给出的具体文件；该脚本不会抽样。
- 更新失败时不要手工删除 `*-old`、`.part` 或事务标记，先让下一次启动执行恢复。

常见用户侧问题与日志位置见[故障排查](troubleshooting.md)。「用量与余额」模块与上游 dsh-usage-stats 的同步规程见[用量同步](usage-sync.md)。

## 用量与余额假数据（dev 专用）

`pwsh -File .\dev-run.ps1 -FakeUsage`（或自设环境变量 `DSH_BOX_FAKE_USAGE=1` 后启动）注入确定性假数据，不配任何凭据/订阅即可看全显示分支：

- **余额/订阅卡**：ok（CNY/USD、含赠送/充值明细行）、unlimited（∞）、预警两档（warning/critical）、not-configured / unauthorized / unsupported（含本地 Ollama 门控文案）、窗口型卡片（quota/session/weekly + 重置时间）、stale 旧快照标记、当前会话徽标。
- **用量报告**：今日/本月/累计 token 与缓存命中率、月历热力图（含上一月导航）、最近 14 天、按模型下钻；成本列走真实 `render` 与定价引擎——当月各天为官方 DeepSeek（显示金额），上一月留一天自定义路由（该日与「累计」显示「—」，演示未定价 fail-closed 语义）。
- **状态栏**（需在设置中开启"隐藏 dsh 统计行"）：五组统计与 tooltip 明细走真实组装路径；实时速率为 18–52 tok/s 三角波，可见动态变化。

边界：假数据**零网络请求、不写聚合缓存**，报告经 `aggregate::render` 现算、成本按 `pricing.rs` 实价估算——展示链路与真实数据完全同一条。环境变量未设置时代码完全休眠，生产不受影响。实现见 `src-tauri/src/usage/dev_fake.rs`。
