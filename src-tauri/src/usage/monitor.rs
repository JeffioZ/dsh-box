//! 账户后台监测：对齐上游 dsh-usage-stats 的账户缓存服务
//! （`ACCOUNT_REFRESH_MS` + `withStaleData` 瞬错保旧 + single-flight）。
//!
//! dsh Ready + Managed 后立即全量刷新一次，此后每 300s 一轮；结果写入进程
//! 内缓存（`cached_*` 供 get 命令秒回，空缓存回退同步查询），每轮完成后
//! 广播 `usage-accounts-updated`。外部服务模式不查询（凭据归外部环境管）。

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::app_state::{AppState, Config};

use super::balance::AccountSnapshot;
use super::subscriptions::SubscriptionSnapshot;

/// 后台刷新周期：与上游 ACCOUNT_REFRESH_MS（300000ms）同值。
const ACCOUNT_REFRESH_MS: Duration = Duration::from_secs(300);

/// 门控未满足时的短睡眠（与 notify/live 的 5s 门控同模式）。
const GATE_POLL: Duration = Duration::from_secs(5);

/// 凭据文件跟随轮询间隔（与 tray 的 settings.yaml 跟随同节奏：无变化时
/// 每轮仅一次 stat，零解析开销；3s 窗口对连续写入天然防抖）。
const CREDENTIALS_POLL: Duration = Duration::from_secs(3);

/// 账户快照缓存：None = 从未完成过全量刷新（get 命令回退同步查询）。
static CACHE: Mutex<Option<CachedSnapshots>> = Mutex::new(None);

#[derive(Clone)]
struct CachedSnapshots {
    accounts: Vec<AccountSnapshot>,
    subscriptions: Vec<SubscriptionSnapshot>,
}

/// `usage-accounts-updated` 事件载荷（与 get 命令同结构）。
#[derive(serde::Serialize, Clone)]
pub struct AccountsPayload {
    pub accounts: Vec<AccountSnapshot>,
    pub subscriptions: Vec<SubscriptionSnapshot>,
}

/// 启动账户后台监测（后台线程，退出中自动停止；外部模式空转不查询；
/// 就绪门控统一走 background::service_gate——仅本地托管且 Ready 放行，
/// 凭据归外部 dsh 环境管理）。
pub(crate) fn start_account_monitor(app: AppHandle) {
    crate::background::spawn_gated_periodic(
        app,
        "account-monitor",
        ACCOUNT_REFRESH_MS,
        GATE_POLL,
        // 立即一轮（single-flight：与手动触发合并，不并发两轮）。
        |app| request_account_refresh(app.clone()),
    );
}

/// 凭据文件 mtime（文件不存在或不可读为 None）。
fn credentials_mtime(config: &Config) -> Option<std::time::SystemTime> {
    std::fs::metadata(config.dsh_home().join(".credentials.yaml"))
        .ok()
        .and_then(|meta| meta.modified().ok())
}

/// 凭据文件变化判定（纯函数）：文件出现或 mtime 变化即触发。文件消失
/// 不触发——dsh 与本壳的写入都是同目录原子替换，运行中消失只可能是用户
/// 主动清空，无凭据的刷新结果（not-configured）交给周期轮自然收敛。
fn credentials_changed(
    baseline: Option<std::time::SystemTime>,
    current: Option<std::time::SystemTime>,
) -> bool {
    current.is_some() && baseline != current
}

