mod add;
mod changelog;
mod ci;
mod config;
mod coverage;
mod detect_tools;
mod fs;
mod generate;
mod git;
mod github;
mod handoff;
mod inherited;
mod init;
mod install_pin;
mod intent;
mod markdown;
mod migrate;
mod migrate_config;
mod migrate_output;
mod migrate_source_plan;
mod owned_files;
mod preconditions;
mod release;
mod repository;
mod status;
mod tag_shape;
mod tags;
mod template;
mod upgrade;
mod version;
mod write_set;

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
    /// Write oakum's three files and print the workflow to paste.
    Init(init::InitArgs),
    /// Transform another tool's bump files and config into oakum's, and print remaining steps.
    Migrate(migrate::MigrateArgs),
    /// Push tags and create GitHub releases for untagged manifest versions.
    Release(release::ReleaseArgs),
    /// Write planned package versions, inherited pins, lockfile rows, declared extra-files, changelogs, and — when bumping the Cargo member named `oakum` and config already declares `tool-version` — updates that pin.
    Version(version::VersionArgs),
    /// GitHub writes for CI.
    Ci(ci::CiArgs),
    /// Print plan bump-file inputs as JSON (hidden plumbing for tests).
    #[command(name = "plan-intent", hide = true)]
    PlanIntent(intent::PlanIntentArgs),
    /// Print foreign release-tool markers (hidden plumbing for tests).
    #[command(name = "detect-release-tools", hide = true)]
    DetectReleaseTools,
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
        Some(Commands::Init(args)) => init::run(&args).map_err(CliError::from_boxed),
        Some(Commands::Migrate(args)) => migrate::run(&args).map_err(CliError::from_boxed),
        Some(Commands::PlanIntent(args)) => intent::run(&args).map_err(CliError::from_boxed),
        Some(Commands::DetectReleaseTools) => detect_tools::run(),
        Some(Commands::Status(args)) => status::run(&args).map_err(CliError::from_boxed),
        Some(Commands::ReachableTags) => tags::run(),
        Some(Commands::Upgrade) => upgrade::run().map_err(CliError::from_boxed),
        Some(Commands::Version(args)) => version::run(&args).map_err(CliError::from_boxed),
        Some(Commands::Release(args)) => release::run(&args),
        Some(Commands::Ci(args)) => ci::run(&args),
    }
}

