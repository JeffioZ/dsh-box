//! 会话 Token 用量聚合（纯逻辑，无 IO）。
//!
//! ## 版权归属
//!
//! 本模块的聚合算法（事件样本提取、bucket 结构 `input/output/cacheRead/
//! cacheWrite`、按 `(turn, step)` 去重替换、按 `provider/model` 归因、增量
//! 游标折叠、日志收缩后的整段重折）衍生自
//! [dsh-usage-stats](https://github.com/Ychris12138/dsh-usage-stats)
//! （Copyright (c) 2026 dsh-usage-stats contributors，MIT License），并以
//! Rust 独立重写。因其构成对上述 MIT 许可软件的实质性派生，按 MIT 条款
//! 在此保留版权与许可声明，完整文本见仓库根 `THIRD_PARTY_NOTICES.md`。
//!
//! 同步锚点：上游仓库 <https://github.com/Ychris12138/dsh-usage-stats>
//! （npm 包 `@ychris12138/dsh-usage-stats`）。token 聚合语义锚定
//! `f513669`（2026-08-24，对应源文件 `lib/usage.js` 的
//! `applyUsageDelta` / `resetUsageState` / `renderUsage` 等，其后上游无
//! 语义变化）；成本账（`CostAcc` 与样本成本估算，见 `pricing.rs`）锚定
//! **v0.3.1**（`c6212d9`，2026-08-28，对应 `lib/billing.js`）。
//! 与上游的刻意分歧：
//! - `render` 同 token 的模型行按名称升序二次排序（上游仅按 token 降序，
//!   并列时保持插入序）；
//! - 增量缓存文件名与版本独立（`dshbox-usage-stats-cache.json`，见
//!   `cache.rs`），不与上游共享、互写缓存文件；
//! - 无时间戳的样本跳过不折（上游会落入 `NaN-NaN-NaN` 日期桶）；
//! - 数据源只有持久化会话日志一种，`FoldState.kind` 恒为 `Persisted`
//!   （上游还有 live 内存事件源并处理 live/persisted 迁移；字段与
//!   `reset_fold` 语义保留，缓存结构与上游对齐）；
//! - 成本以 USD 单币种累加（上游多币种 map 简化）；定价资格取日志归因
//!   `provider == "deepseek"`（上游另校验 baseURL 主机名，见
//!   docs/usage-sync.md 分歧清单）。
//!
//! ## 语义说明
//!
//! 追加式会话日志中的用量样本来源：
//! - `assistant/chunk` 且 `data.chunk.type == "usage"`：`data.chunk.usage`
//! - `assistant/message`：`data.usage`
//!
//! 同一 `(turn, step)` 的重复样本是「替换」而非累加，避免流式过程中
//! 前值后值被重复计入；后一个样本归于其自身事件发生日。
//!
//! 模型归因：`assistant/message` 用 `data.message.source.{provider,model}`；
//! `usage` chunk 回退到最近一次 `request/header` 的 `data.header.config`；
//! 两者皆无则落入 `unknown/unknown`。回退游标 `current_model` 只在
//! `request/header` 事件上更新——`assistant/message` 的归因来自事件自身，
//! 不污染后续 chunk 的回退归因（对齐上游 `applyUsageDelta`）。

use std::collections::HashMap;

/// 一次用量样本的四类 token 计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buckets {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Buckets {
    pub fn total(self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    pub(crate) fn add_into(&mut self, other: Buckets) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    pub(crate) fn sub_into(&mut self, other: Buckets) {
        self.input -= other.input;
        self.output -= other.output;
        self.cache_read -= other.cache_read;
        self.cache_write -= other.cache_write;
    }
}

/// prompt 侧（input + cache_read + cache_write）的缓存命中率（百分比，一位
/// 小数）；无任何 prompt token 时为 None。
pub fn cache_hit_rate(b: Buckets) -> Option<f64> {
    let prompt = b.input + b.cache_read + b.cache_write;
    if prompt == 0 {
        return None;
    }
    Some(((b.cache_read as f64 / prompt as f64) * 1000.0).round() / 10.0)
}

/// 加法式成本累加器（上游 v0.3.1 `lib/billing.js` 成本账的移植）：金额 +
/// 已定价/不可信样本计数。`incomplete > 0` 表示有量但无可信单价，渲染为
/// 「—」而不是低估（fail-closed）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CostAcc {
    pub usd: f64,
    pub priced: u32,
    pub incomplete: u32,
}

impl CostAcc {
    pub(crate) fn add(&mut self, sample: super::pricing::SampleCost) {
        if !sample.counted {
            return;
        }
        if sample.complete {
            self.usd += sample.usd;
            self.priced += 1;
        } else {
            self.incomplete += 1;
        }
    }

    /// 「替换去重」时回退一笔旧样本贡献（饱和递减，不出现负计数）。
    pub(crate) fn sub(&mut self, sample: super::pricing::SampleCost) {
        if !sample.counted {
            return;
        }
        if sample.complete {
            self.usd -= sample.usd;
            self.priced = self.priced.saturating_sub(1);
        } else {
            self.incomplete = self.incomplete.saturating_sub(1);
        }
    }

    pub(crate) fn merge(&mut self, other: CostAcc) {
        self.usd += other.usd;
        self.priced += other.priced;
        self.incomplete += other.incomplete;
    }

    pub(crate) fn complete(&self) -> bool {
        self.incomplete == 0
    }
}

