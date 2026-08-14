# DSHDesktop

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的跨平台桌面外壳，基于 [Tauri v2](https://tauri.app)。界面即官方 `dsh web`，功能与浏览器完全一致。

## 功能

- 单文件、体积小：Windows 单 exe（约 4–5 MB），无控制台窗口，双击直接出界面
- 开箱即用：自动安装 WebView2（Windows）/ Node.js / dsh 包，干净机器也能跑
- 托盘常驻：打开、查询 API 余额、在浏览器中打开、重启服务、检查更新、开机自启动、关于、退出
- 一键更新 dsh 与 Node.js（事务化，失败自动回滚；启动时静默检查新版）
- 标题栏实时显示 DeepSeek API 余额（官方 `/user/balance` 接口，每 5 分钟刷新）
- 单实例、看门狗自愈、端口冲突自动回退、窗口位置记忆、深浅色自适应
- 自绘右键菜单（dsh 页面内）与自绘托盘菜单/弹窗，风格与 dsh 界面统一

## 平台支持

| 平台 | 架构 | 状态 |
|---|---|---|
| Windows 10/11 | x64 | ✅ |
| macOS | arm64 / x64 | ✅ |
| Linux | x64 | ✅ |

## 使用

下载对应平台的二进制直接运行。首次运行会自动安装 Node.js（缺失时）与 dsh 包，随后启动 `dsh web`（默认端口 3080）。

数据目录：
- Windows：`%LOCALAPPDATA%\DSHDesktop`
- macOS：`~/Library/Application Support/com.deepseek.dsh-desktop`
- Linux：`~/.local/share/com.deepseek.dsh-desktop`

可选配置（数据目录下 `config.json`）：

```json
{
  "port": 3080,
  "api_key": "sk-...",
  "api_base": "https://api.deepseek.com"
}
```

API Key 解析顺序：`config.json` → 环境变量 `DEEPSEEK_API_KEY` → dsh 凭据文件（`~/.dsh/.credentials.yaml`）。

## 构建

前置：Rust stable；Windows 另需 VS2022 C++ 工具链，Linux 另需 WebKitGTK 系统包（`libwebkit2gtk-4.1-dev`、`libgtk-3-dev`、`libayatana-appindicator3-dev` 等）。

```powershell
# 一键构建（Windows）
powershell -NoProfile -ExecutionPolicy Bypass -File .\build.ps1

# 分步
npm install
npm run icons                # 生成图标（首次）
powershell -File scripts\cargo.ps1 build
node scripts\copy-exe.mjs
```

开发模式（改 UI 免编译）：
```powershell
powershell -File .\dev-build.ps1   # 首次：构建不嵌入资源的开发版（dist-dev\DSHDesktop-dev.exe）
powershell -File .\dev-run.ps1     # 运行：自动启动 UI 静态服务器(4321) + 开发版 exe
# 之后只改 ui/ 下的文件，重启 dev-run.ps1（或刷新页面）即生效；改 Rust 代码需重新 dev-build
```

CI（GitHub Actions）自动完成三平台构建、单测与产物上传。

## 项目结构

```
desktop/
  ui/                   # 启动页、标题栏、托盘菜单、统一弹窗（common.css 共享样式）
  src-tauri/src/        # Rust 外壳（按职责分层，依赖无环）
    main.rs             # 入口；Windows 含 WebView2 自动安装预检
    lib.rs              # run() 组装、状态广播、导航、右键菜单注入脚本、自定义协议
    app_state.rs        # 共享状态：配置、引导阶段、生命周期锁、弹窗轮询数据
    commands.rs         # Tauri 命令层（IPC 转发，无业务实现）
    app_dialog.rs       # 统一自绘弹窗（余额 / 检查更新 / 关于）
    dialog.rs           # 原生消息框封装（模态、互斥）
    file_actions.rs     # 本地文件动作（默认程序打开 / 定位 / 打开方式）
    icons.rs            # 图标提取（SHGetFileInfo → PNG，含缓存）
    logging.rs          # 日志（轮转）
    titlebar.rs         # 自绘标题栏（子 webview）
    tray_menu.rs        # 自绘托盘菜单窗口
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

依赖方向：`versions`（无依赖）→ `runtime` → `dsh` → `updater`/`app_state`；`commands`/`tray`/`balance`/`app_dialog`/`tray_menu` 只调用下层。平台差异集中在 `processes.rs`、`autostart.rs`、`main.rs`、`runtime.rs`、`file_actions.rs`（按平台选 Node 包与打开方式），其余模块均跨平台。

## 开发约定

- 提交信息使用 Conventional Commits 风格
- 推荐使用 PowerShell 7（pwsh）执行构建脚本；脚本兼容系统自带 Windows PowerShell 5.1
- 图标改动后需 `npm run icons` 并重新构建
- 本项目由 AI（DeepSeek 智能体）辅助开发与维护，改动经人工审阅

## 许可

[MIT](LICENSE)。第三方依赖遵循其各自许可；dsh 本体使用官方 npm 包原样安装，不影响其官方升级。
