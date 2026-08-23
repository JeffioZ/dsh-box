# DSHBox

![CI](https://github.com/JeffioZ/dsh-box/actions/workflows/build.yml/badge.svg)
![License](https://img.shields.io/github/license/JeffioZ/dsh-box)
![Platforms](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-4176e6)

[English](README.en.md) · [架构](docs/architecture.md) · [开发指南](docs/development.md) · [故障排查](docs/troubleshooting.md) · [安全模型](docs/security.md)

DSHBox（包名 `dsh-box`）是 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的跨平台桌面外壳，基于 Tauri v2、Rust 与原生 HTML/CSS/JavaScript。主界面直接加载官方 `dsh web`，外壳负责运行环境、窗口、托盘、更新与本地系统集成，不 fork 或 patch dsh。

> [!IMPORTANT]
> 仓库目前处于首个公开版本发布前。请从源码构建；后续 Release 产物仍将保持未签名，系统可能要求手动放行。详情见[安全模型](docs/security.md)。

## 快速开始

先安装 Rust 1.85+、Node.js `^22.19.0` 或 `>=24.0.0`，以及对应平台的 [Tauri v2 系统依赖](https://v2.tauri.app/start/prerequisites/)。Windows 构建：

```powershell
git clone https://github.com/JeffioZ/dsh-box.git
cd dsh-box
npm ci
npm run check
pwsh -NoLogo -NoProfile -File .\build.ps1
```

Windows 产物位于 `dist\DSHBox.exe`。macOS/Linux 的构建命令和依赖说明见[开发指南](docs/development.md)。

## 功能

| 能力 | 说明 |
|---|---|
| 开箱即用 | 自动检测 Node.js、安装 dsh，并在 Windows 缺少 WebView2 时引导安装 |
| 服务生命周期 | 系统分配端口回退、外部 dsh 安全接入、进程树清理、服务看门狗和页面心跳恢复 |
| 桌面体验 | 自绘标题栏与状态栏、系统托盘、通知、窗口位置记忆、深浅色和中英双语 |
| 安全更新 | dsh/Node 事务化更新和中断恢复；Windows 应用附件按精确 tag 与 SHA-256 校验 |
| 本地文件菜单 | 默认打开、VS Code/记事本打开、文件管理器定位、复制路径与 UTF-8 文本 |
| 插件管理 | 通过官方 `dsh plugin` 搜索、安装、卸载与更新；首次内置插件可明确取消 |
| 模型配置 | 类型化校验并导入/导出 `llm-pi-ai` 自定义路由，凭据与设置分开保存 |
| 便携模式 | Windows exe 同级放置 `portable.txt`，运行时与配置改存相邻 `data/` |

## 平台

| 平台 | 架构 | 最低环境 | 发布形式 |
|---|---|---|---|
| Windows | x64 | Windows 10；WebView2 Runtime | 单个 `DSHBox.exe` |
| macOS | arm64 / x64 | macOS 13.5+ | 未签名 dmg |
| Linux | x64 / arm64 | Ubuntu 22.04、Debian 12 或等价 WebKitGTK 4.1 环境 | zip |

Windows 是主要本地测试平台；五个目标由 GitHub Actions 构建。Linux 上 dsh 的 Landlock 需要内核 5.13+，不满足时由 dsh 自身降级。

## 安装与首次启动

首个公开版本发布后，可从 [Releases](https://github.com/JeffioZ/dsh-box/releases) 下载对应产物：

- Windows：运行 `DSHBox.exe`。
- macOS：把应用拖入 Applications。若 Gatekeeper 拦截，按住 Control 点击应用并选择“打开”，或在“系统设置 → 隐私与安全性”中允许。
- Linux：解压后运行 `DSHBox`；请先安装发行版要求的 WebKitGTK 4.1 依赖。

首次启动会准备运行时，然后显示首次配置页（DeepSeek API Key 可留空）：

1. API Key 写入 dsh 的 `$DSH_HOME/.credentials.yaml`，不会再复制到 DSHBox 的 `config.json`。
2. 语言与主题写入 dsh 的 `settings.yaml`，与官方 CLI/Web 界面共享。
3. 开机自启动使用各平台系统机制。
4. “安装内置插件”默认勾选，但可取消；未勾选时不会自动安装。

之后可在“设置 → 服务管理 → DeepSeek API Key”中替换或清除密钥；若环境变量已提供密钥，该区域只读并显示由外部管理。

## 配置与数据

默认数据根目录：

| 平台 | 路径 |
|---|---|
| Windows | `%LOCALAPPDATA%\DSHBox` |
| macOS | `~/Library/Application Support/com.deepseek.dsh-box` |
| Linux | `$XDG_DATA_HOME/com.deepseek.dsh-box`，未设置时为 `~/.local/share/com.deepseek.dsh-box` |

主要内容：

- `config.json`：用户可理解的外壳设置。
- `state.json`：窗口位置、首次引导和后台维护标记；不建议手工编辑。
- `node/`、`dsh/`、`package-manager/`：DSHBox 管理的 Node、dsh 与固定版本 pnpm。
- `npm-cache/`、`pnpm-store/`：安装与插件维护共用的本地包缓存。
- `logs/dshbox.log`：UTC 日志，超过 2 MiB 轮转为 `.old`。
- `$DSH_HOME`：默认 `~/.dsh`，由官方 dsh 与 DSHBox 共享，不位于上述数据根目录。

`config.json` 支持：

```json
{
  "port": 18080,
  "api_base": "https://api.deepseek.com",
  "language": "zh-CN",
  "hide_tool_calls": false,
  "hide_stats_line": true,
  "hide_statusbar": false,
  "hide_balance": false,
  "auto_update_plugins": true,
  "task_notifications": true,
  "dsh_update_channel": "latest",
  "close_behavior": "tray",
  "launch_behavior": "window",
  "download_source": "auto"
}
```

`dsh_update_channel` 可取 `latest` 或风险更高的预览通道 `next`。`close_behavior` 可取 `tray` / `quit`，`launch_behavior` 可取 `window` / `tray`，`download_source` 可取 `auto` / `official` / `mirror`。`config.json` 与 `state.json` 职责严格分离，不读取旧文件中的跨界字段。

`port` 是本地托管服务的首选端口，不是必须占用的固定端口。DSHBox 先尝试上次成功端口和该首选端口；均不可用时通过 `dsh web --port 0` 让操作系统一次分配可用端口，并把结果记入 `state.json`，不会顺序扫描大段端口。

启动时若在首选端口或 dsh 官方默认端口 `3080` 发现通过页面与 `host.describe` 双重校验的外部 dsh，DSHBox 会先询问是否连接并记住该服务指纹。首次运行时该选择优先于本地设置；选择外部服务会暂缓只适用于本地运行时的凭据与插件引导，日后首次改用本地服务时再补充显示。外部模式只负责显示与重连，不会停止、重启或更新该进程，也不会改写其凭据、模型或插件；服务消失时会明确报错，用户可重试或改用本地服务。

环境变量优先于 `config.json`：

| 变量 | 作用 |
|---|---|
| `DSH_BOX_ROOT` | 覆盖数据根目录 |
| `DSH_BOX_PORT` | 覆盖监听端口 |
| `DSH_BOX_API_KEY` | 覆盖 DeepSeek API Key |
| `DSH_BOX_API_BASE` | 覆盖 API 基地址 |
| `DSH_HOME` | 覆盖 dsh 官方主目录 |
| `DSHD_LANG` | 固定 `zh-CN` 或 `en` |

API Key 解析顺序为：`DSH_BOX_API_KEY` → `DEEPSEEK_API_KEY` → `$DSH_HOME/.credentials.yaml`。

## 内置插件

首次引导可选择安装：

- [DSH Market](https://github.com/dsh-market/dsh-market)（`dshmarket`）
- [DSH File Upload](https://github.com/HongMing-Huang/dsh-file-upload)（`dsh-file-upload`）

同意后，未安装的内置插件会在 dsh 就绪后安装；仍保持内置身份的已安装插件每 24 小时检查一次更新。更换预置插件时，仍由 DSHBox 管理的旧包会迁移到新包；用户曾主动卸载旧包则沿用原选择，不会借换包重新安装。用户主动卸载后，插件页会保留手动重装入口；装回后按普通用户插件管理，不再恢复内置标签或自动更新。安装、更新和卸载都走 `dsh plugin` CLI；多个变更会合并为一次重启，自动维护只在会话空闲后应用。

插件管理页另有“社区插件”列表，仅提供来源和手动安装入口，不会自动安装或更新；卸载后会重新出现在该列表中。

插件是与 dsh 同环境执行的第三方代码。社区清单、内置或市场收录都不构成安全背书；安装前应核实来源。清单维护规则见 [resources/README.md](src-tauri/resources/README.md)。

## 开发

要求 Rust 1.85+、Node.js `^22.19.0` 或 `>=24.0.0`，以及对应平台的 Tauri 系统依赖。Windows 推荐 PowerShell 7。

```powershell
npm install
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

Windows 构建与开发：

```powershell
pwsh -NoLogo -NoProfile -File .\build.ps1
pwsh -NoLogo -NoProfile -File .\dev-build.ps1
pwsh -NoLogo -NoProfile -File .\dev-run.ps1
```

开发模式通过本地 4321 端口加载 `ui/`，只改 UI 不需要重编 Rust。完整环境、图标、版本和发布流程见[开发指南](docs/development.md)。

## 项目结构

```text
desktop/
├─ assets/brand/                   # 唯一品牌 SVG 源
├─ ui/                             # 无打包器的内置页面、共享样式与双语文案
│  ├─ index.html + startup.*       # 启动页与首次配置
│  ├─ control-center.*             # 余额/更新/插件/设置/关于
│  ├─ titlebar.* / statusbar.*     # 主窗口子 WebView
│  ├─ tray-menu.html + menu.js     # 托盘与菜单交互
│  └─ common.* + i18n.js           # 共享工具、设计 token、文案
├─ src-tauri/
│  ├─ resources/                   # 内置插件清单与页面注入资源
│  └─ src/
│     ├─ bootstrap.rs / lib.rs     # 应用装配与公共边界
│     ├─ app_state/                # 配置、状态、JSON/文本持久化
│     ├─ commands/                 # 仅做 IPC 来源校验与转发
│     ├─ runtime/                  # Node、dsh 包和服务启动
│     ├─ updater/                  # 检查、平台更新与事务恢复
│     ├─ plugins/                  # CLI 执行、维护策略与手动操作
│     ├─ model_config/             # 模型路由解析、导入与导出
│     ├─ webview/                  # 导航边界、自定义协议与注入
│     └─ platform/windows/         # Windows 专属 WebView2 预检
├─ scripts/                        # 一致性检查、图标、构建辅助
└─ .github/workflows/              # 三平台 CI 与 tag 发布
```

详细依赖方向和启动时序见[架构文档](docs/architecture.md)。

## 项目边界

DSHBox 只通过三条通道与 dsh 协作：

1. 向官方 Web 页面注入受限的初始化/菜单脚本。
2. 读取会话日志，行级合并写入 `$DSH_HOME/settings.yaml` 与 `.credentials.yaml`。
3. 调用 `dsh web` 和 `dsh plugin ...`。

不 fork dsh、不 patch npm 包、不修改会话格式、不重复实现官方 Web UI。更多取舍见[为什么做 DSHBox](docs/why-desktop.md)。

## 参与与许可

提交前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 和 [SECURITY.md](SECURITY.md)。项目代码采用 [MIT License](LICENSE)；依赖、运行时下载项和品牌资源说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
