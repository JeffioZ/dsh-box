mod navigation;
mod protocol;

#[cfg(test)]
pub(crate) use navigation::hide_stats_apply;
pub(crate) use navigation::{
    app_dev_origin, hide_stats_early, inject_dsh_page, is_allowed_navigation, is_dsh_url,
    is_local_app_url, PAGE_INIT_SCRIPT,
};
pub use navigation::{apply_hide_stats, apply_hide_tools, navigate, navigate_to_splash};
pub(crate) use protocol::handle_dshd_scheme;
