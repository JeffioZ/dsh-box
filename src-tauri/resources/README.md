# 运行时资源

- `builtin-plugins.json`：首次引导可选安装并由 DSHBox 维护的插件清单。
- `injections/context-menu.js`：导航到可信 dsh 页面后注入的右键菜单。

## 内置插件清单

每项字段：

- `id`：实际 npm 包名，也是内部状态键。
- `spec`：传给 `dsh plugin add` 的依赖规格。
- `name`、`description`、`repoUrl`：审阅与文档元数据。
- `recommended`：清单意图标记；当前清单均为推荐项。

新增插件前必须核实仓库、许可证、维护状态和安装副作用。普通下线只移出 JSON，保留用户已安装副本；发现安全或兼容事故需要强制撤回时，还要加入 `plugins/maintenance.rs` 的 `MARKET_REMOVED`。同一个包不能同时出现在维护与撤回清单。

## 注入脚本

脚本在 dsh 页面 origin 内执行，但本地文件动作必须经带会话令牌的 `dshd://` 协议返回 Rust。修改菜单后运行 `npm run check`，并手工验证链接、选中文本和本地文件三种右键场景；不要用简化菜单替换操作系统用户已经依赖的项目。
