//! tests. Part of the `account` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `account.rs`.

use super::*;
use std::cell::{Cell, RefCell};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

static ACCOUNT_CONFIG_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn session_roundtrip_and_harmony_shape_compat() {
    // 鸿蒙版四字段文件（无 model）→ 默认模型补齐。
    let harmony = r#"{
        "server_url": "https://cloud.example.com",
        "username": "tangren",
        "access_token": "a.b",
        "refresh_token": "r"
    }"#;
    let session = AccountSession::from_json(harmony).unwrap();
    assert_eq!(session.model, DEFAULT_CLOUD_MODEL);
    assert_eq!(session.username, "tangren");
    assert_eq!(session.stable_user_id(), None);
    // 本仓写出的形状（含 model）自往返。
    let json = serde_json::to_string(&session).unwrap();
    let back = AccountSession::from_json(&json).unwrap();
    assert_eq!(session, back);

    let opaque = r#"{
        "server_url":"https://cloud.example.com",
        "username":"u",
        "access_token":"  access  ",
        "refresh_token":"\trefresh\n"
    }"#;
    let opaque = AccountSession::from_json(opaque).unwrap();
    assert_eq!(opaque.access_token, "  access  ");
    assert_eq!(opaque.refresh_token, "\trefresh\n");
}

#[test]
fn sessions_saved_under_retired_centers_follow_the_current_domain() {
    // 两代 `.xyz` 中心域名下的存量会话在加载时都透明迁移到 `.cn`。
    let legacy = r#"{
        "server_url": "LEGACY_CENTER",
        "user_id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
        "username": "tangren",
        "access_token": "a.b",
        "refresh_token": "r"
    }"#;
    for retired in LEGACY_CENTER_URLS {
        let session = AccountSession::from_json(&legacy.replace("LEGACY_CENTER", retired)).unwrap();
        assert_eq!(session.server_url, SOLUM_CENTER_URL);
    }
    // 合并只针对旧中心域名，其他地址原样保留。
    let untouched =
        AccountSession::from_json(&legacy.replace("LEGACY_CENTER", "https://solum.example.org"))
            .unwrap();
    assert_eq!(untouched.server_url, "https://solum.example.org");
}

#[test]
fn center_domain_merge_maps_only_retired_hosts() {
    // 恰好等于任一旧中心域名（含末尾斜杠）→ 当前身份；其他地址一律不动。
    for retired in LEGACY_CENTER_URLS {
        assert_eq!(migrate_legacy_center_url(retired), SOLUM_CENTER_URL);
        assert_eq!(
            migrate_legacy_center_url(&format!("{retired}/")),
            SOLUM_CENTER_URL
        );
    }
    assert_eq!(
        migrate_legacy_center_url(SOLUM_CENTER_URL),
        SOLUM_CENTER_URL
    );
    assert_eq!(
        migrate_legacy_center_url("https://solumsync.example.org"),
        "https://solumsync.example.org"
    );
}

#[test]
fn only_uuid_sessions_form_a_full_account_identity() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "solum-account-full-session-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("solum-account.json");
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", &path);

    let mut session = AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: String::new(),
        username: "legacy".into(),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    };
    session.save().unwrap();
    assert!(AccountSession::load().is_some());
    assert!(AccountSession::load_full().is_none());

    session.user_id = "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into();
    session.save().unwrap();
    assert_eq!(
        AccountSession::load_full()
            .and_then(|loaded| loaded.stable_user_id().map(str::to_owned))
            .as_deref(),
        Some("9d4df1be-9f7b-4a3a-b986-ec920d2df60e")
    );

    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn refresh_never_adopts_a_different_uuid_with_the_same_username() {
    let original = AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
        username: "same-name".into(),
        access_token: "a".into(),
        refresh_token: "r".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    };
    let mut replacement = original.clone();
    replacement.user_id = "6ea64c69-0531-45cb-b585-500c5f479fd8".into();
    assert!(!same_account_identity(&original, &replacement));
    replacement.user_id = original.user_id.clone();
    assert!(same_account_identity(&original, &replacement));
}

