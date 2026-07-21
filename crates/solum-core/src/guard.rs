//! HITL (human-in-the-loop) safety guard — ARCHITECTURE.md §3.3, F7.
//!
//! The core guarantee is structural, not advisory: a `Dangerous` tool **cannot
//! be executed** without a [`Grant`], and a `Grant` can only be minted inside
//! this module by [`Guard::run`] after a valid one-time [`ExecutionToken`] has
//! been presented. No prompt, persona, or proactivity level can fabricate a
//! grant, because nothing outside this module can construct one.
//!
//! Flow for a dangerous action:
//! 1. `request_confirmation` → returns a [`PendingConfirmation`] with an effect
//!    preview (nothing has run, no token exists yet);
//! 2. human approves → `confirm` mints a single-use [`ExecutionToken`] bound to
//!    the exact action fingerprint **and to a digest of the previewed effect**;
//! 3. `run` with that token re-previews the action and authorizes exactly one
//!    execution **only if the effect still matches what the human saw**, then
//!    burns the token.
//!
//! Step 3's re-preview is what makes the confirmation mean something. Without
//! it a token says only "the human approved `ledger_purge(before=X)`", not "the
//! human approved deleting *those 12 rows*" — and between preview and execution
//! the store can change (sync pulls, new captures, another window). A stale
//! confirmation would then authorize a strictly larger deletion than the one
//! shown. Both the pending confirmation and the minted token therefore expire.
//!
//! Every dangerous attempt — executed **or refused** — is recorded in an
//! append-only audit trail.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use chrono::{Duration, NaiveDateTime};

use crate::error::{CoreError, Result};
use crate::model::RiskLevel;

/// One-time tokens expire this long after being minted.
const TOKEN_TTL_MINUTES: i64 = 5;

/// A pending confirmation the human never answered goes stale this long after
/// being requested. Longer than [`TOKEN_TTL_MINUTES`] because a human is
/// reading a preview here, but still bounded: an hours-old preview describes a
/// store that no longer exists.
const PENDING_TTL_MINUTES: i64 = 10;

/// Proof of authorization. Only [`Guard::run`] can build one, so a tool's
/// `execute` is unreachable without the guard's blessing.
#[non_exhaustive]
pub struct Grant {
    _private: (),
}

/// What a tool may touch while previewing/executing. Real tools act on the
/// local store (D3: `ledger_purge`); the orchestrator lends its store for the
/// duration of the call. Previews get the same context so they can show real
/// consequences ("将删除 12 条"), but the trait contract still forbids them
/// from mutating.
pub struct ToolCtx<'a> {
    pub store: &'a crate::store::Store,
    /// Injected clock (ARCHITECTURE.md: solum-core never reads the system
    /// time). Tools that need to reason about "now" — an import capping a
    /// backup's self-reported timestamp, for instance — take it from here.
    pub now: NaiveDateTime,
}

/// A tool the agent can invoke. Dangerous tools receive a [`Grant`] they cannot
/// forge; the guard is the only minter.
///
/// `Send` is required so the orchestrator (which owns the tool registry) can be
/// shared behind a mutex with UI threads (e.g. the Tauri shell's managed state).
pub trait Tool: Send {
    fn name(&self) -> &str;
    fn risk_level(&self) -> RiskLevel;
    /// Short description shown in a confirmation request. Implementations may
    /// omit raw arguments when they contain private user data.
    fn request_summary(&self, args: &str) -> String {
        format!("{}({args})", self.name())
    }
    /// What is allowed into append-only audit storage. Defaults to the request
    /// summary for existing tools; privacy-sensitive outbound tools override it
    /// so an audit trail does not become a second copy of their payload.
    fn audit_summary(&self, args: &str) -> String {
        self.request_summary(args)
    }
    /// Human-readable description of what *would* happen — shown at confirmation
    /// time. Must not mutate anything.
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String;
    /// Perform the action. The `_grant` proves the guard authorized this call.
    fn execute(&self, args: &str, grant: &Grant, ctx: &ToolCtx) -> Result<String>;
}

fn fingerprint(tool: &str, args: &str) -> u64 {
    let mut h = DefaultHasher::new();
    tool.hash(&mut h);
    0u8.hash(&mut h); // separator
    args.hash(&mut h);
    h.finish()
}

