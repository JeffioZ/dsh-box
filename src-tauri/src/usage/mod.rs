//! 会话 Token 用量聚合（历史/跨会话）与供应商账户监测。
//!
//! 数据只走薄外壳的合法通道：本地会话日志（`$DSH_HOME/sessions/*.jsonl.zstd`）
//! 与 dsh 配置文件（`settings.yaml` 的 `llm-deepseek`/`llm-pi-ai` 段 +
//! `.credentials.yaml`）。不注入 dsh、不改 dsh 内核。

mod aggregate;
mod balance;
mod cache;
mod dev_fake;
pub(crate) mod export;
mod live;
mod log;
mod monitor;
mod pricing;
mod providers;
mod subscriptions;

pub use aggregate::{render, FoldState, UsageReport};
pub use balance::AccountSnapshot;
#[cfg(test)]
pub(crate) use balance::Balance;
pub(crate) use live::{
    current_session_id, refresh_once, session_activity, snapshot, start_live_rate, start_periodic,
    StatsPayload,
};
pub(crate) use log::session_log_path;
pub(crate) use monitor::{
    cached_accounts, cached_deepseek, cached_subscriptions, request_account_refresh,
    start_account_monitor,
};
#[cfg(test)]
pub(crate) use monitor::{set_cache_for_test, CACHE_TEST_LOCK};
pub use subscriptions::SubscriptionSnapshot;

/// 本地今天的 `YYYY-MM-DD`（导出文件名用；假数据模式同样按真实日期）。
pub fn day_key_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    aggregate::day_key(now)
}

/// 枚举已配置供应商路由并查询各账户余额（阶段 2 入口）。
pub fn accounts(config: &crate::app_state::Config) -> Result<Vec<AccountSnapshot>, String> {
    if dev_fake::enabled() {
        return Ok(dev_fake::accounts());
    }
    Ok(providers::configured_routes(config)
        .into_iter()
        .map(|route| balance::query_route(config, &route))
        .collect())
}

/// 查询全部订阅额度适配器（阶段 3 入口）。
pub fn subscriptions(config: &crate::app_state::Config) -> Vec<SubscriptionSnapshot> {
    if dev_fake::enabled() {
        return dev_fake::subscriptions();
    }
    subscriptions::subscriptions(config)
}

/// 把一次会话日志的**新增**事件折叠进状态（增量：只应用 `seq > consumed`）。
///
/// 会话日志为追加式的独立 zstd 帧序列：按 `byte_offset` 只解码上次消费之后
/// 的**完整**新帧（撕裂尾帧本轮跳过、下一轮补全），每轮开销 O(新增数据)
/// 而非 O(全部历史)——此前每次都对整个文件全量解压+解析再按游标过滤，
/// 长会话期间每 2-5 秒重复重算全部历史。`last_sample` 跨折叠边界保留，
/// 保证同一 `(turn, step)` 的替换语义在分次折叠时仍然精确。
///
/// 重建兜底：文件变短（截断/重建）或新事件 seq 回退（同长度重建）时，
/// 退回一次性整段重折，旧聚合清零重来。
pub fn fold_log(state: &mut FoldState, path: &std::path::Path) -> Result<(), String> {
    let file_len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if state.byte_offset == file_len {
        return Ok(());
    }
    if file_len < state.byte_offset {
        // 截断/重建为更短文件：清零游标后走下面的增量路径从 0 重折
        state.reset_fold();
    }
    if state.byte_offset > 0
        && state.byte_offset < file_len
        && !log::starts_with_frame_magic(path, state.byte_offset)?
    {
        // 偏移未落在帧边界：文件被原地重写（同长/更长但内容不同）。
        // 若继续增量解码，偏移落在新帧中间、伪 magic 扫描会跳过帧头
        // "恢复"出半截事件流，seq 回退检测也会失明——必须整段重折。
        return refold_full(state, path, file_len);
    }
    let (new_text, new_offset) = log::read_frames_from(path, state.byte_offset)?;
    // 拼上跨轮残留的半行后按行切分；尾段（无换行结尾）留待下一轮
    let mut combined = std::mem::take(&mut state.pending_line);
    combined.push_str(&new_text);
    let mut lines: Vec<&str> = combined.split('\n').collect();
    if let Some(tail) = lines.pop() {
        state.pending_line = tail.to_string();
    }
    let mut fresh = Vec::new();
    let mut seq_regressed = false;
    for line in lines {
        if let Some(event) = aggregate::Event::parse(line) {
            if event.seq < state.consumed {
                seq_regressed = true;
                break;
            }
            if event.seq > state.consumed {
                fresh.push(event);
            }
        }
    }
    if seq_regressed {
        // 同长度/更长但 seq 重启：日志被重建，一次性整段重折
        return refold_full(state, path, file_len);
    }
    aggregate::apply_delta(state, &fresh);
    if let Some(last) = fresh.last() {
        state.consumed = last.seq;
    }
    // 本壳唯一的数据来源是持久化会话日志（对齐上游 state.kind 的
    // live/persisted 标记；无 live 内存事件源，恒为 Persisted）。
    state.kind = aggregate::FoldKind::Persisted;
    state.byte_offset = new_offset;
    state.file_len = file_len;
    Ok(())
}