/// 跟随 `$DSH_HOME/.credentials.yaml` 的 mtime：用户在 dsh 设置页保存
/// key 后立即触发一轮账户刷新，不必等 5 分钟周期（状态栏"未配置 API
/// Key"的最长滞留由整周期缩短为本轮询间隔）。非标准节奏的手写循环，
/// 谓词复用 `service_gate`；外部模式不跟随（凭据归外部环境管）。
pub(crate) fn start_credentials_follow(app: AppHandle) {
    std::thread::spawn(move || {
        // 启动基线取当前文件状态：避免每次启动把既有文件误判为"变化"
        //（监测自身门控放行后本会立即刷一轮，启动期多触发是纯浪费）。
        let mut baseline = credentials_mtime(&app.state::<AppState>().config());
        loop {
            std::thread::sleep(CREDENTIALS_POLL);
            match crate::background::service_gate(&app) {
                crate::background::Gate::Quitting => return,
                // 未就绪/外部模式：只跟随基线不触发；期间的变化由门控重开
                // 时监测自身的立即轮兜底，避免重开瞬间的重复刷新。
                crate::background::Gate::NotReady => {
                    baseline = credentials_mtime(&app.state::<AppState>().config());
                    continue;
                }
                crate::background::Gate::Ready => {}
            }
            let config = app.state::<AppState>().config();
            let current = credentials_mtime(&config);
            if !credentials_changed(baseline, current) {
                continue;
            }
            baseline = current;
            crate::logging::log("credentials: mtime 变化，触发账户刷新");
            request_account_refresh(app.clone());
        }
    });
}

/// 缓存的账户/订阅快照（None = 从未刷新过，调用方回退同步查询）。
pub(crate) fn cached_accounts() -> Option<Vec<AccountSnapshot>> {
    CACHE.lock().ok()?.as_ref().map(|c| c.accounts.clone())
}

pub(crate) fn cached_subscriptions() -> Option<Vec<SubscriptionSnapshot>> {
    CACHE.lock().ok()?.as_ref().map(|c| c.subscriptions.clone())
}

/// 监测缓存中新鲜的 DeepSeek 官方路由快照（< ACCOUNT_REFRESH_MS），供状态栏
/// 余额直接复用（缓存空/过期由调用方回退直连查询）。stale 快照的
/// updated_at 停留在上次成功时刻，天然被新鲜度判为过期。
pub(crate) fn cached_deepseek() -> Option<AccountSnapshot> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    CACHE
        .lock()
        .ok()?
        .as_ref()?
        .accounts
        .iter()
        .find(|a| matches!(a.id.as_str(), "deepseek-official" | "deepseek"))
        .filter(|a| {
            a.updated_at
                .is_some_and(|t| now.saturating_sub(t) < ACCOUNT_REFRESH_MS.as_secs())
        })
        .cloned()
}

/// single-flight 门控：一轮刷新进行中时，触发请求合并为一次「补一轮」，
/// 不并发两轮（对齐上游 createAccountService 的 inflight 合并）。
#[derive(Default)]
struct RefreshGate {
    running: bool,
    pending: bool,
}

impl RefreshGate {
    /// 请求执行权：true = 调用方应开一轮；false = 已有轮在进行，请求已合并。
    fn acquire(&mut self) -> bool {
        if self.running {
            self.pending = true;
            return false;
        }
        self.running = true;
        true
    }

    /// 一轮结束：true = 有待补请求（已消费），需再跑一轮；false = 清空运行态。
    fn finish_or_continue(&mut self) -> bool {
        if self.pending {
            self.pending = false;
            true
        } else {
            self.running = false;
            false
        }
    }
}

static GATE: Mutex<RefreshGate> = Mutex::new(RefreshGate {
    running: false,
    pending: false,
});

/// 测试专用：CACHE 为进程内静态，所有写 CACHE 的用例（含 balance.rs 的
/// 缓存命中用例）都必须持有此锁串行，避免并行测试互相污染。
#[cfg(test)]
pub(crate) static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 测试专用：直接写入账户缓存（绕过刷新线程与门控）。
#[cfg(test)]
pub(crate) fn set_cache_for_test(accounts: Vec<AccountSnapshot>) {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(CachedSnapshots {
            accounts,
            subscriptions: Vec::new(),
        });
    }
}