#[test]
fn model_update_preserves_the_latest_rotated_tokens() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "solum-account-model-update-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));

    let expected = AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
        username: "same-name".into(),
        access_token: "old-access".into(),
        refresh_token: "old-refresh".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    };
    let mut latest = expected.clone();
    latest.access_token = "new-access".into();
    latest.refresh_token = "new-refresh".into();
    latest.save().unwrap();

    let updated = update_active_model(&expected, "mimo-v2.5-pro").unwrap();
    assert_eq!(updated.access_token, "new-access");
    assert_eq!(updated.refresh_token, "new-refresh");
    assert_eq!(updated.model, "mimo-v2.5-pro");
    assert_eq!(AccountSession::load_full().unwrap(), updated);

    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_file_lock_blocks_a_second_writer() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "solum-account-file-lock-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
    let lock_path = session_lock_path(&AccountSession::path());
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock_session_file(&lock_file).unwrap();

    let session = state_test_session();
    let (sent, received) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let result = publish_session(&session);
        sent.send(result).unwrap();
    });
    assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
    unlock_session_file(&lock_file).unwrap();
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("writer did not resume after OS lock release")
        .unwrap();
    writer.join().unwrap();
    drop(lock_file);

    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stale_refresh_cannot_recreate_a_missing_or_different_session() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "solum-account-stale-refresh-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
    let stale = state_test_session();
    let fake = RotatingFake {
        calls: RefCell::new(Vec::new()),
    };

    assert!(refresh_and_save_with_client(&fake, &stale).is_err());
    assert!(
        fake.calls.borrow().is_empty(),
        "missing state must fail before HTTP"
    );

    let mut replacement = stale.clone();
    replacement.user_id = "6ea64c69-0531-45cb-b585-500c5f479fd8".into();
    replacement.access_token = "other-access".into();
    replacement.refresh_token = "other-refresh".into();
    publish_session(&replacement).unwrap();
    assert!(refresh_and_save_with_client(&fake, &stale).is_err());
    assert_eq!(AccountSession::load_full().unwrap(), replacement);
    assert!(
        fake.calls.borrow().is_empty(),
        "another identity must never be contacted"
    );

    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn refresh_preserves_a_model_change_that_lands_during_network_io() {
    struct ModelRaceFake {
        expected: AccountSession,
    }
    impl AccountHttp for ModelRaceFake {
        fn post_json(
            &self,
            url: &str,
            _access_token: Option<&str>,
            _body: Value,
        ) -> std::result::Result<Value, AccountHttpError> {
            if url.ends_with("/v1/auth/refresh") {
                update_active_model(&self.expected, "mimo-v2.5-pro").unwrap();
                return Ok(json!({
                    "access_token":"access-2", "refresh_token":"refresh-2",
                    "user":{"id":self.expected.user_id.clone(),
                            "username":self.expected.username.clone()}
                }));
            }
            Ok(json!({"status":"ok"}))
        }
    }

    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "solum-account-model-race-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
    let session = state_test_session();
    publish_session(&session).unwrap();
    let rotated = refresh_and_save_with_client(
        &ModelRaceFake {
            expected: session.clone(),
        },
        &session,
    )
    .unwrap();
    assert_eq!(rotated.refresh_token, "refresh-2");
    assert_eq!(rotated.model, "mimo-v2.5-pro");
    assert_eq!(AccountSession::load_full().unwrap(), rotated);

    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn logout_during_refresh_revokes_the_new_pair_instead_of_resurrecting_it() {
    struct LogoutRaceFake {
        revoke_calls: Cell<usize>,
        user_id: String,
    }
    impl AccountHttp for LogoutRaceFake {
        fn post_json(
            &self,
            url: &str,
            _access_token: Option<&str>,
            _body: Value,
        ) -> std::result::Result<Value, AccountHttpError> {
            if url.ends_with("/v1/auth/refresh") {
                take_session().unwrap();
                return Ok(json!({
                    "access_token":"access-2", "refresh_token":"refresh-2",
                    "user":{"id":self.user_id.clone(),"username":"u"}
                }));
            }
            if url.ends_with("/v1/auth/logout") {
                self.revoke_calls.set(self.revoke_calls.get() + 1);
            }
            Ok(json!({"status":"ok"}))
        }
    }

    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "solum-account-logout-race-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
    let session = state_test_session();
    publish_session(&session).unwrap();
    let fake = LogoutRaceFake {
        revoke_calls: Cell::new(0),
        user_id: session.user_id.clone(),
    };
    assert!(refresh_and_save_with_client(&fake, &session).is_err());
    assert!(AccountSession::load().is_none());
    assert_eq!(
        fake.revoke_calls.get(),
        1,
        "newly issued pair must be revoked"
    );

    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_rejects_missing_fields_and_bad_urls() {
    let missing = r#"{"server_url":"https://x.example","username":"u","access_token":"","refresh_token":"r"}"#;
    assert!(AccountSession::from_json(missing).is_err());
    let plain_http = r#"{"server_url":"http://10.0.0.2:8787","username":"u","access_token":"a","refresh_token":"r"}"#;
    assert!(AccountSession::from_json(plain_http).is_err());
    let loopback = r#"{"server_url":"http://127.0.0.1:8787","username":"u","access_token":"a","refresh_token":"r"}"#;
    assert!(AccountSession::from_json(loopback).is_ok());
}

#[test]
fn model_validation_mirrors_server() {
    assert_eq!(normalize_cloud_model("").unwrap(), DEFAULT_CLOUD_MODEL);
    assert_eq!(
        normalize_cloud_model(" mimo-v2.5-pro ").unwrap(),
        "mimo-v2.5-pro"
    );
    assert!(normalize_cloud_model("bad model").is_err());
    assert!(normalize_cloud_model(&"x".repeat(101)).is_err());
}

#[test]
fn stable_user_id_requires_a_lowercase_uuid() {
    assert!(is_valid_user_id("9d4df1be-9f7b-4a3a-b986-ec920d2df60e"));
    assert!(!is_valid_user_id("alice"));
    assert!(!is_valid_user_id("9D4DF1BE-9F7B-4A3A-B986-EC920D2DF60E"));
}

#[test]
fn auth_response_missing_tokens_is_an_error_not_a_broken_session() {
    let bad = json!({ "user": { "username": "u" }, "access_token": "a" });
    assert!(parse_auth_response(&bad, "https://x.example", "u", "mimo-v2.5").is_err());
    let good = json!({ "user": { "username": "real" }, "access_token": "a", "refresh_token": "r" });
    let session = parse_auth_response(&good, "https://x.example", "fallback", "mimo-v2.5").unwrap();
    assert_eq!(session.username, "real");
}

/// Scripted fake: first authorized call 401s, refresh rotates the pair,
/// retry succeeds — and the rotated pair is what the session ends up with.
struct RotatingFake {
    calls: RefCell<Vec<String>>,
}

impl AccountHttp for RotatingFake {
    fn post_json(
        &self,
        url: &str,
        access_token: Option<&str>,
        _body: Value,
    ) -> std::result::Result<Value, AccountHttpError> {
        self.calls.borrow_mut().push(url.to_string());
        if url.ends_with("/v1/auth/refresh") {
            return Ok(json!({
                    "access_token": "access-2",
                    "refresh_token": "refresh-2",
                    "user": {
                        "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                        "username": "u"
                    },
            }));
        }
        match access_token {
            Some("access-2") => Ok(json!({
                "choices": [ { "message": { "content": "pong" } } ]
            })),
            _ => Err(AccountHttpError::Unauthorized),
        }
    }
}

#[test]
fn expired_access_token_refreshes_once_and_retries() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    // 会话文件落到临时目录，避免测试写进仓库工作区。
    let dir = std::env::temp_dir().join(format!("solum-account-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var(
        "SOLUM_ACCOUNT_CONFIG",
        dir.join("solum-account.json").to_string_lossy().to_string(),
    );
    let mut session = AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
        username: "u".into(),
        access_token: "access-1".into(),
        refresh_token: "refresh-1".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    };
    publish_session(&session).unwrap();
    let fake = RotatingFake {
        calls: RefCell::new(Vec::new()),
    };
    let reply = AccountReasoner::complete_with_client(&fake, &mut session, "s", "u").unwrap();
    assert_eq!(reply, "pong");
    assert_eq!(session.access_token, "access-2");
    assert_eq!(session.refresh_token, "refresh-2");
    let calls = fake.calls.borrow();
    assert_eq!(calls.len(), 3, "chat → refresh → chat retry");
    assert!(calls[0].ends_with("/v1/ai/chat/completions"));
    assert!(calls[1].ends_with("/v1/auth/refresh"));
    assert!(calls[2].ends_with("/v1/ai/chat/completions"));
    // 轮换后的会话已持久化（post_with_refresh 立即落盘，重试失败也不丢新令牌）。
    let persisted = AccountSession::load().expect("rotated session persisted");
    assert_eq!(persisted.access_token, "access-2");
    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

struct StateFake {
    calls: RefCell<Vec<String>>,
    omit_default: bool,
}

impl AccountHttp for StateFake {
    fn post_json(
        &self,
        url: &str,
        _access_token: Option<&str>,
        _body: Value,
    ) -> std::result::Result<Value, AccountHttpError> {
        self.calls.borrow_mut().push(url.to_string());
        Ok(json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "user": {
                "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                "username": "u"
            }
        }))
    }

    fn get_json(
        &self,
        url: &str,
        access_token: &str,
    ) -> std::result::Result<Value, AccountHttpError> {
        self.calls.borrow_mut().push(url.to_string());
        if access_token == "access-1" {
            return Err(AccountHttpError::Unauthorized);
        }
        if url.ends_with("/v1/ai/models") {
            let models = if self.omit_default {
                json!([{ "id": "mimo-v2.5-pro", "is_default": false }])
            } else {
                json!([
                    { "id": "mimo-v2.5", "is_default": true },
                    { "id": "mimo-v2.5-pro", "is_default": false }
                ])
            };
            return Ok(json!({
                "default_model": "mimo-v2.5",
                "models": models,
                "custom_models_enabled": true
            }));
        }
        Ok(json!({
            "plan": "standard",
            "custom_models_enabled": true,
            "monthly_request_limit": 1000,
            "requests_used": 7,
            "requests_remaining": 993,
            "reset_at": "2026-09-01T00:00:00.000Z",
            "success_count": 6,
            "failure_count": 1,
            "fallback_count": 1,
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150,
            "latest_request": null
        }))
    }
}

