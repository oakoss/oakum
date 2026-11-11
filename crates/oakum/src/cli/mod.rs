mod add;
mod changelog;
mod check_report;
mod ci;
mod config;
mod consume;
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
mod lockfiles;
mod markdown;
mod migrate;
mod migrate_config;
mod migrate_gates;
mod migrate_output;
mod migrate_source_plan;
mod owned_files;
mod preconditions;
mod printable;
mod reasons;
mod release;
mod release_state;
mod render;
mod repository;
mod retired;
mod root_files;
mod stale;
mod status;
mod tag_shape;
mod tags;
mod template;
mod upgrade;
mod verdict;
mod version;
mod workflow;
mod workspace;
mod write_set;

use std::ffi::OsString;
use std::fmt;

use clap::{Parser, Subcommand};

/// Polyglot release tool: version math, changelogs, and graph-derived bumps.
#[derive(Debug, Parser)]
#[command(
    name = "oakum",
    version,
    about,
    long_about = None,
    after_help = "Typical flow: `oakum init`, `oakum workflow` saved under `.github/workflows/`, `oakum add` for each change, \
                  `oakum status` to see the plan, and `oakum check --strict` on pull requests. \
                  CI runs `oakum ci version-pr` and `oakum release`.\n\n\
                  Guides: https://github.com/oakoss/oakum/tree/main/docs/guide"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

macro_rules! merge_base {
    () => {
        "the merge-base with `origin/main`, else `main`, else `master`"
    };
}

macro_rules! since_tags {
    () => {
        "each package oakum tags starts at its highest reachable tag"
    };
}

/// `--from` on `generate` and `release`, which read commits since one base.
pub(super) const FROM_MERGE_BASE: &str = concat!(
    "Git ref to scan from (exclusive). Default: ",
    merge_base!(),
    "."
);

/// `--from` on the commands that read coverage as well as the plan.
pub(super) const FROM_PLAN: &str = concat!(
    "Git ref to scan from (exclusive). Default: ",
    merge_base!(),
    "; for intent read from conventional commits, ",
    since_tags!(),
    " instead."
);

/// `--from` on the commands that read only the plan, which bump files make
/// no use of a base for.
pub(super) const FROM_INTENT: &str = concat!(
    "Git ref to read conventional commits from (exclusive); unused with bump files. Default: ",
    since_tags!(),
    ", and every other package at ",
    merge_base!(),
    "."
);

/// Listed in the order a user meets them; clap shows declaration order.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Write oakum's three files and print the workflow to paste.
    Init(init::InitArgs),
    /// Print the workflow for this repository on stdout, writing nothing.
    Workflow(workflow::WorkflowArgs),
    /// Write one `.changeset/*.md` bump file.
    Add(add::AddArgs),
    /// Derive one `.changeset/*.md` from commits on this branch.
    Generate(generate::GenerateArgs),
    /// Print the versioned release state: the `summary` render by default, or JSON.
    Status(status::StatusArgs),
    /// Report drift and name the fix.
    Check(preconditions::CheckArgs),
    /// Apply the plan: bump versions, update lockfiles, write changelogs, consume bump files.
    Version(version::VersionArgs),
    /// Push tags and create GitHub releases for untagged manifest versions.
    Release(release::ReleaseArgs),
    /// GitHub writes for CI.
    Ci(ci::CiArgs),
    /// Transform another tool's bump files and config into oakum's, and print remaining steps.
    Migrate(migrate::MigrateArgs),
    /// Print plan bump-file inputs as JSON (hidden plumbing for tests).
    #[command(name = "plan-intent", hide = true)]
    PlanIntent(intent::PlanIntentArgs),
    /// Print foreign release-tool markers (hidden plumbing for tests).
    #[command(name = "detect-release-tools", hide = true)]
    DetectReleaseTools,
    /// Print tags reachable from HEAD as `commit\\ttag`.
    #[command(name = "reachable-tags", hide = true)]
    ReachableTags,
    /// Tag-only readiness path (no coverage).
    #[command(name = "tag-drift", hide = true)]
    TagDrift,
    /// Migrate `.changeset/_config.toml` and `_schema.json` to this binary, and refresh a README an earlier oakum wrote.
    Upgrade,
}

