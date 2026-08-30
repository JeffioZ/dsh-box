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
    /// 已消费的事件序号（增量游标）。
    pub consumed: u64,
    /// 折叠数据来源（对齐上游 v0.3 `state.kind`）。本壳只扫持久化日志，
    /// 恒为 `Persisted`；保留字段是为缓存结构与上游对齐。
    pub kind: FoldKind,
    /// 上次折叠时会话日志的文件长度：追加式日志 len 未变即无新事件，
    /// `fold_log` 据此跳过全量解码（serde 默认值兼容旧缓存）。
    pub file_len: u64,
}

impl FoldState {
    /// 重置增量折叠字段（对齐上游 `resetUsageState`）：保留 kind/file_len
    /// 等元数据，折叠游标全部清零，供整段重折前调用。
    pub(crate) fn reset_fold(&mut self) {
        self.days.clear();
        self.last_sample = None;
        self.current_model = None;
        self.current_route = None;
        self.consumed = 0;
    }
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
            seq: value.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
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
            "assistant/message" => {
                let usage = data.get("usage")?;
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
fn local_offset_seconds() -> i64 {
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

/// 对外 wire 结构（序列化为 JSON 给前端）。
#[derive(serde::Serialize)]
pub struct UsageReport {
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
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TZ").ok();
        std::env::set_var("TZ", "Asia/Shanghai");
        unsafe { libc::tzset() };
        assert_eq!(compute_local_offset_seconds(), 28_800);
        match prev {
            Some(v) => std::env::set_var("TZ", v),
            None => std::env::remove_var("TZ"),
        }
        unsafe { libc::tzset() };
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