/// Digest of a rendered effect preview. Two runs of the same action against the
/// same store state produce the same digest; any change the preview is able to
/// describe (row counts, target titles, times) changes it.
fn effect_digest(preview: &str) -> u64 {
    let mut h = DefaultHasher::new();
    preview.hash(&mut h);
    h.finish()
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("system RNG available");
    let mut s = String::with_capacity(bytes * 2);
    for b in buf {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A described, not-yet-authorized action.
#[derive(Debug, Clone)]
pub struct ActionRequest {
    pub tool: String,
    pub risk: RiskLevel,
    pub summary: String,
    pub effect_preview: String,
}

/// Returned to the UI so a human can decide. Holds no execution power.
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub id: String,
    pub request: ActionRequest,
    /// When the confirmation was requested. Confirmations go stale after
    /// [`PENDING_TTL_MINUTES`].
    pub requested_at: NaiveDateTime,
    fingerprint: u64,
    /// Digest of `request.effect_preview` — what the human is being shown.
    effect_digest: u64,
}

/// A single-use authorization to run one specific action, with the effect the
/// human approved pinned to it.
#[derive(Debug, Clone)]
pub struct ExecutionToken {
    pub id: String,
    fingerprint: u64,
    /// Digest of the effect preview shown at confirmation time. `run`
    /// re-previews and refuses on mismatch.
    effect_digest: u64,
    created_at: NaiveDateTime,
}

/// What the guard decided about an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Executed,
    Refused,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Executed => "executed",
            Decision::Refused => "refused",
        }
    }
}

/// An append-only audit record. The guard never mutates or deletes these.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub ts: NaiveDateTime,
    pub tool: String,
    pub risk: RiskLevel,
    pub summary: String,
    pub decision: Decision,
    pub token_id: Option<String>,
    pub detail: String,
}

/// The guard. Holds pending confirmations, live one-time tokens, and the
/// in-memory audit buffer (which the caller drains and persists to the
/// append-only store).
#[derive(Default)]
pub struct Guard {
    pending: HashMap<String, PendingConfirmation>,
    tokens: HashMap<String, ExecutionToken>,
    audit: Vec<AuditEntry>,
}

impl Guard {
    pub fn new() -> Self {
        Guard::default()
    }

    /// Step 1: describe an action and get a pending confirmation. Nothing runs.
    pub fn request_confirmation(
        &mut self,
        tool: &dyn Tool,
        args: &str,
        now: NaiveDateTime,
        ctx: &ToolCtx,
    ) -> PendingConfirmation {
        let preview = tool.preview(args, ctx);
        let request = ActionRequest {
            tool: tool.name().to_string(),
            risk: tool.risk_level(),
            summary: tool.request_summary(args),
            effect_preview: preview.clone(),
        };
        let pending = PendingConfirmation {
            id: random_hex(8),
            request,
            requested_at: now,
            fingerprint: fingerprint(tool.name(), args),
            effect_digest: effect_digest(&preview),
        };
        self.pending.insert(pending.id.clone(), pending.clone());
        pending
    }

    /// Step 2: a human approved the pending confirmation → mint a one-time
    /// token bound to that exact action *and* to the effect they were shown.
    ///
    /// A confirmation older than [`PENDING_TTL_MINUTES`] is dropped rather than
    /// honoured: the preview it was based on is no longer trustworthy, so the
    /// caller must request a fresh one.
    pub fn confirm(&mut self, pending_id: &str, now: NaiveDateTime) -> Result<ExecutionToken> {
        let pending = self.pending.remove(pending_id).ok_or_else(|| {
            CoreError::GuardRefused(format!("no pending confirmation {pending_id}"))
        })?;
        if now - pending.requested_at > Duration::minutes(PENDING_TTL_MINUTES) {
            return Err(CoreError::GuardRefused(
                "确认已过期（预览距今超过 10 分钟），请重新预览后再确认".into(),
            ));
        }
        let token = ExecutionToken {
            id: random_hex(16),
            fingerprint: pending.fingerprint,
            effect_digest: pending.effect_digest,
            created_at: now,
        };
        self.tokens.insert(token.id.clone(), token.clone());
        Ok(token)
    }

    /// Drop pending confirmations that have gone stale. Called opportunistically
    /// so an abandoned preview cannot sit in memory indefinitely.
    fn sweep_pending(&mut self, now: NaiveDateTime) {
        let ttl = Duration::minutes(PENDING_TTL_MINUTES);
        self.pending.retain(|_, p| now - p.requested_at <= ttl);
    }

