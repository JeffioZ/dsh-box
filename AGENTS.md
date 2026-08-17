# AGENTS.md —— DSHDesktop 项目记忆

本文件是给 AI 编码代理的常驻项目说明，每次会话开始时加载。保持简洁、可执行；代码才是真相，这里只写"代码里看不出来的约定与边界"。

## 项目是什么

DSHDesktop（`dsh-desktop`，v0.1.x）是 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的**跨平台桌面外壳**（Tauri v2 + Rust，前端为无打包器的原生 JS/HTML）。

**核心定位（不要破坏）**：
- 薄外壳：加载官方 `dsh web` 界面，**不改 dsh 内核、不 fork 上游、不 patch dsh 包**。
- 只通过三条合法通道与 dsh 交互：① 注入 JS 到 dsh 页面（`MENU_INJECT` / `PAGE_INIT_SCRIPT`）；② 读写 `$DSH_HOME` 下的数据文件（只读会话日志、读写 `settings.yaml` 的 `locale`/`ui-theme` 段）；③ 调用 `dsh` CLI（`dsh web`、`dsh plugin ...`）。
- 任何需要修改 dsh 源码/包/会话格式的"功能"一律不做，先讨论替代方案。

## 目录与职责

- `ui/`：内置页面（启动页 `index.html`+`app.js`、标题栏 `titlebar.*`、托盘菜单 `tray-menu.html`、统一弹窗 `dialog.html`、共享 `common.js`/`common.css`）。无打包器，`<script src>` 直接引用。
- `src-tauri/src/`：Rust 外壳，按职责分层（见 README「项目结构」）。命令层 `commands.rs` 只做 IPC 校验与转发。
- `scripts/`：构建与图标脚本。`build.ps1` / `dev-build.ps1` / `dev-run.ps1` 在仓库根。
- `.github/workflows/`：三平台 CI（格式检查、单测、Clippy、release 构建）。

## 常用命令

```powershell
pwsh -NoLogo -NoProfile -File .\build.ps1      # 正式构建 → dist\DSHDesktop.exe
pwsh -File .\dev-build.ps1                      # 开发版（UI 免编译嵌入）
pwsh -File .\dev-run.ps1                        # 启动开发版 + UI 静态服务器(4321)
npm run icons                                   # 图标改动后必须重新生成
pwsh -File scripts\bump-version.ps1             # 发布前递增版本号
cargo test --manifest-path src-tauri/Cargo.toml # 单测
```

开发模式只改 `ui/` 无需重编译；改 Rust 需重新 `dev-build.ps1`。构建脚本兼容 Windows PowerShell 5.1，推荐 pwsh 7。

## 关键架构事实（改代码前必读）

- **配置**：数据目录（默认 `%LOCALAPPDATA%\DSHDesktop`，可用 `DSH_DESKTOP_ROOT` 覆盖）下 `config.json`：`port` / `api_key` / `api_base` / `language`。环境变量（`DSH_DESKTOP_*`、`DSHD_LANG`）优先于 config.json。见 `app_state.rs::Config::load`。
- **语言与主题**：优先读 dsh 的 `$DSH_HOME/settings.yaml`（`locale.preference`、`ui-theme.preference`），写操作只做行级合并且**必须走 `atomic_write`**（崩溃不截断；dsh 有文件监视器会热发布）。`tray::start_follow_dsh_settings` 每 3s 检查一次该文件 mtime，文件变化才跟随（无变化零解析开销）。
- **注入脚本**：`PAGE_INIT_SCRIPT`（深色主题首帧预设）+ `locale::init_script()` 在窗口创建时作为 `initialization_script`；`MENU_INJECT` 是导航到 dsh 页面后 `eval` 的右键菜单注入（**对外部 URL 导航，initialization_script 不可靠，必须走 navigate 后的 eval**——见 `lib.rs` 的 `on_navigation` 观察器）。
- **更新事务**：`update_txn.rs` 提供"备份 + 标记 + 中断恢复"原语；`updater.rs` 的 dsh/Node 更新与回滚都基于它。新增任何"替换文件"类操作（如 exe 自更新）必须复用该事务模式。
- **进程树**：`processes.rs` 用 Windows Job / Unix 进程组守卫，退出时清理全部子进程，不留孤儿。
- **启动流程**：`dsh.rs::boot_loop` → `boot_once`（Node/dsh 就绪 → 静默更新检查 `silent_check` → 启动 `dsh web` → `wait_ready` → 通知前端 `dsh-status` ready → `navigate`）。看门狗 `watchdog` 负责服务异常恢复。**新增"启动页步骤"（如首次配置）应插入 `boot_once` 的状态机，而不是另起线程抢状态。**
- **主题跟随窗口**：主窗口/弹窗/托盘菜单首次创建与切换共用 `Config::resolve_dsh_theme`。
- **单实例**：`tauri-plugin-single-instance`；重复启动把参数转给已运行实例。

## 约定

- 提交信息用 Conventional Commits；发布版本的变更摘要写在 commit message 的 `chore: release x.y.z——…` 里（没有独立 CHANGELOG.md）。
- **提交前必查**：`cargo fmt`（及 `cargo clippy --all-targets -- -D warnings`、`cargo test --lib`）。CI 的 Format/Clippy 步骤以 `-D warnings` 拦截任何格式与 lint 问题；**本地只验证了当前平台，非 Windows 平台的 cfg 门控死代码（如 `#[cfg(windows)]` 字段）只有全平台 CI 才能抓到**——功能改动尽量走 PR，让 CI 在合并前跑全平台。
- **发布 = 打 tag**：版本号三处同步（`src-tauri/Cargo.toml`、`package.json`、`src-tauri/tauri.conf.json`，CI 有 version-check 校验）→ 提交 `chore: release x.y.z——…` → 打 `vx.y.z` tag 推送。CI 自动构建五平台（windows-x64/macos-arm64/macos-x64/linux-x64/linux-arm64）并发布 release，**不要手动传本地产物**（产物名与平台数量由 CI 保证）。
- 文案中英双语：`common.js` 的 `DSHD_MESSAGES` 表 + `data-i18n` 属性；新文案必须双语都加。
- **UEUI 工作流（强制）**：任何涉及视觉样式、交互体验、布局/间距/尺寸、动效、配色、字体字号、文案描述的改动，**必须先调用 `ui-ux-pro-max` skill**（本地 `~/.agents/skills/ui-ux-pro-max/scripts/search.py` 按对应 domain 查询设计规则），取得规则依据后再实施；提交信息与文档中不提及该 skill。
- 每个 Rust 文件一个职责，注释只写"为什么"；遵循现有分层（命令层不写业务实现）。
- 新增功能要有对应单测（纯逻辑放 `versions.rs` 等可测模块）。
- 修改 UI 后检查深浅色两套样式（`@media (prefers-color-scheme: light)`）与 `prefers-reduced-motion`。
- 不用 `docker`/`make`；CI 用 GitHub Actions。