/// 单个会话的增量折叠状态。
#[derive(Default)]
pub struct FoldState {
    pub(crate) source_file: String,
    /// 已折叠的按日条目。
    pub days: HashMap<String, DayEntry>,
    /// 最近一次样本（用于跨折叠边界的替换去重）。
    pub(crate) last_sample: Option<SampleRef>,
    /// 最近一次 request/header 归因的 provider/model。
    pub(crate) current_model: Option<String>,
    /// 最近一次路由归因（对齐上游 v0.3 `currentRoute`）：request/header 与
    /// assistant/message 都会推进，供「当前会话上下文」读取；只是
    /// current_model 游标的轻量投影，不是第二份用量账。
    pub current_route: Option<CurrentRoute>,
    /// 会话级统计折叠（见 `SessionStatsState`；与用量共用增量游标）。
    pub stats: SessionStatsState,
    /// 已消费的事件序号（增量游标）。
    pub consumed: u64,
    /// 折叠数据来源（对齐上游 v0.3 `state.kind`）。本壳只扫持久化日志，
    /// 恒为 `Persisted`；保留字段是为缓存结构与上游对齐。
    pub kind: FoldKind,
    /// 上次折叠时会话日志的文件长度（诊断用；短路判断改用 byte_offset）。
    pub file_len: u64,
    /// 已消费的**原始压缩字节**偏移：折叠只解码该偏移之后的完整 zstd 帧，
    /// 每轮开销 O(新增) 而非 O(全部历史)（撕裂尾帧不推进，等下一轮补全）。
    pub byte_offset: u64,
    /// 跨轮残留的半行（帧文本按换行切分后的尾段；完整行不会滞留）。
    pub(crate) pending_line: String,
}

impl FoldState {
    /// 重置增量折叠字段（对齐上游 `resetUsageState`）：保留 kind/file_len
    /// 等元数据，折叠游标全部清零，供整段重折前调用。
    pub(crate) fn reset_fold(&mut self) {
        self.days.clear();
        self.last_sample = None;
        self.current_model = None;
        self.current_route = None;
        self.stats = SessionStatsState::default();
        self.consumed = 0;
        self.byte_offset = 0;
        self.pending_line.clear();
    }

    /// 会话全期累计用量（跨日求和）：供状态栏在投影缺失时兜底
    /// `tokenUsage`（字段对齐上游投影的 wire 形状）。
    pub fn session_usage_totals(&self) -> (f64, f64, f64, f64) {
        let mut totals = Buckets::default();
        for day in self.days.values() {
            totals.add_into(day.totals);
        }
        (
            totals.input as f64,
            totals.output as f64,
            totals.cache_read as f64,
            totals.cache_write as f64,
        )
    }
}

/// 会话级统计的折叠状态与累计值。折叠语义逐条对齐上游 `sessionStats`
/// 投影（packages/session/session-stats/src/projection.ts 的 apply）：
/// `step/end`（不是 assistant/message）是步数权威——完成/失败/取消/
/// max-tokens 的步都恰落一条；模型时间配 `step/start → assistant/message`，
/// 首 token 取首个非空 delta（步内 `llm/retry` 存活），decode 覆盖有
/// outputTokens 上报的步，工具时间按 callId 配对 `tool/call → tool/result`；
/// 取消的步不组装消息，其部分流时间不计入任何时间项。
#[derive(Default)]
pub struct SessionStatsState {
    /// 至少有一个已闭合 step 的不同 turn 数。
    pub turns: u64,
    /// 已闭合的 step 数。
    pub steps: u64,
    /// 有消息组装的 step 的模型墙钟时间合计（毫秒）。
    pub llm_ms: i64,
    /// 配对成功的工具调用墙钟时间合计（毫秒）。
    pub tool_ms: i64,
    /// 有首 token 记录的步的首 token 延迟合计（毫秒）。
    pub ttft_ms: i64,
    /// 有首 token 记录的步数。
    pub ttft_steps: u64,
    /// 有 outputTokens 上报的步的解码墙钟时间合计（毫秒）。
    pub decode_ms: i64,
    /// 同一批步的提供商 output token 合计。
    pub decode_tokens: f64,
    /// 最近一次计数的 `step/end` 的 turn。
    pub(crate) last_turn: Option<u64>,
    /// 进行中 step 的边界事实（不在 step 内或消息已组装时为 None）。
    pub(crate) open_step: Option<OpenStep>,
    /// 已派发未落结果的工具调用时刻（callId → time）。
    pub(crate) pending_calls: HashMap<String, i64>,
}

/// `step/start` 打开的进行中 step 边界。
pub(crate) struct OpenStep {
    pub(crate) turn: u64,
    pub(crate) step: u64,
    pub(crate) start_time: i64,
    pub(crate) first_token_time: Option<i64>,
}

/// 折叠数据来源（对齐上游 `state.kind` 的 `"live" | "persisted"`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FoldKind {
    /// 内存事件流（上游 live 会话；本壳无此来源，仅为结构对齐保留）。
    Live,
    /// 持久化会话日志（本壳唯一来源，默认值）。
    #[default]
    Persisted,
}

impl FoldKind {
    /// 缓存落盘用的字符串形式（与上游 `kind` 字段同值）。
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            FoldKind::Live => "live",
            FoldKind::Persisted => "persisted",
        }
    }

    /// 从缓存字符串还原（无法识别按上游 `parseSession` 口径回落 persisted）。
    pub(crate) fn parse(raw: &str) -> Self {
        match raw {
            "live" => FoldKind::Live,
            _ => FoldKind::Persisted,
        }
    }
}

/// 最近一次路由归因（provider/model + 事件时刻；对齐上游 `currentRoute`）。
/// 无凭据、无监测细节，只是会话上下文的展示事实。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentRoute {
    pub provider_id: String,
    pub model: String,
    /// 归因事件的时刻（毫秒 epoch）；事件无时间戳时保留前一值。
    pub updated_at: Option<i64>,
}

/// 一个本地日历日（`YYYY-MM-DD`）的聚合值。
pub struct DayEntry {
    pub totals: Buckets,
    /// 日级成本账（与 totals 同源样本）。
    pub totals_cost: CostAcc,
    /// `provider/model` → 条目（仅含日密钥后三段：provider id 与 model id
    /// 以 `/` 连接）。
    pub models: HashMap<String, ModelEntry>,
}

/// 单个 `provider/model` 的 token 与成本账。
#[derive(Default)]
pub struct ModelEntry {
    pub buckets: Buckets,
    pub cost: CostAcc,
}

/// 「替换去重」所需的样本回执：键 + 归属日 + 归属模型 + 当时桶值与成本。
pub(crate) struct SampleRef {
    pub(crate) key: String,
    pub(crate) day: String,
    pub(crate) model: String,
    pub(crate) buckets: Buckets,
    pub(crate) cost: super::pricing::SampleCost,
}

/// 从事件解析出的用量样本。
struct Sample {
    key: String,
    buckets: Buckets,
    /// 样本自身携带的 provider/model 归因（可能为 None，走 current_model 回退）。
    model: Option<String>,
}