/// Names in a sentence: backticked, comma-joined. Five hand-written copies of
/// this existed across `tag_shape` and `migrate_output`, one of them nine lines
/// from the original.
pub(super) fn quoted(names: impl IntoIterator<Item = impl fmt::Display>) -> String {
    names
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Distinct variants so check outcomes stay distinguishable, and
/// [`Self::exit_code`] carries that distinction out to the shell.
///
/// `Clone` so a failure can be cached and handed to more than one caller.
#[derive(Clone, Debug)]
pub(crate) enum CliError {
    Unverified { detail: String },
    TagDrift { count: usize },
    Uncovered { count: usize },
    Forbidden { path: String },
    MissingActionsToken,
    MissingPullNumber,
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

    /// The three outcomes `AGENTS.md` requires, as the only channel a caller
    /// that is not reading stderr can see: `0` ok, `2` unverified, `1` error.
    ///
    /// Every exit stays non-zero, so a shell testing success is unaffected;
    /// what changes is that "we could not look" no longer arrives wearing the
    /// same number as "this failed".
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Unverified { .. } => 2,
            Self::TagDrift { .. }
            | Self::Uncovered { .. }
            | Self::Forbidden { .. }
            | Self::MissingActionsToken
            | Self::MissingPullNumber
            | Self::Other(_) => 1,
        }
    }

    /// The message without a leading `unverified: ` outcome token.
    pub(crate) fn detail(&self) -> String {
        let message = self.to_string();
        message
            .strip_prefix("unverified: ")
            .map(str::to_owned)
            .unwrap_or(message)
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
            Self::Forbidden { path } => write!(f, "GitHub {path} returned 403"),
            Self::MissingActionsToken => {
                write!(
                    f,
                    "`oakum ci pr-status` needs GITHUB_TOKEN to post a comment"
                )
            }
            Self::MissingPullNumber => write!(
                f,
                "`oakum ci pr-status` needs a pull request number (GITHUB_EVENT_PATH or GITHUB_REF)"
            ),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CliError {}

impl From<github::Error> for CliError {
    fn from(err: github::Error) -> Self {
        match err {
            github::Error::Unverified { detail } => Self::Unverified { detail },
            github::Error::Forbidden { path } => Self::Forbidden { path },
            unauthorized @ github::Error::Unauthorized { .. } => {
                Self::Other(unauthorized.to_string())
            }
            github::Error::Other(message) => Self::Other(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CliError;

    #[test]
    fn detail_strips_a_leading_unverified_token() {
        let nested = CliError::unverified("unverified: no remotes");
        assert_eq!(nested.detail(), "no remotes");
        let plain = CliError::new("boom");
        assert_eq!(plain.detail(), "boom");
    }

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

    /// How many variants `CliError` declares. [`exit_codes`] states one row for
    /// each, so a variant cannot reach the table without stating its code, and
    /// `every_variant_states_an_exit_code` ties the count back to the enum.
    const EXIT_CODED: usize = 7;

    /// Every variant beside the code it must answer. An exhaustive `match` in
    /// [`CliError::exit_code`] forces a new variant to state *a* code; this
    /// table is what checks *which*.
    fn exit_codes() -> [(CliError, i32); EXIT_CODED] {
        [
            (CliError::unverified("unverified: no remotes"), 2),
            (CliError::tag_drift(1), 1),
            (CliError::uncovered(2), 1),
            (
                CliError::Forbidden {
                    path: String::from("/comments"),
                },
                1,
            ),
            (CliError::MissingActionsToken, 1),
            (CliError::MissingPullNumber, 1),
            (CliError::new("boom"), 1),
        ]
    }

    #[test]
    fn only_unverified_exits_two() {
        for (err, expected) in exit_codes() {
            assert_eq!(err.exit_code(), expected, "{err}");
            // The "only" in this test's name: a second variant claiming 2 in
            // both the arm and the table would otherwise pass, the table being
            // a second declaration by the same hand.
            assert_eq!(
                expected == 2,
                matches!(err, CliError::Unverified { .. }),
                "only `Unverified` exits 2: {err}"
            );
        }
    }

    /// The exhaustive `match` catches a variant that states no code; nothing
    /// catches one dropped into the wrong arm, which is the mistake that would
    /// ship silently. Measured: a variant added to the `=> 2` arm left the whole
    /// bin suite green.
    #[test]
    fn every_variant_states_an_exit_code() {
        let source = include_str!("mod.rs");
        // Two pieces, so the needle does not match this line as well.
        let opener = concat!("pub(crate) ", "enum CliError {");
        let body = source
            .split_once(opener)
            .expect("the CliError enum")
            .1
            .split_once("\n}\n")
            .expect("the end of the CliError enum")
            .0;
        let declared = body
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                line.len() - trimmed.len() == 4
                    && trimmed.starts_with(|c: char| c.is_ascii_uppercase())
            })
            .count();
        assert_eq!(
            declared,
            exit_codes().len(),
            "`CliError` declares {declared} variants and the exit-code table lists {}",
            exit_codes().len()
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
    fn github_unverified_maps_to_cli_unverified() {
        let err = CliError::from(super::github::Error::Unverified {
            detail: String::from("unverified: GitHub /graphql returned 502"),
        });
        assert!(matches!(err, CliError::Unverified { .. }));
    }

    #[test]
    fn github_forbidden_maps_to_cli_forbidden() {
        let err = CliError::from(super::github::Error::Forbidden {
            path: String::from("/repos/oakoss/oakum/issues/4/comments"),
        });
        assert!(matches!(err, CliError::Forbidden { .. }));
        assert_eq!(
            err.to_string(),
            "GitHub /repos/oakoss/oakum/issues/4/comments returned 403"
        );
    }

    #[test]
    fn missing_comment_preconditions_are_distinct_variants() {
        assert!(matches!(
            CliError::MissingActionsToken,
            CliError::MissingActionsToken
        ));
        assert!(matches!(
            CliError::MissingPullNumber,
            CliError::MissingPullNumber
        ));
        assert_ne!(
            std::mem::discriminant(&CliError::MissingActionsToken),
            std::mem::discriminant(&CliError::MissingPullNumber)
        );
        assert_eq!(
            CliError::MissingActionsToken.to_string(),
            "`oakum ci pr-status` needs GITHUB_TOKEN to post a comment"
        );
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
