# 运行时资源

- `builtin-plugins.json`：首次引导可选安装并由 DSHBox 维护的插件清单。
- `recommended-plugins.json`：插件管理页展示的社区插件清单，只提供手动安装入口。
- `injections/context-menu.js`：导航到可信 dsh 页面后注入的右键菜单。

## 内置插件清单

每项字段：

- `id`：实际 npm 包名，也是内部状态键。
- `spec`：传给 `dsh plugin add` 的依赖规格。
- `name`、`description_zh`、`description_en`、`homepage`：双语展示与来源核对信息。
- `replaces`（可选）：当前包接替的全部旧内置包名。迁移会继承旧包的主动卸载选择；旧包仍在 profile 时先卸载，卸载失败则不安装新包。

新增插件前必须核实仓库、许可证、维护状态、发布方式和安装副作用。用户主动卸载后，界面会继续使用这里的元数据提供手动重装入口，但不会恢复内置身份。普通下线只移出 JSON，保留用户已安装副本；功能换代用 `replaces`，并保留完整前代链；安全或兼容事故需要无后继地强制撤回时，加入 `plugins/maintenance.rs` 的 `MARKET_REMOVED`。当前包、替换前代与强制撤回清单不得重叠。

## 社区插件清单

`recommended-plugins.json` 的每项必须包含稳定 `id`、安装 `spec`、展示名、中英文描述和 HTTPS GitHub 主页。社区清单与内置清单不能收录同一包。该清单不授予内置身份，不触发自动安装或升级；安装前界面必须允许用户查看项目来源。

## 注入脚本

脚本在 dsh 页面 origin 内执行，但本地文件动作必须经带会话令牌的 `dshd://` 协议返回 Rust。修改菜单后运行 `npm run check`，并手工验证链接、选中文本和本地文件三种右键场景；不要用简化菜单替换操作系统用户已经依赖的项目。
