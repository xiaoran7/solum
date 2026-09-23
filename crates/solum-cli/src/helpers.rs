use anyhow::{anyhow, Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime, Timelike};
use solum_core::model::MemoryLayer;
use solum_core::Orchestrator;
use std::io::{BufRead, Write};

/// Preview first; EOF, blank input, and anything except an explicit `yes`
/// cancel without minting a token. Re-read the clock after the user responds.
pub(crate) fn confirm_and_run_tool(
    o: &mut Orchestrator,
    tool: &str,
    args: &str,
    input: &mut impl BufRead,
    output: &mut impl Write,
    mut clock: impl FnMut() -> Result<NaiveDateTime>,
) -> Result<()> {
    let pending = o
        .request_confirmation(tool, args, clock()?)
        .map_err(to_err)?;
    writeln!(output, "{}", pending.request.effect_preview)?;
    write!(output, "输入 yes 确认执行，其他输入或回车取消：")?;
    output.flush()?;
    let mut answer = String::new();
    // Bound confirmation input; a longer line cannot equal the accepted word.
    std::io::Read::take(input, 64).read_line(&mut answer)?;
    if answer.trim() != "yes" {
        writeln!(output, "已取消，未执行删除。")?;
        return Ok(());
    }
    let token = o.confirm(&pending.id, clock()?).map_err(to_err)?;
    let message = o
        .run_tool(tool, args, Some(token), clock()?)
        .map_err(to_err)?;
    writeln!(output, "{message}")?;
    Ok(())
}

pub(crate) fn to_err(e: solum_core::CoreError) -> anyhow::Error {
    anyhow!(e.to_string())
}

pub(crate) fn parse_layer(s: &str) -> Result<MemoryLayer> {
    Ok(match s.to_lowercase().as_str() {
        "raw_input" | "raw" | "input" => MemoryLayer::RawInput,
        "event" | "ev" => MemoryLayer::Event,
        "notification" | "notif" | "n" => MemoryLayer::Notification,
        "notification_capture" | "capture" => MemoryLayer::NotificationCapture,
        "behavior" | "log" | "b" => MemoryLayer::Behavior,
        "suggestion" | "suggest" | "s" => MemoryLayer::Suggestion,
        "wearable" | "health" | "w" => MemoryLayer::Wearable,
        "fact" | "f" | "memory" => MemoryLayer::Fact,
        "routine" | "r" => MemoryLayer::Routine,
        other => return Err(anyhow!("unknown memory layer: {other}")),
    })
}

pub(crate) fn resolve_now(opt: Option<&str>) -> Result<NaiveDateTime> {
    match opt {
        None => Ok(Local::now().naive_local().with_second_zero()),
        Some(s) => parse_flexible(s).with_context(|| format!("could not parse --now {s:?}")),
    }
}

pub(crate) fn parse_flexible(s: &str) -> Result<NaiveDateTime> {
    let s = s.trim();
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(dt);
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap());
    }
    Err(anyhow!("unrecognized datetime format"))
}

/// Small helper: zero out sub-minute noise so display is clean.
pub(crate) trait SecondZero {
    fn with_second_zero(self) -> Self;
}
impl SecondZero for NaiveDateTime {
    fn with_second_zero(self) -> Self {
        self.with_second(0)
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(self)
    }
}
