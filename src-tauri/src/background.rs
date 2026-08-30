//! 后台周期任务共享骨架。
//!
//! 此前各模块自带「5s 门控轮询就绪状态」的循环，谓词逐字重复且退出语义
//! 各写各的（漏写 is_quitting 即泄漏线程）。这里提供统一的就绪门控与
//! 门控型周期任务 runner；任务本体仍各持一线程——网络等待（余额、更新
//! 检查等 10s+ 超时）不能与 2s/5s 的 UI 轮询共享线程串行。
//!
//! 迁移约定：新后台任务优先用 [`spawn_gated_periodic`]；只有带跨轮状态
//! （如 notify 的会话跟踪）或非标准节奏（指数退避等）的任务才手写循环，
//! 且谓词必须复用 [`service_gate`]，不重新拼 `ownership`/`phase` 条件。

use std::time::Duration;

use tauri::AppHandle;

use crate::app_state::{AppState, BootPhase, ServiceOwnership};
use tauri::Manager;

/// 就绪门控的三态结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// 本地托管服务已就绪，可执行依赖服务的任务。
    Ready,
    /// 未就绪（启动中/已断开/外部模式）：本轮跳过，下个门控周期再看。
    NotReady,
    /// 应用退出中：结束任务线程。
    Quitting,
}

/// 统一的就绪门控：仅本地托管（Managed）且 Ready 时放行。外部服务
/// （含已断开）与任何启动阶段都不放行——外部环境的凭据/数据不归本壳
/// 查询，启动阶段查询只会拿到半初始化状态。
pub(crate) fn service_gate(app: &AppHandle) -> Gate {
    let state = app.state::<AppState>();
    if state.is_quitting() {
        return Gate::Quitting;
    }
    if refresh_allowed(state.service_ownership(), state.phase()) {
        return Gate::Ready;
    }
    Gate::NotReady
}

/// 就绪判定的纯谓词（单测直接覆盖；各模块不得自行重拼该条件）。
pub(crate) fn refresh_allowed(ownership: ServiceOwnership, phase: BootPhase) -> bool {
    ownership == ServiceOwnership::Managed && phase == BootPhase::Ready
}

/// 门控型周期任务：就绪时执行 `task` 后睡 `work_interval`；未就绪时只睡
/// `gate_interval` 空转探测；退出即返回。`task` 内部可自行跳过（如界面
/// 隐藏时直接返回，此时同样按 `work_interval` 节奏下一轮）。
pub(crate) fn spawn_gated_periodic(
    app: AppHandle,
    name: &'static str,
    work_interval: Duration,
    gate_interval: Duration,
    task: impl Fn(&AppHandle) + Send + 'static,
) {
    std::thread::spawn(move || loop {
        match service_gate(&app) {
            Gate::Quitting => {
                crate::logging::log(&format!("background: {name} 退出"));
                return;
            }
            Gate::NotReady => {
                std::thread::sleep(gate_interval);
                continue;
            }
            Gate::Ready => {}
        }
        task(&app);
        std::thread::sleep(work_interval);
    });
}
