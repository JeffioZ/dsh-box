# DSHDesktop

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的跨平台桌面外壳，基于 [Tauri v2](https://tauri.app)。主界面使用官方 `dsh web`，并补充桌面标题栏、托盘、更新与本地文件菜单。

## 功能

- Windows 单文件：单个 exe，无控制台窗口，双击直接启动
- 自动准备运行环境：Windows 缺少 WebView2 时引导安装；各平台在需要时安装 Node.js 与 dsh
- 托盘常驻；标题栏提供共用的主菜单（标题栏已有余额入口，因此主菜单不重复显示余额）
- 一键更新 dsh 与 Node.js（事务化：先备份后替换，失败自动回滚；更新在提交阶段被断电/强杀打断时，下次启动自动还原旧版本）
- 标题栏实时显示 DeepSeek API 余额（官方 `/user/balance` 接口，每 5 分钟自动刷新，也可手动刷新）
- 单实例、服务自动恢复、端口冲突自动回退、窗口位置记忆、深浅色自适应
- 中英文界面：英文系统显示英文，其他系统显示中文（与 dsh 的产品默认语言一致）；也可从标题栏/托盘菜单切换并记住选择
- 自绘右键菜单（dsh 页面内）与自绘托盘菜单/弹窗，风格与 dsh 界面统一

## 平台支持

| 平台 | 架构 | 状态 |
|---|---|---|
| Windows 10/11 | x64 | 支持；主要测试平台 |
| macOS | arm64 / x64 | 支持；CI 产物未签名 |
| Linux | x64 | 支持；需系统已安装 WebKitGTK 等依赖 |

## 使用

从成功的 GitHub Actions 构建下载对应平台的产物。Windows 直接运行 `DSHDesktop.exe`；macOS/Linux 解压 `.tar.gz` 后运行 `DSHDesktop`。这些是未签名的开发/测试产物，系统安全策略可能要求手动允许。

首次运行会自动安装 Node.js（缺失或版本不满足要求时）与 dsh 包，随后启动 `dsh web`（默认端口 3080）。Linux 仍需先安装发行版提供的 WebKitGTK 等 Tauri 系统依赖。

数据目录：

- Windows：`%LOCALAPPDATA%\DSHDesktop`
- macOS：`~/Library/Application Support/com.deepseek.dsh-desktop`
- Linux：`$XDG_DATA_HOME/com.deepseek.dsh-desktop`；未设置 `XDG_DATA_HOME` 时为 `~/.local/share/com.deepseek.dsh-desktop`

该目录下会生成 `node/`、`dsh/`、`npm-cache/` 与 `logs/`（`desktop.log`，时间为 UTC，超过 2 MB 自动轮转为 `.old`）。更新 dsh 或 Node 期间会短暂出现 `dsh-old/` 或 `node-old/` 备份目录，更新成功后自动清理。

可选配置（数据目录下 `config.json`）：

```json
{
  "port": 3080,
  "api_key": "sk-...",
  "api_base": "https://api.deepseek.com",
  "language": "zh-CN"
}
```

也可通过 `DSH_DESKTOP_ROOT`、`DSH_DESKTOP_PORT`、`DSH_DESKTOP_DSH_HOME`、`DSH_DESKTOP_API_KEY` 和 `DSH_DESKTOP_API_BASE` 覆盖对应运行配置；环境变量优先于 `config.json`。界面语言默认跟随系统，可从标题栏或托盘菜单切换（在 `config.json` 中保存为 `"language": "zh-CN"` 或 `"en"`）；`DSHD_LANG=zh-CN` 或 `DSHD_LANG=en` 可用于固定语言（重启应用后生效，并优先于配置文件）。

API Key 解析顺序：`DSH_DESKTOP_API_KEY` → `config.json` → `DEEPSEEK_API_KEY` → dsh 凭据文件（`$DSH_HOME/.credentials.yaml`，未设置 `DSH_HOME` 时为 `~/.dsh/.credentials.yaml`）。

`config.json` 中的 API Key 以明文保存在当前用户的数据目录内，请勿提交到仓库或发送给他人。

## 构建

前置：Rust stable；Windows 另需 VS2022 C++ 工具链和 Node.js（用于复制产物），Linux 另需 WebKitGTK 系统包（`libwebkit2gtk-4.1-dev`、`libgtk-3-dev`、`libayatana-appindicator3-dev` 等）。

```powershell
# 一键构建（Windows）
pwsh -NoLogo -NoProfile -File .\build.ps1
# 输出：dist\DSHDesktop.exe

# 分步
npm install
npm run icons                # 生成图标（首次）
pwsh -File scripts\cargo.ps1 build
node scripts\copy-exe.mjs
```

开发模式（改 UI 免编译）：
```powershell
pwsh -File .\dev-build.ps1   # 首次：构建不嵌入资源的开发版（dist-dev\DSHDesktop-dev.exe）
pwsh -File .\dev-run.ps1     # 运行：自动启动 UI 静态服务器(4321) + 开发版 exe
# 之后只改 ui/ 下的文件，重启 dev-run.ps1（或刷新页面）即生效；改 Rust 代码需重新 dev-build
```

`build.ps1` 和 `scripts/cargo.ps1 build` 只编译，不修改版本号。发布前需要递增补丁版本时，显式运行 `pwsh -File scripts/bump-version.ps1`。

CI（GitHub Actions）负责三平台格式检查、单测、Clippy、release 构建与产物上传；工作流能否启动仍取决于仓库的 Actions 额度和账单状态。

## 项目结构

```
desktop/
  ui/                   # 启动页、标题栏、托盘菜单、统一弹窗（common.css 共享样式）
  src-tauri/src/        # Rust 外壳（按职责分层）
    main.rs             # 入口；Windows 含 WebView2 自动安装预检
    lib.rs              # run() 组装、状态广播、导航、右键菜单注入脚本、自定义协议
    app_state.rs        # 共享状态：配置、引导阶段、生命周期锁、弹窗轮询数据
    commands.rs         # Tauri 命令层（IPC 转发，无业务实现）
    app_dialog.rs       # 统一自绘弹窗（余额 / 检查更新 / 关于）
    dialog.rs           # 原生消息框封装（模态、互斥）
    file_actions.rs     # 本地文件动作（默认程序打开 / 定位 / 打开方式）
    icons.rs            # 图标提取（SHGetFileInfo → PNG，含缓存）
    logging.rs          # 日志（轮转）
    locale.rs           # 系统语言检测与中英文选择
    titlebar.rs         # 自绘标题栏（子 webview）
    tray_menu.rs        # 标题栏/托盘共用菜单模型；Windows 自绘托盘菜单窗口
    runtime.rs          # Node 检测/安装、dsh 包安装、服务启动
    versions.rs         # 版本比较、Node 最低版本判定（纯逻辑）
    update_txn.rs       # 更新事务原语：备份 + 标记 + 中断恢复
    updater.rs          # 一键更新（dsh / Node）
    dsh.rs              # 服务生命周期：引导主循环、看门狗、退出清理
    processes.rs        # 进程管理：树守卫（Windows Job / Unix 进程组）
    util.rs             # 文本截断等小工具
    window.rs           # 窗口位置记忆、DPI 图标
    balance.rs          # API 余额
    tray.rs             # 托盘图标与动作
    autostart.rs        # 开机自启动（三分支）
  scripts/              # 构建与图标生成脚本（icon-codecs.mjs 为图标编解码）
  .github/workflows/    # 三平台 CI
```

核心逻辑按职责拆分：`runtime` 负责运行环境，`dsh` 负责服务生命周期，`updater`/`update_txn` 负责更新与恢复，命令层只做 IPC 校验和转发。平台差异主要集中在 `processes.rs`、`autostart.rs`、`main.rs`、`runtime.rs` 与 `file_actions.rs`。

## 开发约定

- 提交信息使用 Conventional Commits 风格
- 推荐使用 PowerShell 7（pwsh）执行构建脚本；脚本兼容系统自带 Windows PowerShell 5.1
- 图标改动后需 `npm run icons` 并重新构建
- 本项目由 AI（DeepSeek 智能体）辅助开发与维护，改动经人工审阅

## 许可

[MIT](LICENSE)。第三方依赖遵循其各自许可；dsh 本体使用官方 npm 包原样安装，不影响其官方升级。