fn state_test_session() -> AccountSession {
    AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
        username: "u".into(),
        access_token: "access-1".into(),
        refresh_token: "refresh-1".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    }
}

#[test]
fn cloud_state_refreshes_once_then_reads_catalog_and_usage() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("solum-account-state-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
    let fake = StateFake {
        calls: RefCell::new(Vec::new()),
        omit_default: false,
    };
    let mut session = state_test_session();
    publish_session(&session).unwrap();
    let state = cloud_account_state_with_client(&fake, &mut session).unwrap();
    assert_eq!(session.access_token, "access-2");
    assert_eq!(state.catalog.models.len(), 2);
    assert_eq!(state.usage.requests_remaining, 993);
    let calls = fake.calls.borrow();
    assert!(calls[0].ends_with("/v1/ai/models"));
    assert!(calls[1].ends_with("/v1/auth/refresh"));
    assert!(calls[2].ends_with("/v1/ai/models"));
    assert!(calls[3].ends_with("/v1/ai/usage"));
    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cloud_state_rejects_a_catalog_without_its_default_model() {
    let fake = StateFake {
        calls: RefCell::new(Vec::new()),
        omit_default: true,
    };
    let mut session = state_test_session();
    session.access_token = "access-2".into();
    assert!(cloud_account_state_with_client(&fake, &mut session)
        .unwrap_err()
        .to_string()
        .contains("缺少默认模型"));
}

#[test]
fn legacy_refresh_requires_an_explicit_login() {
    let client = RotatingFake {
        calls: RefCell::new(Vec::new()),
    };
    let legacy = AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: String::new(),
        username: "u".into(),
        access_token: "access-1".into(),
        refresh_token: "refresh-1".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    };
    let error = refresh_with_client(&client, &legacy).unwrap_err();
    assert!(error.to_string().contains("重新登录"));
}

