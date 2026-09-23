mod args;
mod dispatch;
mod helpers;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use clap::Parser;
use solum_core::Orchestrator;
use std::path::Path;

use crate::args::Cli;
use crate::dispatch::dispatch;
use crate::helpers::resolve_now;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let now = resolve_now(cli.now.as_deref())?;

    if solum_core::store::adopt_legacy_db(Path::new("pa.sqlite"), Path::new("solum.sqlite"))
        .map_err(|e| anyhow!("adopt pre-rename store: {e}"))?
    {
        eprintln!("已接管改名前的数据库 pa.sqlite → solum.sqlite");
    }
    let db = match &cli.db {
        Some(p) => p.clone(),
        None => solum_core::paths::resolve_profile_with_adoption("solum.sqlite")
            .to_string_lossy()
            .into_owned(),
    };
    let mut o = Orchestrator::open(&db).map_err(|e| anyhow!("open store: {e}"))?;

    let llm_cfg = solum_core::llm::LlmConfig::load();
    if let Some(cfg) = &llm_cfg {
        o.set_reasoner(Box::new(solum_core::llm::LlmReasoner::new(cfg.clone())));
    }

    dispatch(cli.cmd, o, now, cli.now.as_deref(), llm_cfg.as_ref())
}
