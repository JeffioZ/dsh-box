//! 插件变更的合并与应用：多手动变更合并为一次服务重启，空闲时自动应用，
//! 失败按指数退避重试（详见 mod.rs 的模块总述）。

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::AppState;

#[derive(Default)]
struct RestartState {
    generation: u64,
    pending: bool,
    applying: bool,
    deferred: bool,
    waiting_for_idle: bool,
    error: Option<String>,
    /// 连续重启失败次数（指数退避用，成功清零）。
    failures: u32,
    /// 失败退避窗口：此时刻前不重试重启。
    retry_not_before: Option<std::time::Instant>,
}

static RESTART_STATE: std::sync::Mutex<RestartState> = std::sync::Mutex::new(RestartState {
    generation: 0,
    pending: false,
    applying: false,
    deferred: false,
    waiting_for_idle: false,
    error: None,
    failures: 0,
    retry_not_before: None,
});
static RESTART_COORDINATOR_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 重启失败的指数退避：30s 起步翻倍，上限 10 分钟。
pub(super) fn restart_backoff_secs(failures: u32) -> u64 {
    (30u64 << failures.saturating_sub(1).min(5)).min(600)
}

#[derive(Clone, Serialize)]
pub struct PluginApplyStatus {
    pub pending: bool,
    pub applying: bool,
    pub waiting_for_idle: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn apply_status() -> PluginApplyStatus {
    let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
    PluginApplyStatus {
        pending: state.pending,
        applying: state.applying,
        waiting_for_idle: state.waiting_for_idle,
        error: state.error.clone(),
    }
}

pub fn plugin_apply_status() -> PluginApplyStatus {
    apply_status()
}

pub(crate) fn deferred_restart_pending() -> bool {
    let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.pending && state.deferred
}

pub(crate) fn mark_plugin_changes(app: &AppHandle, apply_when_idle: bool) {
    {
        let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.pending = true;
        state.error = None;
        if apply_when_idle {
            state.deferred = true;
        }
    }
    if apply_when_idle {
        start_restart_coordinator(app);
    }
}

pub fn apply_plugin_changes(app: &AppHandle) -> PluginApplyStatus {
    {
        let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.pending {
            state.deferred = true;
            state.error = None;
            // 用户显式要求应用：立即重试，不吃此前的失败退避
            state.retry_not_before = None;
        }
    }
    start_restart_coordinator(app);
    apply_status()
}

fn start_restart_coordinator(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    if RESTART_COORDINATOR_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            if handle.state::<AppState>().is_quitting() {
                break;
            }
            let (should_apply, backing_off) = {
                let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
                let should = state.pending && state.deferred && !state.applying;
                let backing = state
                    .retry_not_before
                    .is_some_and(|t| std::time::Instant::now() < t);
                (should, backing)
            };
            if !should_apply {
                break;
            }
            // 上次重启失败的退避窗口未到：保持 deferred 等待。此前 Err 分支
            // 直接置 deferred=false，一次瞬时重启失败会永久取消自动应用，
            // 只剩插件页手动按钮。
            if backing_off {
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            let config = handle.state::<AppState>().config();
            if crate::usage::session_activity(&config) != Some(false) {
                RESTART_STATE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .waiting_for_idle = true;
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            let generation = {
                let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.applying = true;
                state.waiting_for_idle = false;
                state.generation
            };
            let result = crate::updater::restart_service(&handle);
            let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.applying = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    state.failures = 0;
                    state.retry_not_before = None;
                    if state.generation == generation {
                        state.pending = false;
                        state.deferred = false;
                    }
                }
                Err(error) => {
                    crate::logging::log(&format!(
                        "plugins: 重启服务失败（插件变更仍待应用，将退避重试）：{error}"
                    ));
                    state.error = Some(error);
                    state.failures = state.failures.saturating_add(1);
                    state.retry_not_before = Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_secs(restart_backoff_secs(state.failures)),
                    );
                }
            }
        }
        RESTART_COORDINATOR_RUNNING.store(false, Ordering::Release);
        let needs_restart = {
            let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.pending && state.deferred
        };
        if needs_restart {
            start_restart_coordinator(&handle);
        }
    });
}
