//! `oakum status`: emit versioned release state, never deliver it (ADR-0016).

use clap::Args;

use oakum::state::RenderTarget;

use super::config::load_config;
use super::release_state::{release_state, CoverageMode};
use super::render::render_summary;
use super::repository;
use super::CliError;
use super::{deliver_block, deliver_out, say_err};

#[derive(Debug, Args)]
pub(super) struct StatusArgs {
    /// Print the versioned `ReleaseState` JSON document instead of a render.
    #[arg(long, conflicts_with = "template")]
    json: bool,
    /// Named render. Only `summary` is built in, and it is the default.
    #[arg(long, value_name = "NAME")]
    template: Option<String>,
    /// Git ref to scan from (exclusive). Same default as `generate` / `check`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

pub(super) fn run(args: &StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let target = presentation(args)?;
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    if config.is_default() {
        say_err(super::config::DEFAULTS_NOTE);
    }
    let state = release_state(
        &repo,
        &config,
        args.from.as_deref(),
        target,
        CoverageMode::Reported,
    )?;
    if args.json {
        deliver_out(&serde_json::to_string_pretty(&state)?)
            .map_err(|err| CliError::undelivered("report", &err))?;
        return Ok(());
    }
    deliver_block(&render_summary(&state)).map_err(|err| CliError::undelivered("report", &err))?;
    Ok(())
}

fn presentation(args: &StatusArgs) -> Result<RenderTarget, CliError> {
    if args.json {
        return Ok(RenderTarget::Status);
    }
    match args.template.as_deref().unwrap_or("summary") {
        "summary" => Ok(RenderTarget::Summary),
        name => Err(CliError::new(format!(
            "unknown template `{name}`; known: summary"
        ))),
    }
}
