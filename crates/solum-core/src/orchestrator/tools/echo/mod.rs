//! `echo` -- the two tools that report rather than change: an echo, and a delete that is simulated end to end. The simulated one is here rather than with the real deletions because its only job is to let the HITL flow be exercised without a store write, and reading it next to the echo is what makes that clear.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// A trivially safe tool.
pub(super) struct EchoTool;
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Safe
    }
    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        format!("回显：{args}")
    }
    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        Ok(args.to_string())
    }
}

/// A **simulated** dangerous tool for demonstrating the guard end-to-end. It
/// deliberately does not touch anything real; kept alongside `ledger_purge`
/// as the harmless demo surface.
pub(super) struct SimulatedDeleteTool;
impl Tool for SimulatedDeleteTool {
    fn name(&self) -> &str {
        "demo_delete"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        format!("将永久删除（模拟，不会真正删除）：{args}")
    }
    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        Ok(format!("（模拟）已删除：{args}"))
    }
}