/// 事件 `type`、时间戳与完整 `data`（拥有所有权，解析时一次分配）。
#[derive(Clone)]
pub struct Event {
    /// 事件序号（会话内递增）。
    pub seq: u64,
    /// 毫秒 epoch；缺省时样本无法归日，跳过。
    pub time_ms: Option<i64>,
    pub kind: String,
    /// 完整 `data` 对象。
    pub data: Option<serde_json::Value>,
}

impl Event {
    /// 从一行 JSONL 文本解析（宽松：无法解析返回 None，由调用方跳过）。
    pub fn parse(line: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        Some(Self {
            seq: value.get("seq")?.as_u64()?,
            time_ms: value.get("time").and_then(|v| v.as_i64()),
            kind: value.get("type")?.as_str()?.to_string(),
            data: value.get("data").cloned(),
        })
    }

    /// 事件的 provider/model 归因（均返回 owned，避免借用 data 的生命周期纠缠）。
    fn attribution(&self) -> Option<(String, String)> {
        match self.kind.as_str() {
            "assistant/message" => {
                let source = self.data.as_ref()?.get("message")?.get("source")?;
                let provider = source.get("provider").and_then(|v| v.as_str());
                let model = source.get("model").and_then(|v| v.as_str());
                match (provider, model) {
                    (Some(p), Some(m)) if !p.is_empty() => Some((p.to_string(), m.to_string())),
                    (None | Some(_), Some(m)) => Some(("unknown".to_string(), m.to_string())),
                    _ => None,
                }
            }
            "request/header" => {
                let config = self.data.as_ref()?.get("header")?.get("config")?;
                let provider = config
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .filter(|p| !p.is_empty())
                    .unwrap_or("unknown");
                Some((
                    provider.to_string(),
                    config.get("model")?.as_str()?.to_string(),
                ))
            }
            _ => None,
        }
    }

    fn usage_sample(&self) -> Option<Sample> {
        let data = self.data.as_ref()?;
        let (key, usage) = match self.kind.as_str() {
            "assistant/chunk" => {
                let chunk = data.get("chunk")?;
                if chunk.get("type")?.as_str()? != "usage" {
                    return None;
                }
                let turn = data.get("turn")?.as_u64()?;
                let step = data.get("step")?.as_u64()?;
                (format!("{turn}:{step}"), chunk.get("usage")?)
            }
            "assistant/message" | "assistant/attempt" => {
                let usage = data.get("usage").filter(|v| v.is_object()).or_else(|| {
                    data.get("stream")?
                        .as_array()?
                        .iter()
                        .rev()
                        .find_map(|record| {
                            let chunk = record.get("chunk")?;
                            (record.get("type")?.as_str()? == "chunk"
                                && chunk.get("type")?.as_str()? == "usage")
                                .then(|| chunk.get("usage"))
                                .flatten()
                        })
                })?;
                let turn = data.get("turn")?.as_u64().unwrap_or(0);
                let step = data.get("step")?.as_u64().unwrap_or(0);
                (format!("{turn}:{step}"), usage)
            }
            _ => return None,
        };
        let buckets = Buckets {
            input: u64_of(usage, "inputTokens"),
            output: u64_of(usage, "outputTokens"),
            cache_read: u64_of(usage, "cacheReadTokens"),
            cache_write: u64_of(usage, "cacheWriteTokens"),
        };
        Some(Sample {
            key,
            buckets,
            model: self.attribution().map(|(p, m)| format!("{p}/{m}")),
        })
    }
}

fn u64_of(value: &serde_json::Value, field: &str) -> u64 {
    value
        .get(field)
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| value.get(field).and_then(|v| v.as_f64()).unwrap_or(0.0) as u64)
}

