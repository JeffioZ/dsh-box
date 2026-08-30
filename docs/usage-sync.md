# 用量与余额：与上游 dsh-usage-stats 的同步规程

「用量与余额」模块衍生自 [dsh-usage-stats](https://github.com/Ychris12138/dsh-usage-stats)（MIT，见 `THIRD_PARTY_NOTICES.md`）。上游是 dsh 插件且非常活跃，本文件约定我们如何跟踪其更新。

## 同步锚点

- 上游仓库：`https://github.com/Ychris12138/dsh-usage-stats.git`
- 当前锚定版本：**v0.3.1（commit `c6212d9`，2026-08-28）**。适配器与余额层（`balance.rs` / `subscriptions.rs`）、定价与成本账（`pricing.rs` / `aggregate.rs` 成本累加）已按 v0.3.1 移植；聚合 token 语义（`lib/usage.js`）自 f513669 起上游无变化，视同已对齐。
- 关注信号：新 tag、`lib/usage.js` 的语义与注释变更、适配器清单变更（`lib/accounts.js` / `lib/subscriptions.js` / `lib/balance.js`）、**`lib/pricing.js` 的价格目录变更**（官方调价；移植时同步递增 `usage/cache.rs` 版本号强制重折成本账）

## 文件映射

| DSHBox | 上游 | 关系 |
|---|---|---|
| `src-tauri/src/usage/aggregate.rs` | `lib/usage.js` | 逐函数移植（文件头有映射表与分歧清单） |
| `src-tauri/src/usage/aggregate.rs` 的 `CostAcc` / 成本替换去重 | `lib/billing.js` 的成本累加器（`applyCostSample` 双向、fail-closed `costComplete`） | 语义移植；金额以 USD 单币种实现（上游多币种 map 简化） |
| `src-tauri/src/usage/pricing.rs` | `lib/pricing.js`（`DEEPSEEK_PRICING_RULES` / `estimateTokenCost` / 峰谷判定） | 定价目录与判定移植；上海时区以固定 UTC+8 实现（1991 年后无夏令时） |
| `src-tauri/src/usage/aggregate.rs` 的 `FoldState.kind` / `reset_fold` | `lib/index.js` 的 `state.kind`（live/persisted）与 `resetUsageState` | 语义移植；本壳只有持久化日志源，kind 恒为 persisted |
| `src-tauri/src/usage/aggregate.rs` 的 `current_route` + `usage/mod.rs` 的 `session_context` | `lib/usage.js` 的 `currentRoute` / `currentSessionContext` 与 `lib/index.js` 的 session-context 端点 | 语义移植；展示名解析走本仓 `providers.rs`（不移植 provider-identity 的 family/account 归一） |
| `src-tauri/src/usage/providers.rs` | `lib/index.js` 的 `configuredProviders` | 同两来源口径 |
| `src-tauri/src/usage/balance.rs` | `lib/balance.js` + `lib/accounts.js` 契约（含 OrcaRouter / New API / Sub2API 适配器） | 适配器契约参考，结构重写 |
| `src-tauri/src/usage/subscriptions.rs` | `lib/subscriptions.js`（五适配器 + v0.3.1 的区域主机/端点回退/裸 key 鉴权） | 解析逻辑参考 |
| `src-tauri/src/net_guard.rs`（`guard_https_or_lan_http` / `read_json_capped`） | `lib/network.js` + `lib/accounts.js` 传输层 | 同一口径（https 任意主机 / http 仅私网放行、1 MiB 响应上限）；状态栏余额与用量账户共用 |
| `src-tauri/src/usage/cache.rs` | `lib/index.js` 缓存段 | 同概念；文件名与版本号有意独立（当前 v4，含成本账） |
| `src-tauri/src/usage/log.rs` | `scripts/verify-raw.mjs` | 同「直扫会话日志」思路 |
| `ui/control-center.js` 用量页 | `lib/client.js` 面板结构 | 结构参照，视觉用本仓设计体系重写 |

## 刻意分歧（同步时不得「对齐」掉）

- 聚合 `render` 对同 token 模型加名称次序（上游无）。
- 聚合缓存文件名 `dshbox-usage-stats-cache.json` 与独立版本号（与上游缓存互不兼容，防互相污染）。
- 数据源：上游走 cordis 服务（内存事件 + persistence API），我们直扫 `$DSH_HOME/sessions/*.jsonl.zstd`——因此 `FoldState.kind` 恒为 persisted，live/persisted 迁移分支不存在。
- 账户监测：上游单provider手动刷新；我们为页面级刷新（无供应商选择器）。
- 会话上下文：上游端点支持浏览器路由提示参数（`?provider=&model=`）与多会话 `?session=` 参数；我们只取后端推断的当前会话 + 折叠归因（`usage_session_context_get`，字段契约为 `route_id` / `display_name` / `model`）。
- **定价资格**：上游按 provider-identity（含 baseURL 主机名 `api.deepseek.com` 校验）判定官方 DeepSeek；我们折叠时拿不到 baseURL，取日志归因 `provider == "deepseek"` 即定价（自定义路由伪造该归因且模型名恰为 v4 官方名才会误报）。
- **New API / Sub2API 的路由识别**：上游有 provider-identity 主机探测与 Sub2API 面板指纹（`/api/v1/settings/public` 的 `affiliate_enabled`）自动识别；我们不移植探测，**用户把路由 id 命名为 `new-api`（或 `newapi`）/ `sub2api`（或 `passion`）即启用对应适配器**。
- **OpenRouter 凭据**：上游 v0.3.1 要求 Management Key（`OPENROUTER_MANAGEMENT_KEY`），我们同款（不吃 DeepSeek 壳级覆盖链）。
- **Z.ai 鉴权**：编码计划端点用裸 key（非 Bearer），区域经 `ZAI_API_REGION` 或 `zai-coding-cn` 路由 / bigmodel.cn baseURL 推断，国内站走 `open.bigmodel.cn`（v0.3.1 同款）。
- UI 用 DSHBox 设计 token 与双语体系，不搬上游字面 CSS/组件代码。

## 同步步骤

1. `git -C <上游克隆> fetch --tags`，对比当前锚定 tag 与新 tag 的 `lib/usage.js`、`lib/pricing.js`、`lib/billing.js`、`lib/balance.js`、`lib/subscriptions.js`、`lib/accounts.js`、`lib/provider-identity.js`。
2. 语义变更逐条移植到对应 Rust 文件；适配器新增/端点变更同步到 `usage/balance.rs` / `usage/subscriptions.rs` 并更新其文件头锚点注释。
3. 聚合缓存结构变化、或 `pricing.rs` 价格目录变更时递增 `usage/cache.rs` 的版本号（旧缓存静默重折，无需迁移）。
4. 移植范围扩大到新文件时，更新 `THIRD_PARTY_NOTICES.md` 的声明范围与本文档的映射表。
5. 跑全量检查：`npm run check`、`cargo test --all-targets`、Clippy。
6. 完成后把锚定版本号更新到本文档与 `usage/aggregate.rs` 文件头。

## 观察名单（暂不跟进）

- **dsh 持久化格式演进**：dsh 0.1.2 起默认开启 delta 打包（`text-chunks`/`reasoning-chunks`/`tool-call-chunks` 存储行）；聚合只读 usage 块（永不打包）不受影响，实时速率（`live.rs`）已双形态兼容。另有 opt-in 的 SQLite 持久化后端（`session-persistence-sqlite`，无默认启用）——若上游翻转默认值，「直扫 jsonl.zstd」数据源失效，需迁移到别的通道。
- **成本预算**（上游 `budgets.currency/daily/monthly` + 80%/100% 告警）：需 config.json 新键 + 设置界面；有需求再加。
- **CSV/JSON 导出**（上游 `lib/export.js`，无密钥、schema 版本化）：待做（`tauri-plugin-dialog` 已具备保存能力）。
- **自适应账户刷新**（上游 active 60s / detail 120s / background 900s + 限流退避）：我们固定 300s 一轮。
- 低余额绝对阈值配置（`warning.warnBelow/criticalBelow`）：我们当前只实现 30%/10% 比例阈值，无设置界面；有需求再加。
- **声明式余额查询**（上游 JSON-Pointer 自定义查询模板）：自托管网关可先用 New API / Sub2API 适配器。
