# DSHDesktop

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的跨平台桌面外壳，基于 [Tauri v2](https://tauri.app)。主界面直接加载官方 `dsh web`，并在外面套一层桌面体验：标题栏、托盘、一键更新与本地文件菜单。

## 目录

- [功能](#功能)
- [平台支持](#平台支持)
- [快速开始](#快速开始)
- [配置](#配置)
- [构建](#构建)
- [项目结构](#项目结构)
- [边界与规划](#边界与规划)
- [许可](#许可)

## 功能

**核心能力**

- Windows 单文件分发：单个 exe，无控制台窗口，双击即用
- 自动准备运行环境：Windows 缺少 WebView2 时引导安装；各平台按需安装 Node.js 与 dsh
- 一键更新 dsh 与 Node.js（失败自动回滚；更新被打断时，下次启动自动还原）
- 单实例、服务自动恢复、端口冲突自动回退

**桌面体验**

- 托盘常驻；标题栏共用主菜单与实时余额（官方 `/user/balance` 接口，每 5 分钟自动刷新，可手动刷新）
- 窗口位置记忆、深浅色自适应、Win11 系统圆角
- 中英文界面：英文系统显示英文，其他系统显示中文（与 dsh 的产品默认语言一致）；可从标题栏/托盘菜单切换并记住选择
- 自绘右键菜单（dsh 页面内）、托盘菜单与弹窗，风格与 dsh 界面统一

## 平台支持

| 平台 | 架构 | 最低系统版本 | 状态 |
|---|---|---|---|
| Windows | x64 | Windows 10 及以上 | 支持；主要测试平台 |
| macOS | arm64 / x64 | macOS 13.5 及以上 | 支持；CI 产物未签名 |
| Linux | x64 / arm64 | Ubuntu 22.04 / Debian 12 及等价发行版（WebKitGTK 4.1） | 支持 |

最低版本取各底层依赖中最严格的一个（dsh 本身是纯 Node.js 工具，三平台通用）：

- **Windows 10**：Node.js 22 的最低支持版本；WebView2 支持 1803+（Win11 预装，较老系统由外壳引导安装 Runtime）
- **macOS 13.5**：外壳自动安装的 Node.js 24 LTS 要求 macOS 13.5+
- **Ubuntu 22.04 / Debian 12**：Tauri v2 依赖 WebKitGTK 4.1（高于 Node 的 glibc 2.28 要求）

Linux 上 dsh 的 Landlock 沙箱（文件系统隔离）需要内核 5.13+；不满足时 dsh 自动降级，不影响 Web 主界面运行。

## 快速开始

从 [Releases](https://github.com/JeffioZ/dsh-desktop/releases) 下载对应平台的产物：

- Windows：直接运行 `DSHDesktop.exe`
- macOS / Linux：解压 `.zip` 后运行 `DSHDesktop`

各平台产物以最新 Release 附件为准，GitHub Actions 也会产出各平台的构建产物。所有产物均未签名，系统安全策略可能要求手动允许。

首次运行会自动补齐运行环境——按需安装 Node.js 与 dsh 包——随后启动 `dsh web`（默认端口 3080）。Linux 还需先安装发行版提供的 WebKitGTK 等 Tauri 系统依赖。

## 配置

### 数据目录

| 平台 | 路径 |
|---|---|
| Windows | `%LOCALAPPDATA%\DSHDesktop` |
| macOS | `~/Library/Application Support/com.deepseek.dsh-desktop` |
| Linux | `$XDG_DATA_HOME/com.deepseek.dsh-desktop`（未设置 `XDG_DATA_HOME` 时为 `~/.local/share/com.deepseek.dsh-desktop`） |

该目录下会生成 `node/`、`dsh/`、`npm-cache/` 与 `logs/`（`desktop.log`，时间为 UTC，超过 2 MB 自动轮转为 `.old`）。更新 dsh 或 Node 期间会短暂出现 `dsh-old/` 或 `node-old/` 备份目录，更新成功后自动清理。

### config.json

数据目录下的可选配置文件：

```json
{
  "port": 3080,
  "api_key": "sk-...",
  "api_base": "https://api.deepseek.com",
  "language": "zh-CN",
  "hide_tool_calls": false
}
```

### 环境变量

环境变量优先于 `config.json`：

| 变量 | 作用 |
|---|---|
| `DSH_DESKTOP_ROOT` | 覆盖数据根目录 |
| `DSH_DESKTOP_PORT` | 覆盖监听端口 |
| `DSH_DESKTOP_DSH_HOME` | 覆盖 dsh 主目录（`DSH_HOME`） |
| `DSH_DESKTOP_API_KEY` | 覆盖 API Key |
| `DSH_DESKTOP_API_BASE` | 覆盖 API 基地址 |
| `DSHD_LANG` | 固定界面语言（`zh-CN` / `en`，重启后生效，优先级最高） |

### 便携模式

把 `DSHDesktop.exe` 所在目录放一个空的 `portable.txt` 文件，数据目录（`node/`、`dsh/`、`logs/`、`config.json`）即跟随 exe 存放在旁边的 `data/` 目录，拷贝到 U 盘即可随身携带。删除 `portable.txt` 即恢复常规模式。`DSH_DESKTOP_ROOT` 环境变量仍优先于便携模式。

### API Key

解析顺序：`DSH_DESKTOP_API_KEY` → `config.json` → `DEEPSEEK_API_KEY` → dsh 凭据文件（`$DSH_HOME/.credentials.yaml`，未设置 `DSH_HOME` 时为 `~/.dsh/.credentials.yaml`）。

> `config.json` 中的 API Key 以明文保存在当前用户的数据目录内，请勿提交到仓库或发送给他人。

## 构建

### 前置要求

- Rust stable
- Windows 另需 VS2022 C++ 工具链与 Node.js（用于复制产物）
- Linux 另需 WebKitGTK 系统包（`libwebkit2gtk-4.1-dev`、`libgtk-3-dev`、`libayatana-appindicator3-dev` 等）

### 正式构建

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

`build.ps1` 与 `scripts/cargo.ps1 build` 只编译、不修改版本号。发布前递增补丁版本请显式运行 `pwsh -File scripts/bump-version.ps1`。

### 开发模式（改 UI 免编译）

```powershell
pwsh -File .\dev-build.ps1   # 首次：构建不嵌入资源的开发版（dist-dev\DSHDesktop-dev.exe）
pwsh -File .\dev-run.ps1     # 运行：自动启动 UI 静态服务器(4321) + 开发版 exe
# 之后只改 ui/ 下的文件，重启 dev-run.ps1（或刷新页面）即生效；改 Rust 代码需重新 dev-build
```

### CI

GitHub Actions 负责三平台格式检查、单测、Clippy、release 构建与产物上传。

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
    window.rs           # 窗口位置记忆与系统协商补偿、DPI 图标、Win11 圆角
    balance.rs          # API 余额
    tray.rs             # 托盘图标与动作
    autostart.rs        # 开机自启动（三分支）
  scripts/              # 构建与图标生成脚本（icon-codecs.mjs 为图标编解码）
  .github/workflows/    # 三平台 CI
```

核心逻辑按职责拆分：`runtime` 负责运行环境，`dsh` 负责服务生命周期，`updater`/`update_txn` 负责更新与恢复，命令层只做 IPC 校验与转发。

## 边界与规划

DSHDesktop 是 dsh 的**桌面封装**，而不是另一套实现：

- **不改动 dsh**：主界面直接加载官方 `dsh web`，dsh 照常独立升级，外壳随之跟进
- **只做桌面层**：托盘、标题栏、系统通知、更新、本地文件菜单等桌面体验；不重复实现 dsh 的对话与会话能力，也不另存一份会话数据
- **交互克制**：仅通过 dsh 官方提供的页面、数据文件与 CLI 与之协作，不依赖任何未公开的内部接口

更多设计取舍见 [docs/why-desktop.md](docs/why-desktop.md)。

规划中的能力（尚未交付）：

- 手机远程控制：在手机浏览器/App 中继续本机会话
- IM 通道：在微信/飞书等聊天工具中向 Agent 发起任务

## 许可

[MIT](LICENSE)。第三方依赖遵循其各自许可；dsh 本体使用官方 npm 包原样安装，不影响其官方升级。

## 致谢

感谢 DeepSeek Harness 团队与开源社区，以及每一位参与测试与反馈的用户。