/// 触发一轮全量刷新（立即返回；进行中的轮次会合并请求，结果经
/// `usage-accounts-updated` 推送）。
pub(crate) fn request_account_refresh(app: AppHandle) {
    let Ok(mut gate) = GATE.lock() else {
        return;
    };
    if !gate.acquire() {
        return;
    }
    drop(gate);
    std::thread::spawn(move || loop {
        run_round(&app);
        let Ok(mut gate) = GATE.lock() else {
            return;
        };
        if !gate.finish_or_continue() {
            return;
        }
    });
}

/// 一轮刷新：查询 → 合并旧缓存 → 写缓存 → 广播。
fn run_round(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    let payload = refresh_all(&config);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(CachedSnapshots {
            accounts: payload.accounts.clone(),
            subscriptions: payload.subscriptions.clone(),
        });
    }
    crate::emit_signed(app, "usage-accounts-updated", &payload);
    // 状态栏 chip 只监听 balance-updated（非 usage-accounts-updated），其
    // 周期任务 5 分钟才读一次缓存——任何触发源（周期轮/手动/凭据跟随）
    // 完成后顺带推一次余额，让"dsh 设置页刚填完 key"立即生效
    //（refresh_once 自带本地模式与可见性门控；query_balance 命中刚写入
    // 的新鲜缓存，零网络请求）。
    crate::balance::refresh_once(app.clone());
}

/// 全量刷新：逐路由查余额 + 全部订阅适配器，并与旧缓存做瞬错保旧合并。
fn refresh_all(config: &Config) -> AccountsPayload {
    let previous = CACHE.lock().ok().and_then(|cache| cache.clone());
    let old_accounts = previous
        .as_ref()
        .map(|p| p.accounts.as_slice())
        .unwrap_or(&[]);
    let old_subscriptions = previous
        .as_ref()
        .map(|p| p.subscriptions.as_slice())
        .unwrap_or(&[]);
    // 走 mod 层入口：假数据模式（DSH_BOX_FAKE_USAGE）下同源返回假快照、
    // 零网络请求；真实模式与原逐路由查询等价。
    let accounts: Vec<super::balance::AccountSnapshot> = match super::accounts(config) {
        Ok(accounts) => accounts,
        Err(e) => {
            crate::logging::log(&format!("usage: 账户查询失败 {e}"));
            Vec::new()
        }
    };
    let subscriptions = super::subscriptions(config);
    // 排查日志：只记异常态（not-configured 是"没配凭据"的正常态，不记），
    // 300s 一轮、账户数有限，量可控。error 文本截断防日志膨胀。
    // 假数据模式含人为构造的异常态卡片，不记。
    if !super::dev_fake::enabled() {
        for snapshot in accounts.iter() {
            if snapshot.status != "ok" && snapshot.status != "not-configured" {
                crate::logging::log(&format!(
                    "usage: 账户查询失败 id={} adapter={} status={} error={}",
                    snapshot.id,
                    snapshot.adapter.unwrap_or("-"),
                    snapshot.status,
                    truncate_for_log(snapshot.error.as_deref().unwrap_or(""))
                ));
            }
        }
        for snapshot in subscriptions.iter() {
            if snapshot.status != "ok" && snapshot.status != "not-configured" {
                crate::logging::log(&format!(
                    "usage: 订阅查询失败 id={} adapter={} status={} error={}",
                    snapshot.id,
                    snapshot.adapter,
                    snapshot.status,
                    truncate_for_log(snapshot.error.as_deref().unwrap_or(""))
                ));
            }
        }
    }
    AccountsPayload {
        accounts: merge_accounts(old_accounts, accounts),
        subscriptions: merge_subscriptions(old_subscriptions, subscriptions),
    }
}

/// 日志用错误文本截断（单行、去换行）。
fn truncate_for_log(text: &str) -> String {
    let flat: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if flat.chars().count() <= 160 {
        flat
    } else {
        let prefix: String = flat.chars().take(157).collect();
        format!("{prefix}...")
    }
}

/// 瞬态错误（对齐上游 isTransient）：网络不可用/限流/响应无法解析；
/// 超时类错误在两个查询层均归入 unavailable。
fn is_transient(status: &str) -> bool {
    matches!(status, "unavailable" | "rate-limited" | "invalid-response")
}