/// Says a line on stderr without panicking when the write is refused. A broken
/// pipe makes `eprintln!` panic, which replaces ADR-0034's exit code with 101
/// and loses the refusal — measured: a run owing exit 2 exited 101 and printed
/// nothing when its stderr reader had gone away. A *closed* stderr does not,
/// for the reason `git::env::say` records. Escaped here so no message has to:
/// [`printable`].
pub(crate) fn say_err(line: &str) {
    let _ = printable::say(std::io::stderr(), line);
}

/// The same for stdout. A report is not worth a panic: `println!` aborts on a
/// refused write, so `oakum check | head -1` exited 101 with nothing on either
/// stream, discarding an outcome the run had already established.
pub(crate) fn say_out(line: &str) {
    let _ = printable::say(std::io::stdout(), line);
}

/// A line that is the command's result (a path, a URL, a document) rather than
/// a report about it. `say_out` discards a refusal because a reader who went
/// away chose not to read; a result nobody received is a command that did not
/// finish. The caller maps the error with the side effect that already
/// happened, so exit 2 (unverified) and stderr name what exists.
/// A closed descriptor (`>&-`) is not a refusal: std reports `Ok` on EBADF
/// for stdout, so a caller who closed it gets exit 0, as for `ask`'s stderr.
pub(crate) fn deliver_out(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    writeln!(std::io::stdout(), "{line}")
}

/// [`deliver_out`] for a pre-formatted block, written as-is: it carries its own
/// newlines, and a body that ends without one must stay that way.
pub(crate) fn deliver_block(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    stdout.write_all(text.as_bytes())?;
    // Stdout is line-buffered: a body with no newline would sit until the
    // exit flush, whose refusal nobody reads.
    stdout.flush()
}

/// [`deliver_block`] for a report a person reads rather than a result a script
/// takes, so it is escaped as `say_out` escapes.
pub(crate) fn deliver_report(text: &str) -> std::io::Result<()> {
    deliver_block(&printable::for_terminal(text))
}

/// A prompt on stderr, no newline, flushed so it reaches the terminal before
/// the read that follows. A refused write propagates: a prompt that cannot be
/// shown cannot be answered. A *closed* stderr does not — std reports `Ok` on
/// EBADF, as `say_err` records — so the wizard then runs on an empty read.
pub(crate) fn ask(prompt: &str) -> std::io::Result<()> {
    printable::prompt(std::io::stderr(), prompt)
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
        Commands::Add(args) => add::run(args).map_err(CliError::from_boxed),
        Commands::Check(args) => preconditions::run(&args),
        Commands::TagDrift => preconditions::run_tags_only(),
        Commands::Generate(args) => generate::run(&args).map_err(CliError::from_boxed),
        Commands::Init(args) => init::run(&args).map_err(CliError::from_boxed),
        Commands::Workflow(args) => workflow::run(&args).map_err(CliError::from_boxed),
        Commands::Migrate(args) => migrate::run(&args).map_err(CliError::from_boxed),
        Commands::PlanIntent(args) => intent::run(&args).map_err(CliError::from_boxed),
        Commands::DetectReleaseTools => detect_tools::run(),
        Commands::Status(args) => status::run(&args).map_err(CliError::from_boxed),
        Commands::ReachableTags => tags::run(),
        Commands::Upgrade => upgrade::run().map_err(CliError::from_boxed),
        Commands::Version(args) => version::run(&args).map_err(CliError::from_boxed),
        Commands::Release(args) => release::run(&args),
        Commands::Ci(args) => ci::run(&args),
    }
}

pub(super) fn binary_version() -> Result<semver::Version, Box<dyn std::error::Error>> {
    env!("CARGO_PKG_VERSION")
        .parse::<semver::Version>()
        .map_err(|err| {
            Box::new(CliError::new(format!(
                "this binary reports a non-semver version: {err}"
            )))
            .into()
        })
}

/// The one implementation lives in the library, which builds the same sentence
/// for a render that failed on an undefined variable.
pub(super) use oakum::template::quoted;

