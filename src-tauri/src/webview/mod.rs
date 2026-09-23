mod navigation;
mod protocol;

pub(crate) use navigation::{
    app_dev_origin, boot_continue_inject, inject_dsh_page, is_allowed_navigation, is_dsh_url,
    is_local_app_url, PAGE_INIT_SCRIPT,
};
pub use navigation::{apply_hide_tools, navigate, navigate_to_splash};
pub(crate) use protocol::handle_dshd_scheme;
