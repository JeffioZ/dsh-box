# 架构

## 目标与约束

DSHBox 是 dsh 的薄桌面外壳。对话、Agent、工具、会话格式和官方 Web UI 仍由 dsh 负责；桌面端只管理运行时、生命周期与操作系统集成。任何新能力必须落在页面注入、`$DSH_HOME` 数据或 `dsh` CLI 三条通道之一。

## 模块边界

```text
内置 UI ──IPC──> commands/ ──> 业务模块
   │                 │             ├─ runtime/：Node、dsh 包、服务
   │                 │             ├─ updater/：检查、替换、恢复
   │                 │             ├─ plugins/：dsh plugin CLI
   │                 │             └─ app_state/：运行态与持久化
   │                 └─ 只校验来源、参数并转发
   │
   └─主 WebView──> 官方 dsh web
                     ├─ webview/：可信导航、dshd://、脚本注入
                     └─ $DSH_HOME：设置、凭据、只读会话日志
```

- `bootstrap.rs` 组装 Tauri 插件、窗口、事件和后台任务；`lib.rs` 只保留跨模块公共边界。
- `commands/` 不实现业务，所有 IPC 首先要求调用方是内置页面。
- `app_state/config.rs` 解析用户配置与环境变量；`store.rs` 区分 `config.json` 和 `state.json`；`managed_file.rs` 提供串行、原子文本替换。
- `runtime/` 负责“可运行”，`dsh.rs` 负责“持续运行”，`updater/` 负责“安全替换”，三者不互相复制流程。
- `webview/` 处理可信 origin、自定义协议和导航后注入。右键菜单脚本独立放在 `resources/injections/`，便于语法和契约检查。

## 启动时序

```text
AppState 初始化
  → 恢复中断的更新事务
  → 创建主窗口与内置启动页
  → boot_loop / boot_once
      → 检测或安装 Node
      → 检测或安装 dsh
      → 静默检查更新
      → 启动 dsh web
      → 健康检查
      → 等待首次配置（仅全新安装）
      → 导航到本机 dsh origin
  → watchdog 持续检查服务与页面心跳
```

首次配置不是独立启动线程：它是 `boot_once` 状态机中的一个等待点，避免多个流程竞争窗口状态或重复启动服务。

## 数据所有权

| 数据 | 所有者 | DSHBox 行为 |
|---|---|---|
| `config.json` | DSHBox/用户 | 读写用户设置 |
| `state.json` | DSHBox | 内部状态，不建议手改 |
| `$DSH_HOME/settings.yaml` | dsh/用户 | 只合并约定段落 |
| `$DSH_HOME/.credentials.yaml` | dsh/用户 | 只合并指定凭据行 |
| 会话日志 | dsh | 只读统计与通知 |
| `node/`、`dsh/` | DSHBox | 事务化安装和更新 |

`config.json` 与 `state.json` 不做跨文件兜底或隐式迁移。API Key 不属于 DSHBox 配置，只从环境变量或 dsh 的 `.credentials.yaml` 读取。

插件安装、卸载和更新先形成内存中的“待应用”状态：手动操作允许继续批量修改，用户确认后再合并为一次服务重启；后台维护只在 RPC 能明确确认会话空闲时重启。重启前记录当前 dsh URL，服务恢复后回到原会话位置。

## 更新不变量

- dsh/Node 的目录替换必须复用 `updater/transaction.rs`：备份、标记、校验、提交或回滚。
- Windows 应用附件必须来自精确版本 tag 的唯一资产，先写 `.part` 并验证 Release digest，再进入替换脚本。
- 失败或进程中断后必须保留足以恢复的备份和标记；不能为了“清理干净”删除最后一份可用版本。

## 前端组织

内置页没有打包器。HTML 只保留结构，页面级样式/逻辑使用同名 CSS/JS；`common.css` 是设计 token 和共享组件，`common.js` 是工具，`i18n.js` 是唯一双语文案表。动态 HTML 的文本必须经过转义或 DOM `textContent`。

这套方案降低构建复杂度，代价是没有模块打包与静态类型检查，因此 `npm run check` 会遍历全部受控文件，检查 JS 语法、HTML 本地引用、i18n 完整性、资源清单、图标和右键注入契约。插件网络/进程操作和会话统计查询都放入阻塞任务池，避免占住 UI 的异步执行线程。
