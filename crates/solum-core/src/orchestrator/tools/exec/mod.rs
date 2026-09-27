//! `exec` -- the guarded execution entry points on `Orchestrator`: list a tool's names and risk, ask the guard for a confirmation, mint the one-time token, and run. The run is the only place that drains the guard's audit log into the store, and it reloads the caches afterwards because a tool writes through `ToolCtx.store` — i.e. behind this orchestrator's back — so anything it cached may now be stale.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

impl Orchestrator {
    // ---- guarded tool execution ------------------------------------------

    pub fn tool_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tools.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn tool_risk(&self, name: &str) -> Option<RiskLevel> {
        self.tools.get(name).map(|t| t.risk_level())
    }

    /// Step 1 of the HITL flow: describe an action and get a pending
    /// confirmation. Nothing executes.
    pub fn request_confirmation(
        &mut self,
        tool: &str,
        args: &str,
        now: NaiveDateTime,
    ) -> Result<PendingConfirmation> {
        let t = self
            .tools
            .get(tool)
            .ok_or_else(|| CoreError::NotFound(format!("tool {tool}")))?;
        let ctx = ToolCtx {
            store: &self.store,
            now,
        };
        Ok(self.guard.request_confirmation(t.as_ref(), args, now, &ctx))
    }

    /// Step 2: human approves → mint a one-time token.
    pub fn confirm(&mut self, pending_id: &str, now: NaiveDateTime) -> Result<ExecutionToken> {
        self.guard.confirm(pending_id, now)
    }

    /// Step 3: run the tool. Persists audit records (executed or refused) to the
    /// append-only log regardless of outcome.
    pub fn run_tool(
        &mut self,
        tool: &str,
        args: &str,
        token: Option<ExecutionToken>,
        now: NaiveDateTime,
    ) -> Result<String> {
        let t = self
            .tools
            .get(tool)
            .ok_or_else(|| CoreError::NotFound(format!("tool {tool}")))?;
        let ctx = ToolCtx {
            store: &self.store,
            now,
        };
        let res = self.guard.run(t.as_ref(), args, token, now, &ctx);
        for entry in self.guard.drain_audit() {
            self.store.append_audit(&entry)?;
        }
        // A tool writes through `ToolCtx.store`, i.e. behind this orchestrator's
        // back, so anything it cached may now be stale. `persona_clear` is the
        // concrete case: without this the deleted persona stays live in memory
        // and keeps shaping replies until the next restart.
        if res.is_ok() {
            self.reload_caches()?;
        }
        res
    }
}