/// 一次性整段重折（重建兜底）：清零游标后全量读取解码。
fn refold_full(state: &mut FoldState, path: &std::path::Path, file_len: u64) -> Result<(), String> {
    state.reset_fold();
    let text = log::read_full(path)?;
    let fresh: Vec<aggregate::Event> = text
        .lines()
        .filter_map(aggregate::Event::parse)
        .filter(|e| e.seq > state.consumed)
        .collect();
    aggregate::apply_delta(state, &fresh);
    if let Some(last) = fresh.last() {
        state.consumed = last.seq;
    }
    state.kind = aggregate::FoldKind::Persisted;
    state.byte_offset = file_len;
    state.file_len = file_len;
    Ok(())
}

/// 枚举所有本地会话日志路径。
pub fn list_session_logs(config: &crate::app_state::Config) -> Vec<(String, std::path::PathBuf)> {
    log::list_sessions(config)
}

/// 当前会话上下文（序列化给前端）。三个字段名是前端契约（snake_case），
/// 不得更改；无活动会话或会话尚无路由归因时全部为 null。
#[derive(serde::Serialize)]
pub struct SessionContext {
    pub route_id: Option<String>,
    pub display_name: Option<String>,
    pub model: Option<String>,
}

/// 读取当前会话的路由上下文（只读入口）。
///
/// 数据路径：live RPC 推断当前会话 id → 增量折叠该会话日志拿到最新
/// `current_route`（缓存命中时代价极小）→ 按已配置路由解析展示名
/// （对齐上游 v0.3 session-context 端点：折叠状态是唯一依据，无路由
/// 归因时返回全 null，不猜、不伪造）。
pub fn session_context(config: &crate::app_state::Config) -> SessionContext {
    if dev_fake::enabled() {
        return dev_fake::session_context();
    }
    let Some(session_id) = live::current_session_id(config) else {
        return empty_context();
    };
    let mut sessions = cache::load(config);
    if let Some((_, path)) = list_session_logs(config)
        .into_iter()
        .find(|(id, _)| *id == session_id)
    {
        let state = sessions.entry(session_id.clone()).or_default();
        if let Err(e) = fold_log(state, &path) {
            crate::logging::log(&format!("usage: 会话 {session_id} 上下文折叠失败：{e}"));
        }
    }
    let route = sessions
        .get(&session_id)
        .and_then(|s| s.current_route.as_ref());
    context_of(route, &providers::configured_routes(config))
}

fn empty_context() -> SessionContext {
    SessionContext {
        route_id: None,
        display_name: None,
        model: None,
    }
}

/// 由路由归因组装上下文：展示名优先取已配置路由的 display_name，未配置
/// 的路由回落 provider id（对齐上游 provider 缺省 `displayName: id`）。
fn context_of(
    route: Option<&aggregate::CurrentRoute>,
    routes: &[providers::ProviderRoute],
) -> SessionContext {
    let Some(route) = route else {
        return empty_context();
    };
    let display_name = routes
        .iter()
        .find(|r| r.id == route.provider_id)
        .map(|r| r.display_name.clone())
        .unwrap_or_else(|| route.provider_id.clone());
    SessionContext {
        route_id: Some(route.provider_id.clone()),
        display_name: Some(display_name),
        model: Some(route.model.clone()),
    }
}