#[test]
fn refresh_retries_one_transient_response_failure() {
    struct TransientRefresh {
        calls: Cell<usize>,
    }
    impl AccountHttp for TransientRefresh {
        fn post_json(
            &self,
            _url: &str,
            _token: Option<&str>,
            _body: Value,
        ) -> std::result::Result<Value, AccountHttpError> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            if call == 1 {
                return Err(AccountHttpError::Retryable("connection reset".into()));
            }
            Ok(json!({
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "user": {
                    "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                    "username": "u"
                }
            }))
        }
    }
    let client = TransientRefresh {
        calls: Cell::new(0),
    };
    let rotated = refresh_with_client(&client, &state_test_session()).unwrap();
    assert_eq!(client.calls.get(), 2);
    assert_eq!(rotated.refresh_token, "refresh-2");
}

#[test]
fn refresh_failure_maps_to_relogin_message() {
    let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
    struct AlwaysUnauthorized;
    impl AccountHttp for AlwaysUnauthorized {
        fn post_json(
            &self,
            _url: &str,
            _token: Option<&str>,
            _body: Value,
        ) -> std::result::Result<Value, AccountHttpError> {
            Err(AccountHttpError::Unauthorized)
        }
    }
    let mut session = AccountSession {
        server_url: "https://cloud.example.com".into(),
        user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
        username: "refresh-failure-test".into(),
        access_token: "a".into(),
        refresh_token: "r".into(),
        model: DEFAULT_CLOUD_MODEL.into(),
    };
    let dir = std::env::temp_dir().join(format!(
        "solum-account-refresh-failure-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
    publish_session(&session).unwrap();
    let err = AccountReasoner::complete_with_client(&AlwaysUnauthorized, &mut session, "s", "u")
        .unwrap_err();
    assert!(err.to_string().contains("重新登录"), "{err}");
    clear_session().unwrap();
    std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn generate_pkce_creates_valid_s256_challenge() {
    let pkce = generate_pkce().unwrap();
    assert_eq!(pkce.code_challenge_method, "S256");
    assert_eq!(pkce.code_verifier.len(), 43);
    assert_eq!(pkce.code_challenge.len(), 43);
    assert!(!pkce.code_verifier.contains('='));
    assert!(!pkce.code_challenge.contains('='));

    let mut hasher = Sha256::new();
    hasher.update(pkce.code_verifier.as_bytes());
    let expected_challenge = B64_URL.encode(hasher.finalize());
    assert_eq!(pkce.code_challenge, expected_challenge);
}

#[test]
fn parse_auth_response_handles_oauth_payload() {
    let payload = json!({
        "access_token": "oauth-access-token",
        "refresh_token": "oauth-refresh-token",
        "user": {
            "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
            "username": "tg_user"
        }
    });
    let session = parse_auth_response(&payload, "https://tannerlab.cn", "", "qwen3.5-2b").unwrap();
    assert_eq!(session.server_url, SOLUM_CENTER_URL);
    assert_eq!(session.user_id, "9d4df1be-9f7b-4a3a-b986-ec920d2df60e");
    assert_eq!(session.username, "tg_user");
    assert_eq!(session.access_token, "oauth-access-token");
    assert_eq!(session.refresh_token, "oauth-refresh-token");
}
