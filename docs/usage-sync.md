# 用量与余额：与上游 dsh-usage-stats 的同步规程

「用量与余额」模块衍生自 [dsh-usage-stats](https://github.com/Ychris12138/dsh-usage-stats)（MIT，见 `THIRD_PARTY_NOTICES.md`）。上游是 dsh 插件且非常活跃，本文件约定我们如何跟踪其更新。

## 同步锚点

- 上游仓库：`https://github.com/Ychris12138/dsh-usage-stats.git`
- 当前锚定版本：**v0.3 未发布，锚定 commit `f513669`（2026-08-24）**，待 v0.3 正式 tag 后回锚（锚定 tag 期间不跟 main——上游 main 含未发布改动）
- 关注信号：新 tag、`lib/usage.js` 的语义与注释变更、适配器清单变更（`lib/accounts.js` / `lib/subscriptions.js` / `lib/balance.js`）

## 文件映射

| DSHBox | 上游 | 关系 |
|---|---|---|
| `src-tauri/src/usage/aggregate.rs` | `lib/usage.js` | 逐函数移植（文件头有映射表与分歧清单） |
| `src-tauri/src/usage/aggregate.rs` 的 `FoldState.kind` / `reset_fold` | `lib/index.js` 的 `state.kind`（live/persisted）与 `resetUsageState` | 语义移植；本壳只有持久化日志源，kind 恒为 persisted |
| `src-tauri/src/usage/aggregate.rs` 的 `current_route` + `usage/mod.rs` 的 `session_context` | `lib/usage.js` 的 `currentRoute` / `currentSessionContext` 与 `lib/index.js` 的 session-context 端点 | 语义移植；展示名解析走本仓 `providers.rs`（不移植 provider-identity 的 family/account 归一） |
| `src-tauri/src/usage/providers.rs` | `lib/index.js` 的 `configuredProviders` | 同两来源口径 |
| `src-tauri/src/usage/balance.rs` | `lib/balance.js` + `lib/accounts.js` 契约 | 适配器契约参考，结构重写 |
| `src-tauri/src/usage/subscriptions.rs` | `lib/subscriptions.js` | 五适配器解析逻辑参考 |
| `src-tauri/src/usage/cache.rs` | `lib/index.js` 缓存段 | 同概念；文件名与版本号有意独立 |
| `src-tauri/src/usage/log.rs` | `scripts/verify-raw.mjs` | 同「直扫会话日志」思路 |
| `ui/control-center.js` 用量页 | `lib/client.js` 面板结构 | 结构参照，视觉用本仓设计体系重写 |

## 刻意分歧（同步时不得「对齐」掉）

- 聚合 `render` 对同 token 模型加名称次序（上游无）。
- 聚合缓存文件名 `dshbox-usage-stats-cache.json` 与独立版本号（与上游缓存互不兼容，防互相污染）。
- 数据源：上游走 cordis 服务（内存事件 + persistence API），我们直扫 `$DSH_HOME/sessions/*.jsonl.zstd`——因此 `FoldState.kind` 恒为 persisted，live/persisted 迁移分支不存在。
- 账户监测：上游单provider手动刷新；我们为页面级刷新（无供应商选择器）。
- 会话上下文：上游端点支持浏览器路由提示参数（`?provider=&model=`）与多会话 `?session=` 参数；我们只取后端推断的当前会话 + 折叠归因（`usage_session_context_get`，字段契约为 `route_id` / `display_name` / `model`）。
- UI 用 DSHBox 设计 token 与双语体系，不搬上游字面 CSS/组件代码。

## 同步步骤

1. `git -C <上游克隆> fetch --tags`，对比当前锚定 tag 与新 tag 的 `lib/usage.js`、`lib/balance.js`、`lib/subscriptions.js`、`lib/accounts.js`、`lib/provider-identity.js`。
2. 语义变更逐条移植到对应 Rust 文件；适配器新增/端点变更同步到 `usage/balance.rs` / `usage/subscriptions.rs` 并更新其文件头锚点注释。
3. 聚合缓存结构变化时递增 `usage/cache.rs` 的版本号（旧缓存静默重折，无需迁移）。
4. 移植范围扩大到新文件时，更新 `THIRD_PARTY_NOTICES.md` 的声明范围与本文档的映射表。
5. 跑全量检查：`npm run check`、`cargo test --all-targets`、Clippy。
6. 完成后把锚定版本号更新到本文档与 `usage/aggregate.rs` 文件头。

## 观察名单（暂不跟进）

- **v0.3 正式 tag**：当前锚定 commit `f513669`；tag 落地后回锚并复核 `lib/usage.js` / `lib/index.js` 有无 tag 前追加改动。
- 低余额绝对阈值配置（`warning.warnBelow/criticalBelow`）：我们当前只实现 30%/10% 比例阈值，无设置界面；有需求再加。