/// 本地日历日 `YYYY-MM-DD`（按本机时区，与会话日志「浏览器本地日」语义一致）。
///
/// 本机 UTC 偏移在进程内缓存一次（偏移极少变化；DST 切换至多造成边界时刻
/// 归日偏差一天，可接受）。Windows 走 `GetTimeZoneInformation`；其他平台走
/// `localtime_r`（含 DST 生效值，失败回退 UTC）。
pub fn day_key(time_ms: i64) -> String {
    let local_secs = time_ms.div_euclid(1000) + local_offset_seconds();
    let days = local_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 本机相对 UTC 的偏移秒数（正数 = 东半球），进程内缓存一次。
pub(crate) fn local_offset_seconds() -> i64 {
    use std::sync::OnceLock;
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(compute_local_offset_seconds)
}

#[cfg(windows)]
fn compute_local_offset_seconds() -> i64 {
    // Bias 是「UTC = local + Bias」中的分钟数（东半球为负），因此取反。
    // 当前生效的附加偏差按返回值区分：夏令时期间用 DaylightBias，
    // 其余（标准时/无夏令时）用 StandardBias。
    let mut info: windows_sys::Win32::System::Time::TIME_ZONE_INFORMATION =
        unsafe { std::mem::zeroed() };
    let result = unsafe { windows_sys::Win32::System::Time::GetTimeZoneInformation(&mut info) };
    if result == windows_sys::Win32::System::Time::TIME_ZONE_ID_INVALID {
        return 0;
    }
    // windows-sys 0.61 只导出 TIME_ZONE_ID_INVALID；DAYLIGHT 按 Win32 定义取常量值
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
    let active_bias = if result == TIME_ZONE_ID_DAYLIGHT {
        info.Bias + info.DaylightBias
    } else {
        info.Bias + info.StandardBias
    };
    -(active_bias as i64) * 60
}

#[cfg(not(windows))]
fn compute_local_offset_seconds() -> i64 {
    // POSIX `localtime_r`（线程安全）按当前时刻取本地偏移（含 DST 生效值）；
    // 失败回退 UTC。macOS / Linux 均提供；与 Windows 路径同为「进程内取一次」。
    unsafe {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as libc::time_t)
            .unwrap_or(0);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

/// Howard Hinnant 的 civil_from_days 算法（公历）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 把一段新事件折叠进状态（顺序执行、可跨多次调用复用增量游标）。
pub fn apply_delta(state: &mut FoldState, events: &[Event]) {
    for event in events {
        // 同一步重试是新的计费请求，不能用成功样本覆盖前次尝试的消耗。
        if event.kind == "llm/retry-started" {
            if let Some(data) = &event.data {
                if let (Some(turn), Some(step)) = (
                    data.get("turn").and_then(|v| v.as_u64()),
                    data.get("step").and_then(|v| v.as_u64()),
                ) {
                    if state
                        .last_sample
                        .as_ref()
                        .is_some_and(|s| s.key == format!("{turn}:{step}"))
                    {
                        state.last_sample = None;
                    }
                }
            }
        }
        // 对齐上游 v0.3（lib/usage.js applyUsageDelta）：request/header 与
        // assistant/message 都推进 current_route（实时路由上下文，事件无
        // 时间戳时 updated_at 保留前一值）；token 归因语义不变——
        // current_model 仍只在 request/header 更新，assistant/message 的
        // 归因来自事件自身 data.message.source，不污染后续 chunk 的回退归因。
        if matches!(event.kind.as_str(), "request/header" | "assistant/message") {
            if let Some((p, m)) = event.attribution() {
                if event.kind == "request/header" {
                    state.current_model = Some(format!("{p}/{m}"));
                }
                state.current_route = Some(CurrentRoute {
                    provider_id: p,
                    model: m,
                    updated_at: event
                        .time_ms
                        .or(state.current_route.as_ref().and_then(|r| r.updated_at)),
                });
            }
        }
        let Some(time) = event.time_ms else {
            continue;
        };
        apply_stats_event(&mut state.stats, event, time);
        let Some(sample) = event.usage_sample() else {
            continue;
        };
        let day = day_key(time);
        let (provider, model_id) = sample
            .model
            .clone()
            .or_else(|| state.current_model.clone())
            .and_then(|combined| {
                combined
                    .split_once('/')
                    .map(|(p, m)| (p.to_string(), m.to_string()))
            })
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
        let model = format!("{provider}/{model_id}");
        // 成本按「事件时刻 × 归因」估算（上游 estimateTokenCost；官方
        // DeepSeek 之外的归因未定价 → incomplete）。
        let cost = super::pricing::estimate_sample(&provider, &model_id, time, sample.buckets);
        // 同 key 重复样本：从原归属日/模型减去旧值（替换而非累加）。
        if let Some(prev) = &state.last_sample {
            if prev.key == sample.key {
                if let Some(entry) = state.days.get_mut(&prev.day) {
                    entry.totals.sub_into(prev.buckets);
                    entry.totals_cost.sub(prev.cost);
                    if let Some(me) = entry.models.get_mut(&prev.model) {
                        me.buckets.sub_into(prev.buckets);
                        me.cost.sub(prev.cost);
                    }
                }
            }
        }
        let entry = state.days.entry(day.clone()).or_insert_with(|| DayEntry {
            totals: Buckets::default(),
            totals_cost: CostAcc::default(),
            models: HashMap::new(),
        });
        entry.totals.add_into(sample.buckets);
        entry.totals_cost.add(cost);
        let me = entry.models.entry(model.clone()).or_default();
        me.buckets.add_into(sample.buckets);
        me.cost.add(cost);
        state.last_sample = Some(SampleRef {
            key: sample.key,
            day,
            model,
            buckets: sample.buckets,
            cost,
        });
    }
}

/// 单事件推进会话统计折叠（语义见 `SessionStatsState` 文档；`time` 为
/// 事件时间戳，与上游 `event.time` 同源）。
pub(crate) fn apply_stats_event(stats: &mut SessionStatsState, event: &Event, time: i64) {
    fn turn_step(data: Option<&serde_json::Value>) -> Option<(u64, u64)> {
        let data = data?;
        Some((data.get("turn")?.as_u64()?, data.get("step")?.as_u64()?))
    }
    let data = event.data.as_ref();
    match event.kind.as_str() {
        "step/start" => {
            if let Some((turn, step)) = turn_step(data) {
                stats.open_step = Some(OpenStep {
                    turn,
                    step,
                    start_time: time,
                    first_token_time: None,
                });
            }
        }
        "assistant/attempt" => {
            let Some(open) = &mut stats.open_step else {
                return;
            };
            let Some((turn, step)) = turn_step(data) else {
                return;
            };
            if open.turn != turn || open.step != step {
                return;
            }
            if open.first_token_time.is_none() {
                open.first_token_time = data
                    .and_then(|d| d.get("stream"))
                    .and_then(|s| s.as_array())
                    .and_then(|s| stream_first_token_time(s));
            }
        }
        "assistant/message" => {
            let Some(open) = stats.open_step.take() else {
                return;
            };
            let Some((turn, step)) = turn_step(data) else {
                // 坐标缺失：事件与打开的 step 无法配对，还原状态不动。
                stats.open_step = Some(open);
                return;
            };
            if open.turn != turn || open.step != step {
                stats.open_step = Some(open);
                return;
            }
            let first_token = open.first_token_time.or_else(|| {
                data.and_then(|d| d.get("stream"))
                    .and_then(|s| s.as_array())
                    .and_then(|s| stream_first_token_time(s))
            });
            stats.llm_ms += (time - open.start_time).max(0);
            if let Some(first) = first_token {
                stats.ttft_ms += (first - open.start_time).max(0);
                stats.ttft_steps += 1;
                if let Some(output) = data
                    .and_then(|d| d.get("usage"))
                    .and_then(|u| u.get("outputTokens"))
                    .and_then(|v| v.as_f64())
                    .filter(|v| v.is_finite() && *v >= 0.0)
                {
                    stats.decode_ms += (time - first).max(0);
                    stats.decode_tokens += output;
                }
            }
        }
        "tool/call" => {
            if let Some(call_id) = data.and_then(|d| d.get("callId")).and_then(|v| v.as_str()) {
                stats.pending_calls.insert(call_id.to_string(), time);
            }
        }
        "tool/result" => {
            // callId 在消息的 tool source 上（V3 与 V4 一等 tool-role 同位）。
            let call_id = data
                .and_then(|d| d.get("message"))
                .and_then(|m| m.get("source"))
                .and_then(|s| s.get("callId"))
                .and_then(|v| v.as_str());
            if let Some(call_id) = call_id {
                if let Some(dispatched) = stats.pending_calls.remove(call_id) {
                    stats.tool_ms += (time - dispatched).max(0);
                }
            }
        }
        "step/end" => {
            if let Some((turn, _step)) = turn_step(data) {
                stats.steps += 1;
                if stats.last_turn != Some(turn) {
                    stats.turns += 1;
                    stats.last_turn = Some(turn);
                }
                stats.open_step = None;
            }
        }
        // 结果总在其 turn 内落地；turn 结束仍未落地的调用属于取消/失败，
        // 清掉避免状态无限增长（上游同款）。
        "turn/end" => stats.pending_calls.clear(),
        _ => {}
    }
}

/// 流记录数组的首 token 时刻（对齐上游 `assistantStreamFirstTokenTime`）：
/// 松散 chunk 取首个 token delta 的 time；打包 run 取首个非空成员的重构
/// 时间（成员 k = time0 + 前 k 个 dt 之和），带名字的 tool-call run 记
/// time0。流无 token 时返回 None。
pub(crate) fn stream_first_token_time(stream: &[serde_json::Value]) -> Option<i64> {
    fn is_token_delta(chunk: &serde_json::Value) -> bool {
        match chunk.get("type").and_then(|v| v.as_str()) {
            Some("text-delta") | Some("reasoning-delta") => chunk
                .get("text")
                .and_then(|v| v.as_str())
                .is_some_and(|t| !t.is_empty()),
            Some("tool-call-delta") => {
                chunk
                    .get("argumentsDelta")
                    .and_then(|v| v.as_str())
                    .is_some_and(|a| !a.is_empty())
                    || chunk.get("name").is_some()
            }
            _ => false,
        }
    }
    fn first_member_time(run: &serde_json::Value) -> Option<i64> {
        let fragments = run
            .get("texts")
            .and_then(|v| v.as_array())
            .or_else(|| run.get("args").and_then(|v| v.as_array()))?;
        let dts = run.get("dt").and_then(|v| v.as_array())?;
        let mut time = run.get("time0")?.as_i64()?;
        for (index, fragment) in fragments.iter().enumerate() {
            if index > 0 {
                time += dts.get(index - 1)?.as_i64()?;
            }
            if fragment.as_str().is_some_and(|t| !t.is_empty()) {
                return Some(time);
            }
        }
        None
    }
    for record in stream {
        match record.get("type").and_then(|v| v.as_str()) {
            Some("chunk") => {
                let chunk = record.get("chunk")?;
                if is_token_delta(chunk) {
                    return record.get("time").and_then(|v| v.as_i64());
                }
            }
            Some("tool-call-chunks") => {
                if record.get("name").is_some() {
                    return record.get("time0").and_then(|v| v.as_i64());
                }
            }
            Some("text-chunks") | Some("reasoning-chunks") => {
                if let Some(time) = first_member_time(record) {
                    return Some(time);
                }
            }
            _ => {}
        }
    }
    None
}

/// 对外 wire 结构（序列化为 JSON 给前端）。
#[derive(serde::Serialize)]
pub struct UsageReport {
    pub unavailable_sessions: Vec<String>,
    pub days: Vec<DayReport>,
    pub total: TotalReport,
    /// 计算时刻（epoch 毫秒）。
    pub updated_at: u64,
}

#[derive(serde::Serialize)]
pub struct TotalReport {
    #[serde(flatten)]
    pub buckets: BucketReport,
    pub tokens: u64,
    pub cache_hit_rate: Option<f64>,
    /// 估算成本（USD）；`cost_complete == false` 表示含不可信样本，前端
    /// 应显示「—」而不是金额。
    pub cost_usd: f64,
    pub cost_complete: bool,
}

#[derive(serde::Serialize)]
pub struct DayReport {
    pub date: String,
    #[serde(flatten)]
    pub buckets: BucketReport,
    pub tokens: u64,
    pub cache_hit_rate: Option<f64>,
    pub cost_usd: f64,
    pub cost_complete: bool,
    pub models: Vec<ModelReport>,
}

#[derive(serde::Serialize)]
pub struct ModelReport {
    pub model: String,
    #[serde(flatten)]
    pub buckets: BucketReport,
    pub tokens: u64,
    pub cache_hit_rate: Option<f64>,
    pub cost_usd: f64,
    pub cost_complete: bool,
}

#[derive(serde::Serialize)]
pub struct BucketReport {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl From<Buckets> for BucketReport {
    fn from(b: Buckets) -> Self {
        BucketReport {
            input_tokens: b.input,
            output_tokens: b.output,
            cache_read_tokens: b.cache_read,
            cache_write_tokens: b.cache_write,
        }
    }
}

/// 把（可能多会话的）折叠状态合并成一份全局按日报告。
pub fn render(days: &HashMap<String, DayEntry>, updated_at: u64) -> UsageReport {
    let mut day_reports: Vec<DayReport> = days
        .iter()
        .map(|(date, entry)| {
            let mut models: Vec<ModelReport> = entry
                .models
                .iter()
                .map(|(model, me)| ModelReport {
                    model: model.clone(),
                    buckets: me.buckets.into(),
                    tokens: me.buckets.total(),
                    cache_hit_rate: cache_hit_rate(me.buckets),
                    cost_usd: me.cost.usd,
                    cost_complete: me.cost.complete(),
                })
                .filter(|m| m.tokens > 0)
                .collect();
            models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.model.cmp(&b.model)));
            DayReport {
                date: date.clone(),
                buckets: entry.totals.into(),
                tokens: entry.totals.total(),
                cache_hit_rate: cache_hit_rate(entry.totals),
                cost_usd: entry.totals_cost.usd,
                cost_complete: entry.totals_cost.complete(),
                models,
            }
        })
        .collect();
    day_reports.sort_by(|a, b| a.date.cmp(&b.date));

    let mut total = Buckets::default();
    let mut total_cost = CostAcc::default();
    for entry in days.values() {
        total.add_into(entry.totals);
        total_cost.merge(entry.totals_cost);
    }
    UsageReport {
        unavailable_sessions: Vec::new(),
        days: day_reports,
        total: TotalReport {
            buckets: total.into(),
            tokens: total.total(),
            cache_hit_rate: cache_hit_rate(total),
            cost_usd: total_cost.usd,
            cost_complete: total_cost.complete(),
        },
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats_event(seq: u64, time: i64, kind: &str, data: serde_json::Value) -> Event {
        Event::parse(
            &serde_json::json!({"seq": seq, "time": time, "type": kind, "data": data}).to_string(),
        )
        .unwrap()
    }

    #[test]
    fn stats_fold_matches_upstream_projection_semantics() {
        let mut stats = SessionStatsState::default();
        // turn 1 / step 1：工具配对 + 首 token + usage 上报的完整步
        apply_stats_event(
            &mut stats,
            &stats_event(
                1,
                100,
                "step/start",
                serde_json::json!({"turn": 1, "step": 1}),
            ),
            100,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(2, 110, "tool/call", serde_json::json!({"callId": "c1"})),
            110,
        );
        // 空文本 delta 不算首 token，非空 delta 才算
        apply_stats_event(
            &mut stats,
            &stats_event(
                3,
                115,
                "assistant/attempt",
                serde_json::json!({"turn": 1, "step": 1, "stream": [
                    {"type": "chunk", "time": 115, "chunk": {"type": "text-delta", "text": ""}},
                    {"type": "chunk", "time": 120, "chunk": {"type": "text-delta", "text": "你"}},
                ]}),
            ),
            115,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(
                4,
                150,
                "tool/result",
                serde_json::json!({"turn": 1, "step": 1, "message": {"role": "tool",
                    "source": {"kind": "tool", "callId": "c1"}}}),
            ),
            150,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(
                5,
                200,
                "assistant/message",
                serde_json::json!({"turn": 1, "step": 1,
                    "usage": {"outputTokens": 50}, "stream": []}),
            ),
            200,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(
                6,
                200,
                "step/end",
                serde_json::json!({"turn": 1, "step": 1}),
            ),
            200,
        );
        // turn 2 / step 1：无首 token、无 usage、含未落地的工具调用
        apply_stats_event(
            &mut stats,
            &stats_event(
                7,
                300,
                "step/start",
                serde_json::json!({"turn": 2, "step": 1}),
            ),
            300,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(8, 310, "tool/call", serde_json::json!({"callId": "c2"})),
            310,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(
                9,
                350,
                "assistant/message",
                serde_json::json!({"turn": 2, "step": 1}),
            ),
            350,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(
                10,
                350,
                "step/end",
                serde_json::json!({"turn": 2, "step": 1}),
            ),
            350,
        );
        // turn 结束清掉未落地调用；同 turn 第二个 step 不再计 turn
        apply_stats_event(
            &mut stats,
            &stats_event(11, 360, "turn/end", serde_json::json!({"turn": 2})),
            360,
        );
        assert_eq!(stats.turns, 2);
        assert_eq!(stats.steps, 2);
        assert_eq!(stats.llm_ms, (200 - 100) + (350 - 300));
        assert_eq!(stats.tool_ms, 150 - 110);
        assert_eq!(stats.ttft_ms, 120 - 100);
        assert_eq!(stats.ttft_steps, 1);
        assert_eq!(stats.decode_ms, 200 - 120);
        assert_eq!(stats.decode_tokens, 50.0);
        assert!(stats.pending_calls.is_empty());
        // 坐标不匹配的 assistant/message 不动打开中的 step
        apply_stats_event(
            &mut stats,
            &stats_event(
                12,
                400,
                "step/start",
                serde_json::json!({"turn": 3, "step": 1}),
            ),
            400,
        );
        apply_stats_event(
            &mut stats,
            &stats_event(
                13,
                410,
                "assistant/message",
                serde_json::json!({"turn": 9, "step": 9}),
            ),
            410,
        );
        assert!(stats.open_step.is_some());
    }

    #[test]
    fn stream_first_token_covers_chunks_and_packed_runs() {
        // 松散：空文本与 block/usage 不算，非空 delta / 带 name 的 tool-call 算
        let stream = serde_json::json!([
            {"type": "chunk", "time": 10, "chunk": {"type": "text-delta", "text": ""}},
            {"type": "chunk", "time": 11, "chunk": {"type": "usage"}},
            {"type": "chunk", "time": 12, "chunk": {"type": "tool-call-delta", "argumentsDelta": "", "name": "fs"}}
        ])
        .as_array()
        .unwrap()
        .clone();
        assert_eq!(stream_first_token_time(&stream), Some(12));
        // 打包 text run：成员时间 = time0 + 前缀 dt，首成员空文本跳过
        let run = serde_json::json!([
            {"type": "text-chunks", "time0": 100, "dt": [5, 7], "texts": ["", "ab", "cd"]}
        ])
        .as_array()
        .unwrap()
        .clone();
        assert_eq!(stream_first_token_time(&run), Some(100 + 5));
        // 带 name 的 tool-call run 取 time0
        let tool_run = serde_json::json!([
            {"type": "tool-call-chunks", "time0": 200, "dt": [1], "args": ["x"], "name": "bash"}
        ])
        .as_array()
        .unwrap()
        .clone();
        assert_eq!(stream_first_token_time(&tool_run), Some(200));
        // reasoning 打包 run 无非空成员 → None
        let empty = serde_json::json!([
            {"type": "reasoning-chunks", "time0": 300, "dt": [], "texts": ["", ""]}
        ])
        .as_array()
        .unwrap()
        .clone();
        assert_eq!(stream_first_token_time(&empty), None);
    }

    fn event(seq: u64, time: i64, kind: &str, data: serde_json::Value) -> String {
        serde_json::json!({"seq": seq, "time": time, "type": kind, "data": data}).to_string()
    }

    fn usage_chunk(seq: u64, time: i64, turn: u64, step: u64, input: u64, output: u64) -> String {
        event(
            seq,
            time,
            "assistant/chunk",
            serde_json::json!({"turn": turn, "step": step, "chunk": {
                "type": "usage", "usage": {"inputTokens": input, "outputTokens": output}
            }}),
        )
    }

    const DAY1: i64 = 1_780_000_000_000; // ~2026-05-31 (TBD exact)
    const DAY1B: i64 = 1_780_000_000_000 + 86_400_000;

    #[test]
    fn v2_usage_keeps_retried_attempts_and_uses_last_stream_sample() {
        let stream = |input| {
            serde_json::json!({"turn":1,"step":1,"stream":[
                {"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":1}}},
                {"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":input}}}
            ]})
        };
        let mut state = FoldState::default();
        let parse = |seq, kind, data| Event::parse(&event(seq, DAY1, kind, data)).unwrap();
        apply_delta(&mut state, &[parse(1, "assistant/attempt", stream(100))]);
        assert_eq!(
            state.days.values().map(|d| d.totals.input).sum::<u64>(),
            100
        );
        apply_delta(
            &mut state,
            &[
                parse(
                    2,
                    "llm/retry-started",
                    serde_json::json!({"turn":1,"step":1}),
                ),
                parse(3, "assistant/message", stream(200)),
            ],
        );
        assert_eq!(
            state.days.values().map(|d| d.totals.input).sum::<u64>(),
            300
        );
        let mut final_sample = stream(999);
        final_sample["usage"] = serde_json::json!({"inputTokens":250});
        apply_delta(&mut state, &[parse(4, "assistant/message", final_sample)]);
        assert_eq!(
            state.days.values().map(|d| d.totals.input).sum::<u64>(),
            350
        );
    }

    #[test]
    fn cost_accumulates_replaces_and_fails_closed() {
        // DAY1 处于官方 DeepSeek 平价期（时间带 v1 之前）
        let msg = |input: u64, output: u64| {
            event(
                1,
                DAY1,
                "assistant/message",
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {"source": {"provider": "deepseek", "model": "deepseek-v4-flash"}},
                    "usage": {"inputTokens": input, "outputTokens": output}
                }),
            )
        };
        let mut state = FoldState::default();
        let events: Vec<Event> = [msg(1_000_000, 1_000_000)]
            .iter()
            .map(|l| Event::parse(l).unwrap())
            .collect();
        apply_delta(&mut state, &events);
        // 1M input × 0.14 + 1M output × 0.28 = 0.42 USD
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert!((entry.totals_cost.usd - 0.42).abs() < 1e-9);
        assert!(entry.totals_cost.complete());

        // 同 (turn,step) 替换为 2M input：旧成本回退、新成本入账
        let events: Vec<Event> = [msg(2_000_000, 1_000_000)]
            .iter()
            .map(|l| Event::parse(l).unwrap())
            .collect();
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert!(
            (entry.totals_cost.usd - 0.56).abs() < 1e-9,
            "usd={}",
            entry.totals_cost.usd
        );

        // 无归因样本（unknown/unknown）有量但未定价 → 日成本 fail-closed
        let events: Vec<Event> = [usage_chunk(2, DAY1, 2, 1, 10, 5)]
            .iter()
            .map(|l| Event::parse(l).unwrap())
            .collect();
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert!(!entry.totals_cost.complete());
        assert_eq!(entry.totals_cost.incomplete, 1);

        // render 输出成本字段且 total 继承 fail-closed
        let report = render(&state.days, 0);
        let day = report
            .days
            .iter()
            .find(|d| d.date == day_key(DAY1))
            .unwrap();
        assert!(!day.cost_complete);
        assert!(!report.total.cost_complete);
    }

    #[test]
    fn cache_write_samples_mark_cost_incomplete() {
        let msg = event(
            1,
            DAY1,
            "assistant/message",
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": {"source": {"provider": "deepseek", "model": "deepseek-v4-pro"}},
                "usage": {"inputTokens": 100, "outputTokens": 100, "cacheWriteTokens": 50}
            }),
        );
        let mut state = FoldState::default();
        let events: Vec<Event> = [msg].iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert!(
            !entry.totals_cost.complete(),
            "cacheWrite 无官方价 → incomplete"
        );
        assert_eq!(entry.totals_cost.usd, 0.0);
    }

    #[test]
    fn same_turn_step_replaces_instead_of_double_counting() {
        let mut state = FoldState::default();
        let lines = [
            usage_chunk(1, DAY1, 1, 1, 10, 5),
            usage_chunk(2, DAY1 + 1000, 1, 1, 40, 20), // 同 (turn,step) 替换
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert_eq!(entry.totals.total(), 60); // 不是 75
        assert_eq!(entry.totals.input, 40);
        assert_eq!(entry.totals.output, 20);
    }

    #[test]
    fn attribution_falls_back_to_request_header_model() {
        let mut state = FoldState::default();
        let lines = [
            event(
                1,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "oz", "model": "gpt-x"}}}),
            ),
            usage_chunk(2, DAY1, 1, 1, 100, 0),
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert!(entry.models.contains_key("oz/gpt-x"));
    }

    #[test]
    fn message_attribution_does_not_leak_into_following_chunks() {
        // 上游语义：assistant/message 用自身 source 归因，但不更新回退游标；
        // 其后无新 header 的 usage chunk 仍归于最近一次 request/header 的模型。
        let mut state = FoldState::default();
        let lines = [
            event(
                1,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "oz", "model": "gpt-x"}}}),
            ),
            event(
                2,
                DAY1,
                "assistant/message",
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {"source": {"provider": "kimi", "model": "k2"}},
                    "usage": {"inputTokens": 10, "outputTokens": 5}
                }),
            ),
            usage_chunk(3, DAY1, 2, 1, 100, 50), // message 之后、无新 header
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert_eq!(entry.models.get("kimi/k2").unwrap().buckets.total(), 15);
        assert_eq!(entry.models.get("oz/gpt-x").unwrap().buckets.total(), 150);
        assert_eq!(entry.models.len(), 2);
    }

    #[test]
    fn current_route_tracks_messages_while_current_model_stays_header_driven() {
        // 上游 v0.3：current_route 在 request/header 与 assistant/message 上
        // 都推进（实时路由上下文），current_model 仍只随 request/header。
        let mut state = FoldState::default();
        let lines = [
            event(
                1,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "oz", "model": "gpt-x"}}}),
            ),
            event(
                2,
                DAY1 + 1000,
                "assistant/message",
                serde_json::json!({
                    "turn": 1, "step": 1,
                    "message": {"source": {"provider": "kimi", "model": "k2"}},
                    "usage": {"inputTokens": 10, "outputTokens": 5}
                }),
            ),
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        assert_eq!(state.current_model.as_deref(), Some("oz/gpt-x"));
        let route = state.current_route.as_ref().unwrap();
        assert_eq!(route.provider_id, "kimi");
        assert_eq!(route.model, "k2");
        assert_eq!(route.updated_at, Some(DAY1 + 1000));
    }

    #[test]
    fn current_route_keeps_previous_updated_at_when_event_has_no_time() {
        // 上游 v0.3：事件无时间戳时 current_route 仍推进，updated_at 保留
        // 前一值（`Number.isFinite(event.time) ? event.time : prev ?? null`）。
        let mut state = FoldState::default();
        let lines = [
            event(
                1,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "oz", "model": "gpt-x"}}}),
            ),
            // 无 time 字段的 message：路由切换生效，时刻保留 header 的值。
            serde_json::json!({
                "seq": 2, "type": "assistant/message",
                "data": {"turn": 1, "step": 1,
                    "message": {"source": {"provider": "kimi", "model": "k2"}},
                    "usage": {"inputTokens": 1, "outputTokens": 1}}
            })
            .to_string(),
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        let route = state.current_route.as_ref().unwrap();
        assert_eq!(route.provider_id, "kimi");
        assert_eq!(route.updated_at, Some(DAY1));
    }

    #[test]
    fn reset_fold_clears_fold_cursors_but_keeps_metadata() {
        // 对齐上游 resetUsageState：折叠游标清零，kind/file_len 元数据保留。
        let mut state = FoldState {
            kind: FoldKind::Live,
            file_len: 4096,
            ..Default::default()
        };
        let lines = [
            event(
                1,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "oz", "model": "gpt-x"}}}),
            ),
            usage_chunk(2, DAY1, 1, 1, 10, 5),
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        assert!(state.current_route.is_some());
        state.reset_fold();
        assert!(state.days.is_empty());
        assert!(state.current_route.is_none());
        assert_eq!(state.consumed, 0);
        assert_eq!(state.kind, FoldKind::Live);
        assert_eq!(state.file_len, 4096);
    }

    #[test]
    fn day_key_uses_local_calendar_day_boundaries() {
        // 同一天的两个时刻应落在同一 key；跨 +1 天（86400s）落在另一 key。
        assert_eq!(day_key(DAY1), day_key(DAY1 + 1000));
        assert_ne!(day_key(DAY1), day_key(DAY1B));
    }

    /// 非 Windows 偏移取值走 `localtime_r`：固定时区环境下应得到其标准
    /// 偏移（Asia/Shanghai = +8h）。Windows 路径由全平台 CI 的格式/Clippy
    /// 覆盖，数值断言只在 Unix 跑。
    #[cfg(all(test, unix))]
    #[test]
    fn unix_local_offset_respects_tz() {
        // libc crate 未绑定 tzset（macOS/Linux 的 Unit tests 曾因此编译失败），
        // 直接声明 POSIX 原型：无参无返回，仅重载 TZ 相关内部状态。
        extern "C" {
            fn tzset();
        }
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TZ").ok();
        std::env::set_var("TZ", "Asia/Shanghai");
        unsafe { tzset() };
        assert_eq!(compute_local_offset_seconds(), 28_800);
        match prev {
            Some(v) => std::env::set_var("TZ", v),
            None => std::env::remove_var("TZ"),
        }
        unsafe { tzset() };
    }

    #[test]
    fn v3_session_log_folds_through_the_same_paths() {
        // Session Log V3（dsh ≥0.1.5）物理编码与 V2 相同；差异是语义层的：
        // header 行（type=session，无 seq）应被 Event::parse 跳过；新事件
        // 类型（system/message 等）不被消费直接略过；assistant/message 的
        // usage/message.source 与 request/header 的 header.config 路径不变。
        let mut state = FoldState::default();
        let lines = [
            r#"{"type":"session","version":3,"id":"session-x","createdAt":1,"cwd":"D:\\p","isSeeded":false,"delegationDepth":0,"agentPreset":"standard"}"#.to_string(),
            event(
                1,
                DAY1,
                "system/message",
                serde_json::json!({"turn": 0, "step": 0,
                    "message": {"id": "m", "role": "system", "source": {"kind": "plugin", "plugin": "p"}, "content": []}}),
            ),
            event(
                2,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "ibrain", "model": "claude-for-deepseek-v4-pro"}}}),
            ),
            serde_json::json!({"seq": 3, "time": DAY1, "type": "assistant/message", "data": {
                "turn": 1, "step": 1,
                "message": {"source": {"kind": "model", "provider": "ibrain", "model": "claude-for-deepseek-v4-pro"}},
                "stream": [{"type": "chunk", "chunk": {"type": "text-delta", "text": "hi"}}],
                "usage": {"inputTokens": 11662, "outputTokens": 344}
            }}).to_string(),
        ];
        let events: Vec<Event> = lines.iter().filter_map(|l| Event::parse(l)).collect();
        // header 行无 seq → None 被过滤；system/message 不产生样本
        assert_eq!(events.len(), 3);
        apply_delta(&mut state, &events);
        let entry = state.days.get(&day_key(DAY1)).unwrap();
        assert_eq!(entry.totals.input, 11662);
        assert_eq!(entry.totals.output, 344);
        assert_eq!(
            entry
                .models
                .get("ibrain/claude-for-deepseek-v4-pro")
                .unwrap()
                .buckets
                .total(),
            11662 + 344
        );
        assert_eq!(
            state.current_route.as_ref().unwrap().model,
            "claude-for-deepseek-v4-pro"
        );
    }

    #[test]
    fn render_sorts_days_and_filters_zero_model_rows() {
        let mut state = FoldState::default();
        let lines = [
            event(
                1,
                DAY1,
                "request/header",
                serde_json::json!({"header": {"config": {"provider": "a", "model": "m1"}}}),
            ),
            usage_chunk(2, DAY1, 1, 1, 5, 5),
            usage_chunk(3, DAY1 + 1000, 2, 1, 1, 1),
        ];
        let events: Vec<Event> = lines.iter().map(|l| Event::parse(l).unwrap()).collect();
        apply_delta(&mut state, &events);
        let report = render(&state.days, 0);
        assert_eq!(report.days.len(), 1);
        assert_eq!(report.total.tokens, 12);
    }
}