/// Asserts the schema description for a template surface names every variable
/// that surface actually renders with. The lists are written by hand where a
/// reader configuring the key will find them; this is what stops them rotting
/// away from the contexts, which is how `tag-format` came to document nothing
/// about `package` at all.
#[cfg(test)]
pub(crate) fn schema_names_every_variable(surface: &str, context: impl serde::Serialize) {
    let schema = oakum::config::schema();
    let described = schema["properties"][surface]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("`{surface}` has a schema description"))
        .to_owned();
    let rendered = serde_json::to_value(context).expect("the context serializes");
    let keys = rendered
        .as_object()
        .unwrap_or_else(|| panic!("`{surface}` renders with a map context"));
    assert!(!keys.is_empty(), "`{surface}` renders with no variables");
    for key in keys.keys() {
        assert!(
            described.contains(&format!("`{key}`")),
            "`{surface}` receives `{key}`, which its schema description does not name: {described}"
        );
    }
}

/// What a failure is, once: the exit code and the stderr token are both read
/// off it.
///
/// Ordered by which one a run carrying several refusals should carry out:
/// a finding outranks a look that did not happen, so `Error` is declared first.
/// ADR-0034 exists so a caller can tell the two apart, and an ordering held
/// anywhere but on this type is a second place for them to disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Outcome {
    Error,
    Unverified,
}

impl Outcome {
    fn code(self) -> i32 {
        match self {
            Self::Unverified => 2,
            Self::Error => 1,
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Error => "error",
        }
    }
}

/// A run that did not succeed: its [`Outcome`] and the message stderr carries.
///
/// `Clone` so a failure can be cached and handed to more than one caller.
#[derive(Clone, Debug)]
pub(crate) struct CliError {
    class: Outcome,
    message: String,
    /// A git child that gave no answer, so a caller that turns an answered
    /// look into a plain refusal can still keep this one unverified.
    unanswered: bool,
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            class: Outcome::Error,
            message: message.into(),
            unanswered: false,
        }
    }

    pub(crate) fn unverified(detail: impl Into<String>) -> Self {
        Self {
            class: Outcome::Unverified,
            message: detail.into(),
            unanswered: false,
        }
    }

    pub(crate) fn unanswered(detail: impl Into<String>) -> Self {
        Self {
            unanswered: true,
            ..Self::unverified(detail)
        }
    }

    pub(crate) const fn is_unanswered(&self) -> bool {
        self.unanswered
    }

    #[cfg(test)]
    pub(crate) const fn is_unverified(&self) -> bool {
        matches!(self.class, Outcome::Unverified)
    }

    /// A new detail under this verdict's class. A wrapper that adds what
    /// landed must not reclassify why the run stopped.
    pub(crate) fn recast(&self, detail: impl Into<String>) -> Self {
        Self {
            message: detail.into(),
            ..self.clone()
        }
    }

    /// A result nobody received. `what` names the side effect that already
    /// happened and the thing that did not arrive ("wrote x, but its path"),
    /// so the shell learns both from one line.
    pub(crate) fn undelivered(what: impl std::fmt::Display, err: &std::io::Error) -> Self {
        Self::unverified(format!("{what} could not be delivered to stdout: {err}"))
    }

    pub(crate) fn tag_drift(count: usize) -> Self {
        Self::new(format!("{count} package(s) bumped without a tag"))
    }

    pub(crate) fn uncovered(count: usize) -> Self {
        Self::new(format!(
            "{count} package(s) changed with no covering intent"
        ))
    }

    pub(crate) fn from_boxed(err: Box<dyn std::error::Error>) -> Self {
        match err.downcast::<Self>() {
            Ok(typed) => *typed,
            Err(other) => Self::new(other.to_string()),
        }
    }

    /// Both channels a caller reads, the number and the word, come from here.
    pub(crate) const fn class(&self) -> Outcome {
        self.class
    }

    /// The three outcomes `AGENTS.md` requires, as the only channel a caller
    /// that is not reading stderr can see: `0` ok, `2` unverified, `1` error.
    ///
    /// Every exit stays non-zero, so a shell testing success is unaffected;
    /// what changes is that "we could not look" no longer arrives wearing the
    /// same number as "this failed".
    pub(crate) fn exit_code(&self) -> i32 {
        self.class().code()
    }

    /// The token stderr leads with. Printing a fixed `error: ` put two outcome
    /// tokens on one line — `error: unverified: …` — saying different things
    /// about one result.
    pub(crate) fn outcome(&self) -> &'static str {
        self.class().token()
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
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<Box<dyn std::error::Error>> for CliError {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Self::from_boxed(err)
    }
}