/// 对齐上游 withStaleData：瞬错且历史成功 → 保留旧快照并置 stale
/// （updated_at 保留上次成功时刻）；unauthorized/not-configured 等确定性
/// 状态直接用新快照——无效或缺失的密钥不继续显示旧余额
/// （docs/troubleshooting.md）。
fn merge_account(previous: Option<&AccountSnapshot>, current: AccountSnapshot) -> AccountSnapshot {
    match previous {
        Some(prev) if prev.status == "ok" && is_transient(current.status) => {
            let mut kept = prev.clone();
            kept.stale = true;
            kept
        }
        _ => current,
    }
}

fn merge_accounts(
    previous: &[AccountSnapshot],
    current: Vec<AccountSnapshot>,
) -> Vec<AccountSnapshot> {
    current
        .into_iter()
        .map(|cur| merge_account(previous.iter().find(|p| p.id == cur.id), cur))
        .collect()
}

fn merge_subscription(
    previous: Option<&SubscriptionSnapshot>,
    current: SubscriptionSnapshot,
) -> SubscriptionSnapshot {
    match previous {
        Some(prev) if prev.status == "ok" && is_transient(current.status) => {
            let mut kept = prev.clone();
            kept.stale = true;
            kept
        }
        _ => current,
    }
}

fn merge_subscriptions(
    previous: &[SubscriptionSnapshot],
    current: Vec<SubscriptionSnapshot>,
) -> Vec<SubscriptionSnapshot> {
    current
        .into_iter()
        .map(|cur| merge_subscription(previous.iter().find(|p| p.id == cur.id), cur))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::balance::Balance;
    use crate::usage::subscriptions::QuotaWindow;

    fn ok_account(id: &str, updated_at: u64) -> AccountSnapshot {
        AccountSnapshot {
            id: id.to_string(),
            display_name: id.to_string(),
            mode: "balance",
            adapter: None,
            status: "ok",
            balance: Some(Balance {
                remaining: Some(50.0),
                used: None,
                total: None,
                currency: "CNY".to_string(),
                unlimited: false,
                granted: None,
                topped_up: None,
            }),
            windows: Vec::new(),
            error: None,
            updated_at: Some(updated_at),
            stale: false,
            warn_level: "warning",
        }
    }

    fn err_account(id: &str, status: &'static str) -> AccountSnapshot {
        AccountSnapshot {
            id: id.to_string(),
            display_name: id.to_string(),
            mode: "balance",
            adapter: None,
            status,
            balance: None,
            windows: Vec::new(),
            error: Some("boom".to_string()),
            updated_at: Some(999),
            stale: false,
            warn_level: "none",
        }
    }

    fn ok_subscription(id: &str) -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            id: id.to_string(),
            display_name: id.to_string(),
            mode: "subscription",
            adapter: "zai-token-plan",
            status: "ok",
            plan: "GLM Coding Plan".to_string(),
            windows: vec![QuotaWindow {
                kind: "session".to_string(),
                used_percent: 20.0,
                remaining_percent: 80.0,
                resets_at: None,
            }],
            error: None,
            stale: false,
            warn_level: "none",
        }
    }

    #[test]
    fn transient_error_keeps_previous_ok_snapshot() {
        for status in ["unavailable", "rate-limited", "invalid-response"] {
            let prev = ok_account("deepseek-official", 100);
            let merged = merge_account(Some(&prev), err_account("deepseek-official", status));
            assert_eq!(merged.status, "ok", "{status} 应保旧");
            assert!(merged.stale, "{status} 应置 stale");
            assert_eq!(merged.updated_at, Some(100), "{status} 应保留旧 updated_at");
            assert!(merged.balance.is_some(), "{status} 应保留旧余额");
            assert_eq!(merged.warn_level, "warning", "{status} 应保留旧 warn_level");
        }
    }

    #[test]
    fn unauthorized_and_not_configured_replace_previous_snapshot() {
        for status in ["unauthorized", "not-configured", "blocked", "unsupported"] {
            let prev = ok_account("deepseek-official", 100);
            let merged = merge_account(Some(&prev), err_account("deepseek-official", status));
            assert_eq!(merged.status, status);
            assert!(!merged.stale);
            assert!(merged.balance.is_none(), "{status} 不得继续显示旧余额");
        }
    }

    #[test]
    fn previous_non_ok_snapshot_is_not_kept() {
        // 历史并非成功快照（本身也是错误）时，瞬错直接覆盖。
        let prev = err_account("deepseek-official", "unavailable");
        let merged = merge_account(
            Some(&prev),
            err_account("deepseek-official", "rate-limited"),
        );
        assert_eq!(merged.status, "rate-limited");
        assert!(!merged.stale);
    }

    #[test]
    fn ok_result_clears_stale_flag() {
        let mut prev = ok_account("deepseek-official", 100);
        prev.stale = true;
        let merged = merge_account(Some(&prev), ok_account("deepseek-official", 200));
        assert!(!merged.stale);
        assert_eq!(merged.updated_at, Some(200));
    }

    #[test]
    fn merge_accounts_matches_by_route_id() {
        let previous = vec![ok_account("deepseek-official", 100)];
        let current = vec![
            err_account("deepseek-official", "unavailable"),
            err_account("openrouter", "unavailable"),
        ];
        let merged = merge_accounts(&previous, current);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].stale, "有历史成功的路由应保旧");
        assert!(!merged[1].stale, "无历史的路由直接用新快照");
        assert_eq!(merged[1].status, "unavailable");
    }

    #[test]
    fn subscription_merge_shares_account_semantics() {
        let prev = ok_subscription("zai");
        let mut transient = ok_subscription("zai");
        transient.status = "unavailable";
        transient.windows = Vec::new();
        let merged = merge_subscription(Some(&prev), transient);
        assert!(merged.stale);
        assert_eq!(merged.windows.len(), 1, "瞬错应保留旧窗口");

        let mut unauthorized = ok_subscription("zai");
        unauthorized.status = "unauthorized";
        unauthorized.windows = Vec::new();
        let merged = merge_subscription(Some(&prev), unauthorized);
        assert!(!merged.stale);
        assert_eq!(merged.status, "unauthorized");
        assert!(merged.windows.is_empty(), "401 不得保留旧窗口");
    }

    #[test]
    fn credentials_changed_triggers_on_create_and_modify_only() {
        let t0 = std::time::SystemTime::now();
        let t1 = t0 + std::time::Duration::from_secs(1);
        // 文件出现（首次配置 key 的主路径）
        assert!(credentials_changed(None, Some(t0)));
        // mtime 变化（dsh 原子替换写入）
        assert!(credentials_changed(Some(t0), Some(t1)));
        // 未变化：不触发（无变化零刷新，轮询开销仅一次 stat）
        assert!(!credentials_changed(Some(t0), Some(t0)));
        // 持续不存在：不触发
        assert!(!credentials_changed(None, None));
        // 文件消失：不触发（主动清空由周期轮收敛，见函数注释）
        assert!(!credentials_changed(Some(t0), None));
    }

    #[test]
    fn single_flight_gate_coalesces_concurrent_requests() {
        let mut gate = RefreshGate::default();
        assert!(gate.acquire(), "首个请求获得执行权");
        assert!(!gate.acquire(), "进行中的请求被合并");
        assert!(!gate.acquire(), "多次触发仍只挂起一次");
        assert!(gate.finish_or_continue(), "待补请求被消费，需补一轮");
        assert!(!gate.acquire(), "补轮进行中继续合并");
        assert!(gate.finish_or_continue(), "补轮期间的新请求再补一轮");
        assert!(!gate.finish_or_continue(), "无待补请求则清空运行态");
        assert!(gate.acquire(), "清空后可重新开轮");
    }

    #[test]
    fn refresh_allowed_requires_managed_and_ready() {
        use crate::app_state::{BootPhase, ServiceOwnership};
        use crate::background::refresh_allowed;
        assert!(refresh_allowed(ServiceOwnership::Managed, BootPhase::Ready));
        for ownership in [
            ServiceOwnership::None,
            ServiceOwnership::External,
            ServiceOwnership::ExternalDisconnected,
        ] {
            assert!(!refresh_allowed(ownership, BootPhase::Ready));
        }
        assert!(!refresh_allowed(
            ServiceOwnership::Managed,
            BootPhase::Starting
        ));
    }

    #[test]
    fn snapshot_serializes_stale_and_warn_level_in_snake_case() {
        let json = serde_json::to_value(ok_account("deepseek-official", 100)).unwrap();
        assert_eq!(json["stale"], false);
        assert_eq!(json["warn_level"], "warning");
        assert!(json.get("warnLevel").is_none());
        assert!(json.get("Stale").is_none());
        let json = serde_json::to_value(ok_subscription("zai")).unwrap();
        assert_eq!(json["stale"], false);
        assert_eq!(json["warn_level"], "none");
    }

    #[test]
    fn cache_roundtrip_drives_cache_first_reads() {
        // 空缓存（从未刷新）→ None，命令层据此回退同步查询；写入后秒回。
        // CACHE 为进程内静态，全部断言收敛在单个用例内并持锁串行。
        let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(mut cache) = CACHE.lock() {
            *cache = None;
        }
        assert!(cached_accounts().is_none());
        assert!(cached_subscriptions().is_none());
        assert!(cached_deepseek().is_none());
        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some(CachedSnapshots {
                accounts: vec![ok_account("deepseek-official", 100)],
                subscriptions: vec![ok_subscription("zai")],
            });
        }
        let accounts = cached_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "deepseek-official");
        assert_eq!(cached_subscriptions().unwrap().len(), 1);
        if let Ok(mut cache) = CACHE.lock() {
            *cache = None;
        }
    }

    #[test]
    fn cached_deepseek_requires_fresh_official_route_snapshot() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // 新鲜的官方路由快照：命中。
        set_cache_for_test(vec![ok_account("deepseek-official", now - 10)]);
        assert_eq!(
            cached_deepseek().map(|a| a.id),
            Some("deepseek-official".to_string())
        );
        // 过期（≥ ACCOUNT_REFRESH_MS）：不命中，调用方回退直连。
        set_cache_for_test(vec![ok_account(
            "deepseek-official",
            now - ACCOUNT_REFRESH_MS.as_secs() - 1,
        )]);
        assert!(cached_deepseek().is_none());
        // stale 快照 updated_at 停留在旧成功时刻：按过期处理。
        let mut stale = ok_account("deepseek-official", now - ACCOUNT_REFRESH_MS.as_secs() - 1);
        stale.stale = true;
        set_cache_for_test(vec![stale]);
        assert!(cached_deepseek().is_none());
        // 只有其他路由的新鲜快照：不命中（状态栏只复用 DeepSeek 官方路由）。
        set_cache_for_test(vec![ok_account("openrouter", now - 10)]);
        assert!(cached_deepseek().is_none());
        if let Ok(mut cache) = CACHE.lock() {
            *cache = None;
        }
    }

    #[test]
    fn stale_snapshot_serializes_last_success_time() {
        // 锁死契约：stale 快照保留旧 updated_at（最后成功时间），前端
        // 「更新于」与 stale 标记都显示该时刻（序列化级断言防回退）。
        let prev = ok_account("deepseek-official", 100);
        let merged = merge_account(Some(&prev), err_account("deepseek-official", "unavailable"));
        let json = serde_json::to_value(&merged).unwrap();
        assert_eq!(json["stale"], true);
        assert_eq!(json["updated_at"], 100);
        assert_eq!(json["status"], "ok");
    }
}