    /// Step 3: run the action. Safe tools run freely; Sensitive/Dangerous tools
    /// require a valid, unexpired, matching one-time token. The token is burned
    /// on use. Every non-safe attempt is audited, including refusals.
    pub fn run(
        &mut self,
        tool: &dyn Tool,
        args: &str,
        token: Option<ExecutionToken>,
        now: NaiveDateTime,
        ctx: &ToolCtx,
    ) -> Result<String> {
        let risk = tool.risk_level();
        let summary = tool.audit_summary(args);
        self.sweep_pending(now);

        if risk == RiskLevel::Safe {
            // Safe actions need no confirmation.
            let grant = Grant { _private: () };
            return tool.execute(args, &grant, ctx);
        }

        // Non-safe: a valid token is mandatory.
        let refuse = |guard: &mut Self, reason: &str, tid: Option<String>| -> CoreError {
            guard.audit.push(AuditEntry {
                ts: now,
                tool: tool.name().to_string(),
                risk,
                summary: summary.clone(),
                decision: Decision::Refused,
                token_id: tid,
                detail: reason.to_string(),
            });
            CoreError::GuardRefused(reason.to_string())
        };

        let Some(token) = token else {
            return Err(refuse(self, "no execution token presented", None));
        };
        // Validate the token against our live set (prevents forged/replayed tokens).
        let Some(live) = self.tokens.get(&token.id).cloned() else {
            return Err(refuse(
                self,
                "unknown or already-used token",
                Some(token.id),
            ));
        };
        if live.fingerprint != fingerprint(tool.name(), args) {
            return Err(refuse(
                self,
                "token does not match this action",
                Some(token.id),
            ));
        }
        if now - live.created_at > Duration::minutes(TOKEN_TTL_MINUTES) {
            self.tokens.remove(&token.id);
            return Err(refuse(self, "token expired", Some(token.id)));
        }
        // The effect the human approved must still be the effect we are about to
        // cause. `preview` is contractually side-effect-free, so re-running it
        // here is cheap and safe — and it is the only thing standing between a
        // confirmation of "delete 12 rows" and an execution that deletes 500
        // because the store changed in between.
        if effect_digest(&tool.preview(args, ctx)) != live.effect_digest {
            self.tokens.remove(&token.id);
            return Err(refuse(
                self,
                "确认已失效：目标数据在预览之后发生变化，请重新预览并确认",
                Some(token.id),
            ));
        }

        // Burn the token — single use.
        self.tokens.remove(&token.id);

        let grant = Grant { _private: () };
        let result = tool.execute(args, &grant, ctx);
        self.audit.push(AuditEntry {
            ts: now,
            tool: tool.name().to_string(),
            risk,
            summary,
            decision: Decision::Executed,
            token_id: Some(token.id),
            detail: match &result {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("tool error: {e}"),
            },
        });
        result
    }

    /// Drain audit records so the caller can persist them to the append-only
    /// store. The guard keeps none after draining.
    pub fn drain_audit(&mut self) -> Vec<AuditEntry> {
        std::mem::take(&mut self.audit)
    }