/// 聚合全部本地会话并渲染当前用量报告（只读入口）。
///
/// 增量折叠状态落到 `$DSH_HOME/storages/usage-stats-cache.json`；消失的
/// 会话从缓存剔除；损坏/版本不符静默退回全量重折。
pub fn report(config: &crate::app_state::Config) -> Result<UsageReport, String> {
    if dev_fake::enabled() {
        return Ok(dev_fake::report());
    }
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
                totals_cost: aggregate::CostAcc::default(),
                models: std::collections::HashMap::new(),
            });
        t.totals.add_into(entry.totals);
        t.totals_cost.merge(entry.totals_cost);
        for (model, me) in &entry.models {
            let target_entry = t.models.entry(model.clone()).or_default();
            target_entry.buckets.add_into(me.buckets);
            target_entry.cost.merge(me.cost);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-fold-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root.join("session.jsonl.zstd")
    }

    /// 一条 usage chunk 事件行（seq 递增、固定时间戳）。
    fn usage_line(seq: u64, turn: u64, input: u64, output: u64) -> String {
        serde_json::json!({
            "seq": seq, "time": 1_780_000_000_000i64, "type": "assistant/chunk",
            "data": {"turn": turn, "step": 1, "chunk": {
                "type": "usage", "usage": {"inputTokens": input, "outputTokens": output}
            }}
        })
        .to_string()
            + "\n"
    }

    fn frames(lines: &[String]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            out.extend_from_slice(&zstd::encode_all(line.as_bytes(), 3).unwrap());
        }
        out
    }

    fn total_tokens(state: &FoldState) -> u64 {
        state.days.values().map(|d| d.totals.total()).sum()
    }

    #[test]
    fn incremental_fold_decodes_only_new_frames_and_survives_torn_tail() {
        let path = temp_log("torn");
        let frame1 = frames(&[usage_line(1, 1, 10, 5)]);
        std::fs::write(&path, &frame1).unwrap();
        let mut state = FoldState::default();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 1);
        assert_eq!(total_tokens(&state), 15);
        assert_eq!(state.byte_offset as usize, frame1.len());

        // 追加一个撕裂的半帧（砍掉尾部字节）：本轮跳过，状态不被破坏
        let full2 = frames(&[usage_line(2, 2, 20, 0)]);
        let torn = &full2[..full2.len() - 5];
        let mut with_torn = frame1.clone();
        with_torn.extend_from_slice(torn);
        std::fs::write(&path, &with_torn).unwrap();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 1, "撕裂帧不得部分计入");
        assert_eq!(state.byte_offset as usize, frame1.len(), "偏移不推进");
        assert_eq!(total_tokens(&state), 15);

        // 帧补全后：只折新增事件，历史不重复计
        let mut complete = frame1.clone();
        complete.extend_from_slice(&full2);
        std::fs::write(&path, &complete).unwrap();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 2);
        assert_eq!(total_tokens(&state), 35);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn incremental_fold_refolds_when_seq_regresses_on_rebuild() {
        // 同长度/更长但 seq 重启的重建日志：走整段重折，旧聚合清零
        let path = temp_log("rebuild");
        let first = frames(&[usage_line(1, 1, 10, 5), usage_line(2, 2, 20, 0)]);
        std::fs::write(&path, &first).unwrap();
        let mut state = FoldState::default();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 2);
        assert_eq!(total_tokens(&state), 35);

        // 重建：seq 从 1 重启（内容不同、文件也可能更长）
        let rebuilt = frames(&[
            usage_line(1, 1, 7, 3),
            usage_line(2, 2, 8, 2),
            usage_line(3, 3, 9, 1),
        ]);
        std::fs::write(&path, &rebuilt).unwrap();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 3);
        assert_eq!(total_tokens(&state), 7 + 8 + 9 + 3 + 2 + 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn skips_decode_when_file_len_unchanged() {
        let path = temp_log("skip");
        std::fs::write(&path, frames(&[usage_line(1, 1, 10, 5)])).unwrap();
        let mut state = FoldState::default();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 1);
        assert_eq!(total_tokens(&state), 15);
        // 等长改写为非 zstd 字节：若真去解码，空事件流会触发
        // 「max_seq < consumed」整段重折清空 days；跳过解码则保持原样。
        let len = std::fs::metadata(&path).unwrap().len();
        std::fs::write(&path, vec![b'x'; len as usize]).unwrap();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 1);
        assert_eq!(total_tokens(&state), 15);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn refolds_incrementally_when_file_grows() {
        let path = temp_log("grow");
        std::fs::write(&path, frames(&[usage_line(1, 1, 10, 5)])).unwrap();
        let mut state = FoldState::default();
        fold_log(&mut state, &path).unwrap();
        // 追加一帧后 len 变化：只折新增事件，不重复计入游标前的样本。
        std::fs::write(
            &path,
            frames(&[usage_line(1, 1, 10, 5), usage_line(2, 2, 20, 0)]),
        )
        .unwrap();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 2);
        assert_eq!(total_tokens(&state), 35);
        // 无变化再折一次：结果不变（幂等）。
        fold_log(&mut state, &path).unwrap();
        assert_eq!(total_tokens(&state), 35);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn refolds_from_scratch_when_log_truncated() {
        let path = temp_log("trunc");
        std::fs::write(
            &path,
            frames(&[usage_line(1, 1, 10, 5), usage_line(2, 2, 20, 0)]),
        )
        .unwrap();
        let mut state = FoldState::default();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 2);
        // 截断重建为更短的日志：整段重折，旧聚合不得残留。
        std::fs::write(&path, frames(&[usage_line(1, 1, 7, 3)])).unwrap();
        fold_log(&mut state, &path).unwrap();
        assert_eq!(state.consumed, 1);
        assert_eq!(total_tokens(&state), 10);
        assert_eq!(state.file_len, std::fs::metadata(&path).unwrap().len());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 一条 request/header 事件行（推进 current_route / current_model）。
    fn header_line(seq: u64, provider: &str, model: &str) -> String {
        serde_json::json!({
            "seq": seq, "time": 1_780_000_000_000i64, "type": "request/header",
            "data": {"header": {"config": {"provider": provider, "model": model}}}
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn fold_marks_persisted_kind_and_refold_clears_current_route() {
        let path = temp_log("kind");
        std::fs::write(
            &path,
            frames(&[header_line(1, "oz", "gpt-x"), usage_line(2, 1, 10, 5)]),
        )
        .unwrap();
        let mut state = FoldState::default();
        fold_log(&mut state, &path).unwrap();
        // 本壳唯一来源是持久化日志：kind 恒为 Persisted（对齐上游 state.kind）。
        assert_eq!(state.kind, aggregate::FoldKind::Persisted);
        assert_eq!(
            state.current_route.as_ref().map(|r| r.model.as_str()),
            Some("gpt-x")
        );
        // 截断重建为无 header 的日志：整段重折必须清掉旧的路由归因，
        // 不得把已失效的 current_route 继续报给「当前会话上下文」。
        std::fs::write(&path, frames(&[usage_line(1, 1, 7, 3)])).unwrap();
        fold_log(&mut state, &path).unwrap();
        assert!(state.current_route.is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn session_context_serializes_contract_fields_with_explicit_nulls() {
        // 前端契约定死：route_id / display_name / model 三字段（snake_case），
        // 无值时序列化为 null 而非省略。
        let json = serde_json::to_value(empty_context()).unwrap();
        assert!(json.get("route_id").is_some());
        assert!(json.get("display_name").is_some());
        assert!(json.get("model").is_some());
        assert!(json["route_id"].is_null());
        assert!(json["display_name"].is_null());
        assert!(json["model"].is_null());
        assert!(json.get("routeId").is_none());
        assert!(json.get("displayName").is_none());
    }

    #[test]
    fn context_of_resolves_display_name_and_falls_back_to_provider_id() {
        let routes = vec![providers::ProviderRoute {
            id: "deepseek-official".to_string(),
            display_name: "DeepSeek".to_string(),
            api_key_env: None,
            base_url: None,
        }];
        let known = aggregate::CurrentRoute {
            provider_id: "deepseek-official".to_string(),
            model: "deepseek-chat".to_string(),
            updated_at: None,
        };
        let ctx = context_of(Some(&known), &routes);
        assert_eq!(ctx.route_id.as_deref(), Some("deepseek-official"));
        assert_eq!(ctx.display_name.as_deref(), Some("DeepSeek"));
        assert_eq!(ctx.model.as_deref(), Some("deepseek-chat"));
        // 未配置的路由：展示名回落 provider id（上游 provider 缺省同口径）。
        let unknown = aggregate::CurrentRoute {
            provider_id: "my-gateway".to_string(),
            model: "m".to_string(),
            updated_at: None,
        };
        let ctx = context_of(Some(&unknown), &routes);
        assert_eq!(ctx.display_name.as_deref(), Some("my-gateway"));
        // 无路由归因：全 null。
        let ctx = context_of(None, &routes);
        assert!(ctx.route_id.is_none() && ctx.display_name.is_none() && ctx.model.is_none());
    }

    #[test]
    fn session_context_returns_all_null_without_live_session() {
        // RPC 不可达（回环未用端口）= 无活动会话：全 null，不报错。
        let mut config = crate::app_state::Config::load();
        config.port = 1;
        let ctx = session_context(&config);
        assert!(ctx.route_id.is_none() && ctx.display_name.is_none() && ctx.model.is_none());
    }
}