impl From<github::Error> for CliError {
    fn from(err: github::Error) -> Self {
        match err {
            github::Error::Unverified { detail } => Self::unverified(detail),
            github::Error::Forbidden { path } => Self::new(format!("GitHub {path} returned 403")),
            unauthorized @ github::Error::Unauthorized { .. } => {
                Self::new(unauthorized.to_string())
            }
            github::Error::Other(message) => Self::new(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CliError, Outcome};

    #[test]
    fn detail_strips_a_leading_unverified_token() {
        let nested = CliError::unverified("unverified: no remotes");
        assert_eq!(nested.detail(), "no remotes");
        let plain = CliError::new("boom");
        assert_eq!(plain.detail(), "boom");
    }

    /// A wrapper that adds context must not drop the mark a caller reads to
    /// keep a no-answer unverified.
    #[test]
    fn recast_keeps_an_unanswered_mark() {
        let wrapped = CliError::unanswered("unverified: git log failed").recast("context");
        assert!(wrapped.is_unanswered());
        assert_eq!(wrapped.exit_code(), 2);
        assert!(!CliError::unverified("looked")
            .recast("context")
            .is_unanswered());
    }

    /// Each constructor beside the code it must answer.
    fn exit_codes() -> [(CliError, i32); 6] {
        [
            (CliError::unverified("unverified: no remotes"), 2),
            (CliError::unanswered("unverified: git log failed"), 2),
            (CliError::tag_drift(1), 1),
            (CliError::uncovered(2), 1),
            (
                CliError::from(super::github::Error::Forbidden {
                    path: String::from("/comments"),
                }),
                1,
            ),
            (CliError::new("boom"), 1),
        ]
    }

    #[test]
    fn each_outcome_has_one_code_and_one_token() {
        assert_eq!(
            (Outcome::Unverified.code(), Outcome::Unverified.token()),
            (2, "unverified")
        );
        assert_eq!(
            (Outcome::Error.code(), Outcome::Error.token()),
            (1, "error")
        );
    }

    #[test]
    fn only_unverified_exits_two() {
        for (err, expected) in exit_codes() {
            assert_eq!(err.exit_code(), expected, "{err}");
            assert_eq!(expected == 2, err.is_unverified(), "{err}");
        }
    }

    /// The number and the word a caller reads come off one outcome, so a
    /// failure cannot exit 2 while stderr calls it an error.
    #[test]
    fn the_printed_token_agrees_with_the_exit_code() {
        for (err, expected) in exit_codes() {
            assert_eq!(
                err.outcome() == "unverified",
                expected == 2,
                "`{}` beside exit {expected}: {err}",
                err.outcome()
            );
        }
    }

    /// main prints `<outcome>: <detail>`; a detail that still carries its own
    /// `unverified: ` would put the token on the line twice, which is the
    /// defect this pairing replaced.
    #[test]
    fn the_printed_line_carries_one_outcome_token() {
        let line = {
            let err = CliError::unverified("unverified: no remotes");
            format!("{}: {}", err.outcome(), err.detail())
        };
        assert_eq!(line, "unverified: no remotes");
        assert_eq!(line.matches("unverified").count(), 1, "{line}");
    }

    #[test]
    fn from_boxed_keeps_unverified() {
        let boxed: Box<dyn std::error::Error> =
            Box::new(CliError::unverified("unverified: no remotes"));
        assert!(CliError::from_boxed(boxed).is_unverified());
    }

    #[test]
    fn from_boxed_does_not_treat_io_as_unverified() {
        let boxed: Box<dyn std::error::Error> = Box::new(std::io::Error::other("boom"));
        let err = CliError::from_boxed(boxed);
        assert!(!err.is_unverified());
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn github_unverified_maps_to_cli_unverified() {
        let err = CliError::from(super::github::Error::Unverified {
            detail: String::from("unverified: GitHub /repos/oakoss/oakum returned 502"),
        });
        assert!(err.is_unverified());
    }

    #[test]
    fn github_forbidden_is_an_error_naming_the_path() {
        let err = CliError::from(super::github::Error::Forbidden {
            path: String::from("/repos/oakoss/oakum/issues/4/comments"),
        });
        assert!(!err.is_unverified());
        assert_eq!(
            err.to_string(),
            "GitHub /repos/oakoss/oakum/issues/4/comments returned 403"
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
