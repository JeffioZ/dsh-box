//! 会话 Token 用量聚合（历史/跨会话）与供应商账户监测。
//!
//! 数据只走薄外壳的合法通道：本地会话日志（`$DSH_HOME/sessions/*.jsonl.zstd`）
//! 与 dsh 配置文件（`settings.yaml` 的 `llm-deepseek`/`llm-pi-ai` 段 +
//! `.credentials.yaml`）。不注入 dsh、不改 dsh 内核。

mod aggregate;
mod balance;
mod cache;
mod live;
mod log;
mod providers;
mod subscriptions;

pub use aggregate::{render, FoldState, UsageReport};
pub use balance::AccountSnapshot;
pub(crate) use live::{
    current_session_id, refresh_once, session_activity, snapshot, start_live_rate, start_periodic,
    StatsPayload,
};
pub use subscriptions::SubscriptionSnapshot;

/// 枚举已配置供应商路由并查询各账户余额（阶段 2 入口）。
pub fn accounts(config: &crate::app_state::Config) -> Result<Vec<AccountSnapshot>, String> {
    Ok(providers::configured_routes(config)
        .into_iter()
        .map(|route| balance::query_route(config, &route))
        .collect())
}

/// 查询全部订阅额度适配器（阶段 3 入口）。
pub fn subscriptions(config: &crate::app_state::Config) -> Vec<SubscriptionSnapshot> {
    subscriptions::subscriptions(config)
}

/// 把一次会话日志的**新增**事件折叠进状态（增量：只应用 `seq > consumed`）。
///
/// 会话日志为追加式：全量解码后只折叠游标之后的事件；游标前的样本已经
/// 计入 `state.days`。`last_sample` 跨折叠边界保留，保证同一 `(turn, step)`
/// 的替换语义在分次折叠时仍然精确。
pub fn fold_log(state: &mut FoldState, path: &std::path::Path) -> Result<(), String> {
    let text = log::read_full(path)?;
    let events: Vec<aggregate::Event> = text.lines().filter_map(aggregate::Event::parse).collect();
    // 日志被截断/重建（事件数少于游标）——退回整段重折。
    let max_seq = events.last().map(|e| e.seq).unwrap_or(0);
    if max_seq < state.consumed {
        state.days.clear();
        state.last_sample = None;
        state.current_model = None;
        state.consumed = 0;
    }
    let fresh: Vec<aggregate::Event> = events
        .into_iter()
        .filter(|e| e.seq > state.consumed)
        .collect();
    aggregate::apply_delta(state, &fresh);
    if let Some(last) = fresh.last() {
        state.consumed = last.seq;
    }
    Ok(())
}

/// 枚举所有本地会话日志路径。
pub fn list_session_logs(config: &crate::app_state::Config) -> Vec<(String, std::path::PathBuf)> {
    log::list_sessions(config)
}

/// 聚合全部本地会话并渲染当前用量报告（只读入口）。
///
/// 增量折叠状态落到 `$DSH_HOME/storages/usage-stats-cache.json`；消失的
/// 会话从缓存剔除；损坏/版本不符静默退回全量重折。
pub fn report(config: &crate::app_state::Config) -> Result<UsageReport, String> {
    let mut sessions = cache::load(config);
    let mut seen = std::collections::HashSet::new();
    for (id, path) in list_session_logs(config) {
        seen.insert(id.clone());
        let state = sessions.entry(id.clone()).or_default();
        if let Err(e) = fold_log(state, &path) {
            crate::logging::log(&format!("usage: 会话 {id} 聚合失败：{e}"));
        }
    }
    sessions.retain(|id, _| seen.contains(id));
    if let Err(e) = cache::save(config, &sessions) {
        crate::logging::log(&format!("usage: 保存聚合缓存失败：{e}"));
    }

    // 合并各会话的按日聚合为全局报告。
    let mut days = std::collections::HashMap::new();
    for state in sessions.values() {
        merge_days(&mut days, &state.days);
    }
    let updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(render(&days, updated_at))
}

fn merge_days(
    target: &mut std::collections::HashMap<String, aggregate::DayEntry>,
    source: &std::collections::HashMap<String, aggregate::DayEntry>,
) {
    use aggregate::Buckets;
    for (day, entry) in source {
        let t = target
            .entry(day.clone())
            .or_insert_with(|| aggregate::DayEntry {
                totals: Buckets::default(),
                models: std::collections::HashMap::new(),
            });
        t.totals.add_into(entry.totals);
        for (model, b) in &entry.models {
            t.models.entry(model.clone()).or_default().add_into(*b);
        }
    }
}
