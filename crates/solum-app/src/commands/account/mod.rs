use super::*;
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub(crate) use huawei::{account_huawei_config_get, account_huawei_login};
pub(crate) use login::{
    account_login, account_oauth_config_get, account_oauth_login, account_oauth_pkce_generate,
    account_register, open_external_url,
};
pub(crate) use logout::{
    account_delete, account_logout, recovery_code_issue, recovery_code_login,
    recovery_code_pending, recovery_code_pending_ack, recovery_code_status,
};
pub(crate) use session::{
    account_auth, account_status_get, restart_account_app, schedule_pending_session_revoke,
};
pub(crate) use settings::{account_change_password, account_model_save};
pub(crate) use sms_email::{
    account_captcha_get, account_email_login, account_email_register, account_email_send_code,
    account_phone_login, account_phone_register, account_sms_send,
};

// ---- Solum account (cloud proxy) — ported from the harmony 0.2.0 client ----

mod huawei;
mod login;
mod logout;
mod session;
mod settings;
mod sms_email;
