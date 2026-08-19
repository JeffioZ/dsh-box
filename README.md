# DSHBox

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的跨平台桌面外壳，基于 [Tauri v2](https://tauri.app)。主界面加载官方 `dsh web`，并提供标题栏、状态栏、托盘、更新与本地文件菜单等桌面能力。

<p align="center">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-4d6bfe" />
  <img alt="Platform" src="https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-4d6bfe" />
  <img alt="Runtime" src="https://img.shields.io/badge/dsh-latest-4d6bfe" />
  <img alt="Stack" src="https://img.shields.io/badge/Tauri%20v2%20%7C%20Rust-4d6bfe" />
</p>

## 目录

- [功能](#功能)
- [平台支持](#平台支持)
- [快速开始](#快速开始)
- [配置](#配置)
- [构建](#构建)
- [项目结构](#项目结构)
- [边界与规划](#边界与规划)
- [许可](#许可)
- [内置插件市场](#内置插件市场)

## 功能

| 能力 | 说明 |
|---|---|
| **单文件分发** | Windows 单个 exe、无控制台窗口，双击即用；macOS dmg 拖入 Applications；Linux 解压即用 |
| **零依赖准备** | 自动检测并安装 Node.js 与 dsh；Windows 缺少 WebView2 时引导安装 |
| **一键更新** | dsh 与 Node.js 事务化更新（失败自动回滚，被打断下次启动自动还原）；应用本体自更新（Windows） |
| **服务自愈** | 单实例、看门狗自动恢复、端口冲突自动回退、页面挂起自动重载 |
| **桌面体验** | 托盘常驻、自绘标题栏（主菜单 + 窗口控制）、底部状态栏（会话统计 + API 余额）、统一弹窗（余额/更新/插件/设置/关于） |
| **插件管理** | 内置插件市场：搜索 npm 上的 dsh 插件，一键安装/卸载（走官方 `dsh plugin`） |
| **便携模式** | exe 旁放 `portable.txt`，数据跟随 exe，拷 U 盘即用 |
| **通知与提醒** | 任务完成系统通知（点击回窗口）、运行期每 6 小时自动检查更新 |
| **双语界面** | 界面语言跟随 dsh 设置（中/英），深浅色随系统/主题自动切换 |

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

从 [Releases](https://github.com/JeffioZ/dsh-box/releases) 下载对应平台的产物：

- Windows：直接运行 `DSHBox.exe`
- macOS：下载 `.dmg`，打开后把 **DeepSeek Harness Box** 拖入 Applications，再从启动台/Applications 启动
- Linux：解压 `.zip` 后运行 `DSHBox`

各平台产物以最新 Release 附件为准，GitHub Actions 也会产出各平台的构建产物。所有产物均未签名，系统安全策略可能要求手动允许。

**macOS 首次运行被 Gatekeeper 拦截时**（提示"无法验证…是否包含恶意软件"）：

1. 右键（或按住 `Ctrl` 点击）应用图标 → **打开** → 在弹窗中再次点击 **打开**，仅首次需要；
2. 或在终端执行（把应用拖入终端即可自动填入路径）：
   ```bash
   xattr -dr com.apple.quarantine "/Applications/DeepSeek Harness Box.app"
   ```
3. 若仍被拦：系统设置 → 隐私与安全性 → 找到对应提示 → 点击"仍要打开"。

该提示源于产物未签名/未公证（见"平台支持"），与软件本身无关；签名公证后即可消除。

首次运行会自动补齐运行环境——按需安装 Node.js 与 dsh 包——随后启动 `dsh web`（默认端口 18080）。Linux 还需先安装发行版提供的 WebKitGTK 等 Tauri 系统依赖。

**第一次启动的旅程**：

1. **Loading 界面**：自动检测/安装运行时（进度 + 步骤指示，多步流程一目了然）
2. **首次使用配置**（全新安装时出现）：引导设置 API Key、语言、主题与开机自启（可跳过，之后仍可在 dsh 设置或桌面端设置中调整）
3. **主界面**：就绪后自动进入 dsh 官方 Web 界面；底部状态栏显示会话统计与 API 余额，托盘常驻

## 配置

### 数据目录

| 平台 | 路径 |
|---|---|
| Windows | `%LOCALAPPDATA%\DSHBox` |
| macOS | `~/Library/Application Support/com.deepseek.dsh-box` |
| Linux | `$XDG_DATA_HOME/com.deepseek.dsh-box`（未设置 `XDG_DATA_HOME` 时为 `~/.local/share/com.deepseek.dsh-box`） |

该目录下会生成 `node/`、`dsh/`、`npm-cache/` 与 `logs/`（`desktop.log`，时间为 UTC，超过 2 MB 自动轮转为 `.old`）。更新 dsh 或 Node 期间会短暂出现 `dsh-old/` 或 `node-old/` 备份目录，更新成功后自动清理。

### config.json

数据目录下的可选配置文件：

```json
{
  "port": 18080,
  "api_key": "sk-...",
  "api_base": "https://api.deepseek.com",
  "language": "zh-CN",
  "hide_tool_calls": false,
  "hide_stats_line": true,
  "hide_statusbar": false
}
```

语言与主题会优先跟随 dsh 的 `settings.yaml`；主题不重复写入 `config.json`。开机自启动由各平台的系统机制管理。

### 环境变量

环境变量优先于 `config.json`：

| 变量 | 作用 |
|---|---|
| `DSH_BOX_ROOT` | 覆盖数据根目录 |
| `DSH_BOX_PORT` | 覆盖监听端口 |
| `DSH_BOX_DSH_HOME` | 覆盖 dsh 主目录（`DSH_HOME`） |
| `DSH_BOX_API_KEY` | 覆盖 API Key |
| `DSH_BOX_API_BASE` | 覆盖 API 基地址 |
| `DSHD_LANG` | 固定界面语言（`zh-CN` / `en`，重启后生效，优先级最高） |

### 便携模式

把 `DSHBox.exe` 所在目录放一个空的 `portable.txt` 文件，数据目录（`node/`、`dsh/`、`logs/`、`config.json`）即跟随 exe 存放在旁边的 `data/` 目录，拷贝到 U 盘即可随身携带。删除 `portable.txt` 即恢复常规模式。`DSH_BOX_ROOT` 环境变量仍优先于便携模式。

### API Key

解析顺序：`DSH_BOX_API_KEY` → `config.json` → `DEEPSEEK_API_KEY` → dsh 凭据文件（`$DSH_HOME/.credentials.yaml`，即 `~/.dsh-box/.credentials.yaml`）。

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
# 输出：dist\DSHBox.exe

# 分步
npm install
npm run icons                # 生成图标（首次）
pwsh -File scripts\cargo.ps1 build
node scripts\copy-exe.mjs
```

`build.ps1` 与 `scripts/cargo.ps1 build` 只编译、不修改版本号。发布前递增补丁版本请显式运行 `pwsh -File scripts/bump-version.ps1`。

### 开发模式（改 UI 免编译）

```powershell
pwsh -File .\dev-build.ps1   # 首次：构建不嵌入资源的开发版（dist-dev\DSHBox-dev.exe）
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
    app_dialog.rs       # 统一自绘弹窗（余额/检查更新/插件/设置/关于）
    dialog.rs           # 原生消息框封装（模态、互斥）
    file_actions.rs     # 本地文件动作（默认程序打开 / 定位 / 打开方式）
    icons.rs            # 图标提取（SHGetFileInfo → PNG，含缓存）
    logging.rs          # 日志（轮转）
    locale.rs           # 系统语言检测与中英文选择
    onboarding.rs       # 首次使用配置与持久化
    plugins.rs          # 插件搜索、安装/卸载与内置插件后台维护
    stats.rs            # 会话统计读取、格式化与实时速率估算
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

DSHBox 是 dsh 的**桌面封装**，而不是另一套实现：

- **不改动 dsh**：主界面直接加载官方 `dsh web`，dsh 照常独立升级，外壳随之跟进
- **只做桌面层**：托盘、标题栏、系统通知、更新、本地文件菜单等桌面体验；不重复实现 dsh 的对话与会话能力，也不另存一份会话数据
- **交互克制**：仅通过 dsh 官方提供的页面、数据文件与 CLI 与之协作，不依赖任何未公开的内部接口

更多设计取舍见 [docs/why-desktop.md](docs/why-desktop.md)。

规划中的能力（尚未交付）：

- 手机远程控制：在手机浏览器/App 中继续本机会话
- IM 通道：在微信/飞书等聊天工具中向 Agent 发起任务

## 许可

[MIT](LICENSE)。第三方依赖遵循其各自许可；dsh 本体使用官方 npm 包原样安装，不影响其官方升级。

## 内置插件市场

DSHBox 默认预装两个社区插件（经 `dsh plugin` CLI 安装；`dsh-file-drop` 为 BSD-3-Clause）：

- [dsh-market](https://github.com/dsh-market/dsh-market)（npm 包 `dshmarket`）——dsh 内的可视化插件市场：社区插件目录浏览、搜索、一键安装、主题切换与备份恢复。
- [dsh-file-drop](https://github.com/dannyvan/dsh-file-drop)（npm 包 `dsh-file-drop`）——拖拽/点击文件插入对话：Linux 经 uri-list 直取原始路径，Windows/macOS 走插件自带的工作区上传兜底。

**自动安装**：dsh 服务就绪后自动安装未装的内置包并重启服务生效；仅首次引导执行（按包记录 `market_bootstrapped_<pkg>` 标记）。
**自动更新**：每 24 小时检查一次 npm 最新版本，落后则后台升级并重启服务；检查与升级均静默失败重试，不阻塞使用。可在「设置 → 自动更新内置插件」关闭自动升级（首次预装引导不受影响），或随时在「插件管理」页手动「检查更新 / 更新」。
**移除**：卸载任一内置包后（`dsh plugin --profile web remove <pkg>`），DSHBox 不会自动重装；更新检查也仅作用于仍已安装的包。
**安全软件兼容**：插件安装/升级通过 Node.js 调用 pnpm 执行。若安全软件（如火绒）拦截 node.exe 并弹窗询问，请在信任区添加 DSHBox 数据目录（Windows 默认 `%LOCALAPPDATA%\DSHBox`）或其中的 `node\node.exe`；未放行时自动升级会退避重试（24 小时内不重复打扰），可改用「插件管理」页手动更新。
市场内的插件均为第三方代码，安装前请确认来源可信；列表收录不等于安全背书（见上游 [awesome-dsh-plugin](https://github.com/awesome-dsh-plugin/awesome-dsh-plugin) 的免责声明）。

## 致谢

感谢 DeepSeek Harness 团队与开源社区，以及同类桌面端项目带来的启发；感谢 [dsh-market](https://github.com/dsh-market/dsh-market) 提供的插件市场与 [awesome-dsh-plugin](https://github.com/awesome-dsh-plugin/awesome-dsh-plugin) 维护的社区插件目录；感谢每一位参与测试与反馈的用户。
