mod add;
mod config;
mod coverage;
mod generate;
mod github_output;
mod install_pin;
mod intent;
mod preconditions;
mod repository;
mod status;
mod tags;
mod upgrade;

use std::ffi::OsString;
use std::fmt;

use clap::{Parser, Subcommand};

/// Polyglot release tool: version math, changelogs, and graph-derived bumps.
#[derive(Debug, Parser)]
#[command(name = "oakum", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Write one `.changeset/*.md` bump file.
    Add(add::AddArgs),
    /// Report drift and name the fix.
    Check(preconditions::CheckArgs),
    /// Derive one `.changeset/*.md` from commits on this branch.
    Generate(generate::GenerateArgs),
    /// Print plan bump-file inputs as JSON (hidden plumbing for tests).
    #[command(name = "plan-intent", hide = true)]
    PlanIntent(intent::PlanIntentArgs),
    /// Print the versioned release state as JSON or a named render.
    Status(status::StatusArgs),
    /// Print tags reachable from HEAD as `commit\\ttag`.
    #[command(name = "reachable-tags", hide = true)]
    ReachableTags,
    /// Tag-only readiness path (no coverage).
    #[command(name = "tag-drift", hide = true)]
    TagDrift,
    /// Migrate `.changeset/_config.toml` and `_schema.json` to this binary.
    Upgrade,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    run_with(std::env::args_os())
}

fn run_with<I, T>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);
    match cli.command {
        None => {
            println!("oakum");
            Ok(())
        }
        Some(Commands::Add(args)) => add::run(args),
        Some(Commands::Check(args)) => preconditions::run(&args),
        Some(Commands::TagDrift) => preconditions::run_tags_only(),
        Some(Commands::Generate(args)) => generate::run(&args),
        Some(Commands::PlanIntent(args)) => intent::run(&args),
        Some(Commands::Status(args)) => status::run(&args),
        Some(Commands::ReachableTags) => tags::run(),
        Some(Commands::Upgrade) => upgrade::run(),
    }
}

/// Shared CLI errors that should print without a Rust dump.
#[derive(Debug)]
pub(crate) struct CliError {
    message: String,
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}
