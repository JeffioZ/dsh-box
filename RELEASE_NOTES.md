# DSHBox 1.0.0

首个正式版本。DSHBox 是 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（dsh）的跨平台桌面外壳（Tauri v2 + Rust），主界面加载官方 `dsh web` 界面。自 0.1.16 起经多轮迭代，本版本为功能与稳定性基线。

## 运行时管理

- **零依赖准备**：启动流程分步引导（检测 → 安装 → 启动服务），进度在启动页实时展示；自动安装 Node.js 与 dsh 包
- **事务化更新**：`update_txn` 提供"备份 + 标记 + 中断恢复"原语，dsh 与 Node.js 替换失败自动回滚，更新被打断时下次启动自动还原
- **服务自愈**：看门狗健康检查失败自动重启 `dsh web`；页面心跳监控（10s 轮询、35s 超时判定、指数退避上限约 18.7 分钟），主线程挂起/崩溃自动重载
- **周期检查**：启动时静默检查 + 运行期每 6 小时检查 dsh 新版本（仅提示，不自动安装）；检查更新弹窗内实时展示下载/安装/重启进度与结果（事件即时渲染 + 轮询兜底，竞态不丢结果）
- **进程树守卫**：Windows Job / Unix 进程组，退出时清理全部子进程，不留孤儿

## 首次使用与配置

- **首次引导**：设置 API Key、语言、主题与开机自启（可跳过，之后可在设置中调整）
- **语言与主题跟随**：读取 dsh 的 `$DSH_HOME/settings.yaml`（`locale.preference` / `ui-theme.preference`），行级合并原子写入，mtime 门控检测（3s 周期，文件不变零开销）
- **环境变量优先**：`DSH_BOX_ROOT` / `DSH_BOX_PORT` / `DSH_BOX_API_KEY` / `DSH_BOX_API_BASE` / `DSH_BOX_DSH_HOME` / `DSHD_LANG` 优先于 `config.json`
- **便携模式**：可执行文件旁放置 `portable.txt` 后，数据目录跟随 exe（exe 旁 `data/`），拷 U 盘即用
- **配置项**：`port` / `api_key` / `api_base` / `language` / `hide_tool_calls` / `hide_stats_line` / `hide_statusbar`

## 桌面体验

- 系统托盘常驻、自绘标题栏（主菜单 + 窗口控制）、统一弹窗（单窗口左侧导航：余额 / 检查更新 / 插件管理 / 设置 / 关于）
- 底部状态栏：会话统计（轮步 / 耗时 / token，实时 tok/s 尾帧估算，RPC 轮询）+ API 余额 chip
- 余额查询：复用单例短超时 HTTP agent；弹窗 stale-while-revalidate（先显缓存、后台刷新）
- 任务完成系统通知：轮询会话日志（`turn`/`end` 事件）；macOS/Linux 点击通知经 `RunEvent::Resumed` 恢复主窗口，Windows 走系统激活与单实例回调
- dsh 页面注入：深色主题首帧预设、右键菜单（含 VS Code 打开本地文件，`dshd://` 协议仅接受绝对路径）、隐藏工具调用与统计行 CSS 注入
- 窗口尺寸记忆；深浅色主题全量 token 化；单实例（重复启动把参数转交已运行实例）

## 插件与扩展

- **内置插件市场**：npm registry 搜索，一键安装/卸载 dsh 插件（走官方 `dsh plugin` CLI），装/卸后自动重启服务生效
- 分类快捷入口：皮肤/主题、工具、工作流预设
- 自动预装 `dshmarket` 与 `dsh-file-drop`（后者 BSD-3-Clause）：已装包每 24 小时检查 npm 最新版，落后时后台自动升级并重启服务；失败静默重试，不阻塞启动

## 应用自更新与分发

- **应用本体自更新（Windows）**：检查 GitHub Releases 最新版 → 下载并校验 → 确认后替换 exe（保留 `.old` 备份、失败回滚）→ 自动重启
- 新版后台预下载：周期检查发现新版即自动下载，完成提示重启应用
- **WebView2 校验**：注册表读取实际版本，缺失或过旧时下载官方引导安装器自动安装

## 平台支持

| 平台 | 架构 | 产物 |
|---|---|---|
| Windows | x64 | 单个 exe，双击即用 |
| macOS | arm64 / x64 | dmg，拖入 Applications 安装（未签名，首次运行需按指引手动放行） |
| Linux | x64 / arm64 | zip，解压即用 |

三平台由 GitHub Actions 自动构建，tag 触发发布，产物全部来自 CI。

## 工程与安全

- 更新事务原语（备份 + 标记 + 中断恢复），所有"替换文件"类操作复用
- 会话日志只读解析（zstd 解压设上限防内存膨胀）；`dshd://` 协议仅接受绝对路径
- 单测覆盖版本解析、更新事务、主题/语言行级合并等核心逻辑；CI 强制 fmt / clippy `-D warnings` / 单测
