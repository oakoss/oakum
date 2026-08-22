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

pub fn run() -> Result<(), CliError> {
    run_with(std::env::args_os())
}

fn run_with<I, T>(args: I) -> Result<(), CliError>
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
        Some(Commands::Add(args)) => add::run(args).map_err(CliError::from_boxed),
        Some(Commands::Check(args)) => preconditions::run(&args),
        Some(Commands::TagDrift) => preconditions::run_tags_only(),
        Some(Commands::Generate(args)) => generate::run(&args).map_err(CliError::from_boxed),
        Some(Commands::PlanIntent(args)) => intent::run(&args).map_err(CliError::from_boxed),
        Some(Commands::Status(args)) => status::run(&args).map_err(CliError::from_boxed),
        Some(Commands::ReachableTags) => tags::run(),
        Some(Commands::Upgrade) => upgrade::run().map_err(CliError::from_boxed),
    }
}

/// Distinct variants so check outcomes stay distinguishable. All variants print and exit 1.
#[derive(Debug)]
pub(crate) enum CliError {
    Unverified { detail: String },
    TagDrift { count: usize },
    Uncovered { count: usize },
    Other(String),
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    pub(crate) fn unverified(detail: impl Into<String>) -> Self {
        Self::Unverified {
            detail: detail.into(),
        }
    }

    pub(crate) fn tag_drift(count: usize) -> Self {
        Self::TagDrift { count }
    }

    pub(crate) fn uncovered(count: usize) -> Self {
        Self::Uncovered { count }
    }

    pub(crate) fn from_boxed(err: Box<dyn std::error::Error>) -> Self {
        match err.downcast::<Self>() {
            Ok(typed) => *typed,
            Err(other) => Self::Other(other.to_string()),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unverified { detail } => f.write_str(detail),
            Self::TagDrift { count } => {
                write!(f, "{count} package(s) bumped without a tag")
            }
            Self::Uncovered { count } => {
                write!(f, "{count} package(s) changed with no covering intent")
            }
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::CliError;

    #[test]
    fn check_outcomes_are_distinct_variants() {
        let unverified = CliError::unverified("unverified: no remotes");
        let drift = CliError::tag_drift(1);
        let uncovered = CliError::uncovered(2);
        assert!(matches!(unverified, CliError::Unverified { .. }));
        assert!(matches!(drift, CliError::TagDrift { count: 1 }));
        assert!(matches!(uncovered, CliError::Uncovered { count: 2 }));
        assert_ne!(
            std::mem::discriminant(&unverified),
            std::mem::discriminant(&drift)
        );
    }

    #[test]
    fn from_boxed_keeps_unverified() {
        let boxed: Box<dyn std::error::Error> =
            Box::new(CliError::unverified("unverified: no remotes"));
        assert!(matches!(
            CliError::from_boxed(boxed),
            CliError::Unverified { .. }
        ));
    }

    #[test]
    fn from_boxed_does_not_treat_io_as_unverified() {
        let boxed: Box<dyn std::error::Error> = Box::new(std::io::Error::other("boom"));
        assert!(matches!(CliError::from_boxed(boxed), CliError::Other(_)));
    }

    #[test]
    fn tag_drift_and_uncovered_display_match_the_check_summaries() {
        assert_eq!(
            CliError::tag_drift(1).to_string(),
            "1 package(s) bumped without a tag"
        );
        assert_eq!(
            CliError::uncovered(2).to_string(),
            "2 package(s) changed with no covering intent"
        );
    }
}
