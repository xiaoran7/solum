//! Solum 独立账号：鉴权自建 solum-cloud AI 代理（`server/`，与 Solum Harmony 共用同一契约）。
//!
//! 登录后 AI 请求发往 `{server}/v1/ai/chat/completions`，第三方 API Key 只存在于
//! 服务端环境变量；登录动作本身不上传本机数据库。同一短期 access token 也供同步
//! relay 验证租户身份，但同步正文仍由独立的设备端密钥加密。
//!
//! 会话落盘 `solum-account.json`（gitignore）：`server_url` / `user_id` /
//! `username` / `access_token` / `refresh_token`。`user_id` 是服务端生成且不可变的
//! 身份主键；用户名只用于登录和展示，绝不能再充当本地目录或中继租户主键。
//! 旧会话没有 `user_id` 时必须重新登录，不能参与刷新或账号数据访问。
//! 密码只用于登录请求本身，绝不落盘。

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use base64::Engine;
#[cfg(not(target_os = "android"))]
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(target_os = "android")]
use std::os::fd::AsRawFd;

use crate::error::{CoreError, Result};
use crate::extract::Reasoner;

mod authenticate;
mod flows;
mod http;
mod reasoner;
mod session;
#[cfg(test)]
mod tests;

pub use authenticate::*;
pub use flows::*;
pub use http::*;
pub use reasoner::*;
pub use session::*;