    /// Peek at the audit buffer without draining (tests / diagnostics).
    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::cell::Cell;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 6)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn test_store() -> crate::store::Store {
        crate::store::Store::open_in_memory().unwrap()
    }

    /// A dangerous tool that records whether it ran — without touching the real
    /// filesystem. `ran` proves execution actually happened (or didn't).
    struct FakeDelete {
        ran: Cell<bool>,
    }
    impl Tool for FakeDelete {
        fn name(&self) -> &str {
            "delete_file"
        }
        fn risk_level(&self) -> RiskLevel {
            RiskLevel::Dangerous
        }
        fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
            format!("将永久删除: {args}")
        }
        fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
            self.ran.set(true);
            Ok(format!("deleted {args}"))
        }
    }

    struct SafeEcho;
    impl Tool for SafeEcho {
        fn name(&self) -> &str {
            "echo"
        }
        fn risk_level(&self) -> RiskLevel {
            RiskLevel::Safe
        }
        fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
            format!("echo {args}")
        }
        fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
            Ok(args.to_string())
        }
    }

    #[test]
    fn safe_runs_without_token() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        assert_eq!(g.run(&SafeEcho, "hi", None, now(), &ctx).unwrap(), "hi");
        assert!(g.audit().is_empty()); // safe actions aren't audited
    }

    #[test]
    fn dangerous_refused_without_token() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        let err = g.run(&tool, "/important", None, now(), &ctx).unwrap_err();
        assert!(matches!(err, CoreError::GuardRefused(_)));
        assert!(!tool.ran.get(), "tool must not have executed");
        assert_eq!(g.audit().len(), 1);
        assert_eq!(g.audit()[0].decision, Decision::Refused);
    }

    #[test]
    fn dangerous_runs_after_confirm() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        let pending = g.request_confirmation(&tool, "/tmp/x", now(), &ctx);
        assert!(pending.request.effect_preview.contains("永久删除"));
        let token = g.confirm(&pending.id, now()).unwrap();
        let out = g.run(&tool, "/tmp/x", Some(token), now(), &ctx).unwrap();
        assert_eq!(out, "deleted /tmp/x");
        assert!(tool.ran.get());
        assert_eq!(g.audit()[0].decision, Decision::Executed);
    }

    #[test]
    fn token_is_single_use() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        let pending = g.request_confirmation(&tool, "/tmp/x", now(), &ctx);
        let token = g.confirm(&pending.id, now()).unwrap();
        g.run(&tool, "/tmp/x", Some(token.clone()), now(), &ctx)
            .unwrap();
        // Reusing the same token must fail.
        let err = g
            .run(&tool, "/tmp/x", Some(token), now(), &ctx)
            .unwrap_err();
        assert!(matches!(err, CoreError::GuardRefused(_)));
    }

    #[test]
    fn token_does_not_authorize_a_different_action() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        let pending = g.request_confirmation(&tool, "/tmp/x", now(), &ctx);
        let token = g.confirm(&pending.id, now()).unwrap();
        // Same tool, different args → fingerprint mismatch → refused.
        let err = g
            .run(&tool, "/tmp/OTHER", Some(token), now(), &ctx)
            .unwrap_err();
        assert!(matches!(err, CoreError::GuardRefused(_)));
        assert!(!tool.ran.get());
    }

    #[test]
    fn token_expires() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        let pending = g.request_confirmation(&tool, "/tmp/x", now(), &ctx);
        let token = g.confirm(&pending.id, now()).unwrap();
        let later = now() + Duration::minutes(TOKEN_TTL_MINUTES + 1);
        let err = g
            .run(&tool, "/tmp/x", Some(token), later, &ctx)
            .unwrap_err();
        assert!(matches!(err, CoreError::GuardRefused(_)));
        assert!(!tool.ran.get());
    }

    /// A dangerous tool whose preview depends on mutable outside state — the
    /// shape of every real bulk tool (`ledger_purge` counts rows).
    struct CountingPurge {
        rows: Cell<u32>,
        ran_with: Cell<u32>,
    }
    impl Tool for CountingPurge {
        fn name(&self) -> &str {
            "counting_purge"
        }
        fn risk_level(&self) -> RiskLevel {
            RiskLevel::Dangerous
        }
        fn preview(&self, _args: &str, _ctx: &ToolCtx) -> String {
            format!("将删除 {} 条", self.rows.get())
        }
        fn execute(&self, _args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
            self.ran_with.set(self.rows.get());
            Ok(format!("已删除 {} 条", self.rows.get()))
        }
    }

    #[test]
    fn confirmation_does_not_survive_a_change_in_effect() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = CountingPurge {
            rows: Cell::new(12),
            ran_with: Cell::new(0),
        };
        let pending = g.request_confirmation(&tool, "{}", now(), &ctx);
        assert!(pending.request.effect_preview.contains("12"));
        let token = g.confirm(&pending.id, now()).unwrap();

        // Store grows between confirmation and execution (sync pull, new rows…).
        tool.rows.set(500);

        let err = g.run(&tool, "{}", Some(token), now(), &ctx).unwrap_err();
        assert!(matches!(err, CoreError::GuardRefused(_)));
        assert_eq!(tool.ran_with.get(), 0, "must not have deleted the 500 rows");
        assert_eq!(g.audit()[0].decision, Decision::Refused);
    }

    #[test]
    fn unchanged_effect_still_executes() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = CountingPurge {
            rows: Cell::new(12),
            ran_with: Cell::new(0),
        };
        let pending = g.request_confirmation(&tool, "{}", now(), &ctx);
        let token = g.confirm(&pending.id, now()).unwrap();
        g.run(&tool, "{}", Some(token), now(), &ctx).unwrap();
        assert_eq!(tool.ran_with.get(), 12);
    }

    #[test]
    fn stale_pending_confirmation_cannot_be_confirmed() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        let pending = g.request_confirmation(&tool, "/tmp/x", now(), &ctx);
        let much_later = now() + Duration::minutes(PENDING_TTL_MINUTES + 1);
        let err = g.confirm(&pending.id, much_later).unwrap_err();
        assert!(matches!(err, CoreError::GuardRefused(_)));
    }

    #[test]
    fn drain_audit_clears_buffer() {
        let store = test_store();
        let ctx = ToolCtx {
            store: &store,
            now: now(),
        };
        let mut g = Guard::new();
        let tool = FakeDelete {
            ran: Cell::new(false),
        };
        g.run(&tool, "/x", None, now(), &ctx).ok();
        assert_eq!(g.drain_audit().len(), 1);
        assert!(g.audit().is_empty());
    }
}
