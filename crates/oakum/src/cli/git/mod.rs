//! Every git child oakum spawns.
//!
//! The environment, the outcome vocabulary, and the stdout decoding live here
//! rather than at each call site. Written per site, all three drift: prompt
//! suppression reached three of sixteen children before okm-6mz, and whether a
//! failure was `unverified` or a plain error was decided by which module the
//! caller happened to be in.
//!
//! [`Op`] is closed so the set of git operations in the crate is a list one can
//! read, and so each operation states its own outcome class once.

mod env;
#[cfg(test)]
mod fake;

use std::path::{Path, PathBuf};
use std::process::Command;

use super::CliError;

/// Where a failed child lands in the three-outcome vocabulary (AGENTS.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// Informs a verification, so a failure to look is `unverified` — never
    /// silently "nothing to report".
    Verification,
    /// Does work, so a failure is a plain error.
    Action,
}

/// How an accessor decides whether stdout carries an answer. The two disagree,
/// and the guard has to ask the one doing the reading: `text` and
/// `optional_text` trim, so a lone `\x0B` reaches the caller as `""`, while
/// `paths` keeps every non-empty NUL record byte-for-byte, because a file named
/// `" "` is a filename and not silence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reads {
    Text,
    Paths,
}

/// What a successful child writes to stdout, which is what decides the meaning
/// of one that wrote nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    /// Always. Silence from a child that exited 0 means it never answered.
    Always,
    /// Sometimes: no tags, no remotes, nothing changed, a clean worktree. The
    /// emptiness is a real answer, but not one a diagnostic leaves standing.
    Sometimes,
    /// Never — the operation reports through its exit code, and a successful
    /// `git push` writes its whole report to stderr. Silence proves nothing
    /// either way, so no rule can be drawn from it.
    Never,
}

/// Everything the runner needs about an operation, gathered so each `spec` arm
/// states every axis at once.
struct Spec {
    outcome: Outcome,
    reaches_remote: bool,
    answer: Answer,
    /// Free-form commit text, which git does not promise is UTF-8: a commit
    /// object written verbatim by another tool carries raw bytes that `git log`
    /// passes straight through. Replacing one with U+FFFD beats refusing to read
    /// the message at all.
    lossy: bool,
}

impl Spec {
    const LOOK: Self = Self {
        outcome: Outcome::Verification,
        reaches_remote: false,
        answer: Answer::Sometimes,
        lossy: false,
    };
    const ANSWERING_LOOK: Self = Self {
        answer: Answer::Always,
        ..Self::LOOK
    };
    const REMOTE_LOOK: Self = Self {
        reaches_remote: true,
        ..Self::LOOK
    };
    const ACT: Self = Self {
        outcome: Outcome::Action,
        reaches_remote: false,
        answer: Answer::Sometimes,
        lossy: false,
    };
    const ANSWERING_ACT: Self = Self {
        answer: Answer::Always,
        ..Self::ACT
    };
    const LOSSY_ACT: Self = Self {
        lossy: true,
        ..Self::ACT
    };
    /// Work that answers through its exit code alone.
    const PERFORM: Self = Self {
        answer: Answer::Never,
        ..Self::ACT
    };
    const REMOTE_PERFORM: Self = Self {
        reaches_remote: true,
        ..Self::PERFORM
    };
}

/// Every git operation oakum performs.
#[derive(Clone, Copy, Debug)]
pub(super) enum Op<'a> {
    /// Tags reachable from HEAD with their peeled identity (ADR-0014).
    ReachableTags,
    IsShallow,
    /// Remotes configured with `tagOpt = --no-tags`.
    TagOptRemotes,
    RemoteNames,
    AdvertisedTags {
        remote: &'a str,
    },
    /// Paths changed since `from`, NUL-separated.
    ChangedPaths {
        from: &'a str,
    },
    Head,
    RemoteUrl {
        remote: &'a str,
    },
    MergeBase {
        tip: &'a str,
    },
    /// `hash NUL subject NUL body NUL` per commit, oldest first.
    Commits {
        from: &'a str,
    },
    /// Paths in one commit, NUL-separated.
    CommitPaths {
        hash: &'a str,
    },
    CommitParents {
        hash: &'a str,
    },
    /// The commit a local tag points at, peeled.
    LocalTagCommit {
        tag: &'a str,
    },
    WorktreeStatus,
    HeadMessage,
    RefExists {
        reference: &'a str,
    },
    ValidRefName {
        reference: &'a str,
    },
    AnnotatedTag {
        name: &'a str,
        commit: &'a str,
    },
    PushTag {
        remote: &'a str,
        tag: &'a str,
    },
}

impl Op<'static> {
    /// One of each variant, so the tests can state an expected axis per
    /// operation and a new variant fails the count before it can inherit a
    /// neighbour's `Spec`.
    #[cfg(test)]
    const EVERY: [Self; 19] = [
        Self::ReachableTags,
        Self::IsShallow,
        Self::TagOptRemotes,
        Self::RemoteNames,
        Self::AdvertisedTags { remote: "origin" },
        Self::ChangedPaths { from: "v1.0.0" },
        Self::Head,
        Self::RemoteUrl { remote: "origin" },
        Self::MergeBase { tip: "main" },
        Self::Commits { from: "v1.0.0" },
        Self::CommitPaths { hash: "cafebabe" },
        Self::CommitParents { hash: "cafebabe" },
        Self::LocalTagCommit { tag: "v1.0.0" },
        Self::WorktreeStatus,
        Self::HeadMessage,
        Self::RefExists {
            reference: "refs/tags/v1.0.0",
        },
        Self::ValidRefName {
            reference: "v1.0.0",
        },
        Self::AnnotatedTag {
            name: "v1.0.0",
            commit: "HEAD",
        },
        Self::PushTag {
            remote: "origin",
            tag: "v1.0.0",
        },
    ];
}

impl Op<'_> {
    fn argv(&self) -> Vec<String> {
        let owned = |parts: &[&str]| parts.iter().map(|part| (*part).to_owned()).collect();
        match self {
            Self::ReachableTags => owned(&[
                "for-each-ref",
                "--merged=HEAD",
                "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)",
                "refs/tags",
            ]),
            Self::IsShallow => owned(&["rev-parse", "--is-shallow-repository"]),
            Self::TagOptRemotes => owned(&["config", "--get-regexp", r"^remote\..*\.tagopt$"]),
            Self::RemoteNames => owned(&["remote"]),
            Self::AdvertisedTags { remote } => owned(&["ls-remote", "--tags", "--", remote]),
            Self::ChangedPaths { from } => vec![
                String::from("diff"),
                String::from("-z"),
                String::from("--name-only"),
                format!("{from}...HEAD"),
            ],
            Self::Head => owned(&["rev-parse", "HEAD"]),
            Self::RemoteUrl { remote } => owned(&["remote", "get-url", "--", remote]),
            Self::MergeBase { tip } => owned(&["merge-base", tip, "HEAD"]),
            Self::Commits { from } => vec![
                String::from("log"),
                format!("{from}..HEAD"),
                String::from("--reverse"),
                String::from("--format=%H%x00%s%x00%b%x00"),
            ],
            Self::CommitPaths { hash } => owned(&[
                "diff-tree",
                "--no-commit-id",
                "--name-only",
                "-z",
                "-r",
                "--root",
                hash,
            ]),
            Self::CommitParents { hash } => owned(&["rev-list", "--parents", "-n", "1", hash]),
            Self::LocalTagCommit { tag } => vec![
                String::from("rev-parse"),
                String::from("--verify"),
                String::from("--quiet"),
                format!("refs/tags/{tag}^{{}}"),
            ],
            // A `core.fsmonitor` hook that cannot be executed makes git fall
            // back, answer correctly, and write `fatal: cannot exec ...` to
            // stderr while exiting 0 — indistinguishable from a status that
            // never ran. Overriding the setting removes the diagnostic and
            // leaves the answer byte-identical (measured both clean and dirty).
            Self::WorktreeStatus => owned(&[
                "-c",
                "core.fsmonitor=false",
                "status",
                "--porcelain",
                "--untracked-files=all",
            ]),
            Self::HeadMessage => owned(&["log", "-1", "--format=%B"]),
            // Without `--quiet`, an absent ref exits 128 with a diagnostic —
            // the same shape as an unreadable repository.
            Self::RefExists { reference } => {
                owned(&["rev-parse", "--verify", "--quiet", reference])
            }
            Self::ValidRefName { reference } => vec![
                String::from("check-ref-format"),
                format!("refs/tags/{reference}"),
            ],
            Self::AnnotatedTag { name, commit } => {
                vec![
                    String::from("tag"),
                    String::from("-m"),
                    (*name).to_owned(),
                    String::from("--"),
                    (*name).to_owned(),
                    (*commit).to_owned(),
                ]
            }
            Self::PushTag { remote, tag } => vec![
                String::from("push"),
                String::from("--"),
                (*remote).to_owned(),
                format!("refs/tags/{tag}"),
            ],
        }
    }

    /// The class of one operation. Three separate classifiers each defaulted
    /// silently, and the defaults were the dangerous answers: a remote
    /// operation that reads as local loses `BatchMode` and hangs, and a read
    /// that reads as an action turns "we could not look" into a plain error.
    // One arm per variant, never a `|` group: an operation appended to a group
    // compiles while stating nothing and inherits whatever its neighbour
    // happened to be, which is what this whole function exists to prevent.
    // `match_same_arms` asks for exactly that grouping.
    #[expect(
        clippy::match_same_arms,
        reason = "grouping variants is exactly the hazard"
    )]
    const fn spec(&self) -> Spec {
        match self {
            Self::ReachableTags => Spec::LOOK,
            Self::IsShallow => Spec::ANSWERING_LOOK,
            Self::TagOptRemotes => Spec::ANSWERING_LOOK,
            Self::RemoteNames => Spec::LOOK,
            Self::AdvertisedTags { .. } => Spec::REMOTE_LOOK,
            Self::ChangedPaths { .. } => Spec::LOOK,
            Self::Head => Spec::ANSWERING_ACT,
            Self::RemoteUrl { .. } => Spec::ANSWERING_ACT,
            Self::MergeBase { .. } => Spec::ANSWERING_ACT,
            Self::Commits { .. } => Spec::LOSSY_ACT,
            Self::CommitPaths { .. } => Spec::ACT,
            Self::CommitParents { .. } => Spec::ANSWERING_ACT,
            Self::LocalTagCommit { .. } => Spec::ANSWERING_ACT,
            Self::WorktreeStatus => Spec::ACT,
            Self::HeadMessage => Spec::LOSSY_ACT,
            Self::RefExists { .. } => Spec::ANSWERING_ACT,
            Self::ValidRefName { .. } => Spec::PERFORM,
            Self::AnnotatedTag { .. } => Spec::PERFORM,
            Self::PushTag { .. } => Spec::REMOTE_PERFORM,
        }
    }

    /// The subcommand, for diagnostics. Paired with [`Self::operand`] rather than
    /// rendering the whole argv, which would repeat the flags in every message.
    const fn name(&self) -> &'static str {
        match self {
            Self::ReachableTags => "for-each-ref --merged HEAD",
            Self::IsShallow => "rev-parse --is-shallow-repository",
            Self::TagOptRemotes => "config --get-regexp tagopt",
            Self::RemoteNames => "remote",
            Self::AdvertisedTags { .. } => "ls-remote --tags",
            Self::ChangedPaths { .. } => "diff --name-only",
            Self::Head => "rev-parse HEAD",
            Self::RemoteUrl { .. } => "remote get-url",
            Self::MergeBase { .. } => "merge-base",
            Self::Commits { .. } => "log",
            Self::CommitPaths { .. } => "diff-tree",
            Self::CommitParents { .. } => "rev-list --parents",
            Self::LocalTagCommit { .. } => "rev-parse --verify refs/tags",
            Self::WorktreeStatus => "status --porcelain",
            Self::HeadMessage => "log -1",
            Self::RefExists { .. } => "rev-parse --verify",
            Self::ValidRefName { .. } => "check-ref-format",
            Self::AnnotatedTag { .. } => "tag",
            Self::PushTag { .. } => "push",
        }
    }

    /// What the operation was pointed at, so a failure names which remote or ref
    /// it was. Every value here is oakum's own — a remote name, a ref, a range —
    /// not text git produced.
    fn operand(&self) -> Option<String> {
        let owned = |value: &str| Some(value.to_owned());
        match self {
            Self::AdvertisedTags { remote } | Self::RemoteUrl { remote } => owned(remote),
            Self::ChangedPaths { from } => Some(format!("{from}...HEAD")),
            Self::Commits { from } => Some(format!("{from}..HEAD")),
            Self::MergeBase { tip } => owned(tip),
            Self::CommitPaths { hash } | Self::CommitParents { hash } => owned(hash),
            Self::LocalTagCommit { tag } => owned(tag),
            Self::RefExists { reference } | Self::ValidRefName { reference } => owned(reference),
            Self::AnnotatedTag { name, .. } => owned(name),
            Self::PushTag { remote, tag } => Some(format!("{remote} {tag}")),
            Self::ReachableTags
            | Self::IsShallow
            | Self::TagOptRemotes
            | Self::RemoteNames
            | Self::Head
            | Self::WorktreeStatus
            | Self::HeadMessage => None,
        }
    }
}

/// The trace2 channels, whose destination git config can set even when the
/// environment does not.
const TRACE2: [&str; 3] = ["GIT_TRACE2", "GIT_TRACE2_EVENT", "GIT_TRACE2_PERF"];

/// Whether a trace setting sends its output somewhere other than our stderr.
/// Measured on git 2.55: `1`, `2`, and `true` all print to stderr, an
/// unrecognised value warns and then prints to stderr, and only an absolute
/// path is written to a file. Anything that is not a path is dropped, so a
/// caller tracing to a file — `tests/reachable_tags.rs` counts children that
/// way — keeps it.
fn traces_to_a_file(value: &std::ffi::OsStr) -> bool {
    Path::new(value).is_absolute()
}

/// Drops inherited trace settings that would land on our stderr, where every
/// rule above would read them as a diagnostic: with `GIT_TRACE=1` exported a
/// healthy repository reports `unverified`, and `GIT_TRACE_PACKET=1` writes
/// 2704 bytes during an `ls-remote` that legitimately found no tags.
///
/// Matched by prefix rather than against a list. Git ships more than fifteen of
/// these and adds more; a list is a thing to be caught out by, one variable at
/// a time.
pub(super) fn untrace(command: &mut Command) {
    untrace_from(command, std::env::vars_os());
}

/// Split from [`untrace`] so a test can state an environment instead of
/// mutating the process's own: `cargo test` runs tests as threads of one
/// process, and a test that sets `GIT_TRACE` reaches every git child any other
/// test spawns concurrently.
fn untrace_from(
    command: &mut Command,
    env: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) {
    let mut keeps_a_file = [false; TRACE2.len()];
    for (name, value) in env {
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("GIT_TRACE") {
            continue;
        }
        let to_a_file = traces_to_a_file(&value);
        if let Some(at) = TRACE2.iter().position(|trace2| *trace2 == name) {
            keeps_a_file[at] = to_a_file;
        }
        if !to_a_file {
            command.env_remove(name);
        }
    }
    // Trace2 also takes its destination from git config, which no edit to the
    // environment reaches and which `-c` is too late to change: trace2
    // initialises before the option is parsed. Setting the variable off does
    // reach it. Measured with `trace2.normalTarget=2` in global config: a
    // `status` writes 585 bytes to stderr, removing the variable leaves it at
    // 585, `-c trace2.normalTarget=0` raises it to 610 by tracing the option
    // itself, and setting the variable to 0 takes it to nothing.
    //
    // Read from `env` rather than the process, so this stays a function of its
    // argument: consulting both let an exported `GIT_TRACE2_EVENT` decide the
    // answer for an environment that never mentioned it.
    for (at, name) in TRACE2.iter().enumerate() {
        if !keeps_a_file[at] {
            command.env(name, "0");
        }
    }
}

/// `None` when a record is not UTF-8. Kept separate from the run so the decoding
/// is testable without a repository.
fn split_nul_paths(stdout: &[u8]) -> Option<Vec<String>> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8(record.to_vec()).ok())
        .collect()
}

/// What a git child said.
///
/// `std::process::Output` would do, except that `ExitStatus` has no portable
/// constructor: `ExitStatusExt::from_raw` is a per-target extension trait, so a
/// fake built on it needs a `cfg` branch per platform — the portability problem
/// [`fake`] exists to escape. Every rule below reads the exit code, so carrying
/// our own leaves them exercisable without a process.
#[derive(Debug)]
pub(super) struct Reply {
    /// `None` when a signal killed the child before it could exit.
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Reply {
    fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// Whether stdout carries anything the calling accessor would keep, judged
    /// exactly as that accessor judges it. Any disagreement is a hole in one
    /// direction or the other, and both have been live here.
    fn spoke(&self, reads: Reads) -> bool {
        match reads {
            Reads::Text => !String::from_utf8_lossy(&self.stdout).trim().is_empty(),
            Reads::Paths => self
                .stdout
                .split(|byte| *byte == 0)
                .any(|record| !record.is_empty()),
        }
    }

    /// Git reports "absent" or "no" as exit 1 with both streams empty. A wrapper
    /// that exits 1 with a diagnostic did not look, which is not the same thing.
    fn said_no(&self) -> bool {
        self.code == Some(1) && self.stdout.is_empty() && self.stderr.is_empty()
    }

    /// Git's own words, when it wrote any.
    fn diagnostic(&self) -> Option<String> {
        let stderr = String::from_utf8_lossy(&self.stderr);
        let stderr = stderr.trim();
        (!stderr.is_empty()).then(|| stderr.to_owned())
    }

    /// Git's own words when it wrote any; otherwise the status, so a silent
    /// failure and a child killed by a signal do not render identically.
    fn detail(&self) -> String {
        if let Some(said) = self.diagnostic() {
            return said;
        }
        match self.code {
            Some(code) => format!("exit {code} with no diagnostic"),
            None => String::from("terminated by a signal"),
        }
    }
}

impl From<std::process::Output> for Reply {
    fn from(output: std::process::Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

/// The answers a test scripts. Named for what git did, not for the exit code,
/// so a script reads as the situation it stands for.
#[cfg(test)]
impl Reply {
    /// Exit 0, having written this.
    pub(super) fn said(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    /// Exit 1 with both streams empty: how git says "no" or "absent".
    pub(super) fn absent() -> Self {
        Self {
            code: Some(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    /// Exit 0 with a warning and no answer — the shape that must not read as an
    /// empty result.
    pub(super) fn warned(stderr: &str) -> Self {
        Self {
            code: Some(0),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Exit 0 having answered and warned, which git does: `for-each-ref` lists
    /// the good tags and reports a broken ref on stderr in the same run. The
    /// answer stands.
    pub(super) fn said_and_warned(stdout: impl Into<Vec<u8>>, stderr: &str) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.into(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Exit non-zero with a diagnostic, which is a failure to look rather than
    /// an answer of "no".
    ///
    /// # Panics
    ///
    /// On a shape that contradicts that: exit 0 is a success, and exit 1 with
    /// nothing written is [`Self::absent`]. Both were constructible, and each
    /// hands the code under test the opposite of what the script says.
    pub(super) fn failed(code: i32, stderr: &str) -> Self {
        assert!(
            code != 0,
            "a failure exits non-zero; exit 0 is `said` or `warned`"
        );
        assert!(
            !stderr.is_empty(),
            "a diagnosed failure writes a diagnostic; exit 1 with nothing written is `absent`"
        );
        Self {
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// A shape with no name: the wrapper behaviour a classifier has to reject,
    /// rather than anything git is known to produce.
    pub(super) fn exactly(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Self {
        Self {
            code,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    /// Killed before it could exit, so there is no code to read.
    pub(super) fn was_signalled() -> Self {
        Self {
            code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

/// Where an answer comes from. Only [`Runner::Child`] ships.
enum Runner {
    Child,
    #[cfg(test)]
    Fake(fake::Fake),
}

/// Runs git in one repository.
pub(super) struct Git {
    repo: PathBuf,
    runner: Runner,
}

impl Git {
    pub(super) fn at(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            runner: Runner::Child,
        }
    }

    /// Answers from a script instead of a repository, each keyed by the command
    /// it answers. The path is never read.
    #[cfg(test)]
    pub(super) fn answering(replies: impl IntoIterator<Item = (&'static str, Reply)>) -> Self {
        Self {
            repo: PathBuf::new(),
            runner: Runner::Fake(fake::Fake::answering(replies)),
        }
    }

    /// Each operation the caller asked for, in order, named as [`Op::name`]
    /// names it — the same phrase a failure quotes.
    ///
    /// # Panics
    ///
    /// When called on a [`Git`] that runs real children, which cannot report
    /// what it spawned.
    #[cfg(test)]
    pub(super) fn asked(&self) -> Vec<String> {
        match &self.runner {
            Runner::Fake(fake) => fake.asked(),
            Runner::Child => panic!("only a scripted Git records what it was asked"),
        }
    }

    /// Trimmed stdout.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn text(&self, op: Op<'_>) -> Result<String, CliError> {
        let reply = self.checked(op, Reads::Text)?;
        if op.spec().lossy {
            return Ok(String::from_utf8_lossy(&reply.stdout).trim().to_owned());
        }
        String::from_utf8(reply.stdout)
            .map(|text| text.trim().to_owned())
            .map_err(|_| Self::fail(op, "output is not valid UTF-8"))
    }

    /// Raw stdout, for NUL-separated paths that may not be UTF-8 as a whole.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn bytes(&self, op: Op<'_>) -> Result<Vec<u8>, CliError> {
        Ok(self.checked(op, Reads::Paths)?.stdout)
    }

    /// NUL-separated paths. `-z` turns quoting off, so a path carrying newlines,
    /// boundary whitespace, or non-ASCII bytes arrives byte-for-byte and
    /// package-prefix attribution stays exact. A non-UTF-8 path cannot be
    /// compared against manifest-derived package directories, so it fails loudly
    /// rather than being lossily rewritten into one that misses its package.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn paths(&self, op: Op<'_>) -> Result<Vec<String>, CliError> {
        let stdout = self.bytes(op)?;
        split_nul_paths(&stdout).ok_or_else(|| {
            Self::fail(
                op,
                "listed a path that is not valid UTF-8; oakum cannot attribute it to a package",
            )
        })
    }

    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn run(&self, op: Op<'_>) -> Result<(), CliError> {
        self.checked(op, Reads::Text)?;
        Ok(())
    }

    /// For the queries git answers with an exit code. `Ok(false)` only for exit 1
    /// with nothing written, which is how git says "no"; a diagnosed failure is
    /// an error, so "we could not look" never becomes a verdict.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn predicate(&self, op: Op<'_>) -> Result<bool, CliError> {
        let reply = self.answered(op, Reads::Text)?;
        if reply.succeeded() {
            return Ok(true);
        }
        if reply.said_no() {
            return Ok(false);
        }
        Err(Self::fail(op, &reply.detail()))
    }

    /// `Ok(None)` for the queries that report "absent" as exit 1 with nothing
    /// written — `config --get-regexp` with no match, `rev-parse --verify
    /// --quiet` on a missing ref. Any other failure is still an error: a
    /// wrapper that exits 1 with a diagnostic did not look, which is not the
    /// same as looking and finding nothing.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn optional_text(&self, op: Op<'_>) -> Result<Option<String>, CliError> {
        let reply = self.answered(op, Reads::Text)?;
        if !reply.succeeded() {
            if reply.said_no() {
                return Ok(None);
            }
            return Err(Self::fail(op, &reply.detail()));
        }
        let text = String::from_utf8(reply.stdout)
            .map_err(|_| Self::fail(op, "output is not valid UTF-8"))?;
        Ok(Some(text.trim().to_owned()).filter(|text| !text.is_empty()))
    }

    /// The answer, with a failed child already turned into the operation's own
    /// error. Callers that read an exit code as data go through [`Self::ask`].
    fn checked(&self, op: Op<'_>, reads: Reads) -> Result<Reply, CliError> {
        let reply = self.answered(op, reads)?;
        if reply.succeeded() {
            return Ok(reply);
        }
        Err(Self::fail(op, &reply.detail()))
    }

    /// The answer, refusing a child that exited 0 without giving one. What
    /// silence means is a property of the operation, not of the accessor
    /// reading it: keyed on the outcome class instead, this reached neither
    /// `predicate` nor `optional_text`, and a config read that never ran came
    /// back as "no remote suppresses tags".
    fn answered(&self, op: Op<'_>, reads: Reads) -> Result<Reply, CliError> {
        let reply = self.ask(op)?;
        if !reply.succeeded() || reply.spoke(reads) {
            return Ok(reply);
        }
        match op.spec().answer {
            Answer::Always => Err(Self::unanswered(op, &reply)),
            // Any stderr disqualifies, benign text included: an `ls-remote`
            // that found no tags while ssh wrote `Warning: Permanently added
            // ... to the list of known hosts` is refused. Deliberate — nothing
            // reliably separates a benign line from a consequential one, and
            // unlike `core.fsmonitor` no setting drops the diagnostic without
            // dropping the check with it. The refusal quotes the warning, so a
            // second run resolves it.
            Answer::Sometimes if reply.diagnostic().is_some() => Err(Self::unanswered(op, &reply)),
            Answer::Sometimes | Answer::Never => Ok(reply),
        }
    }

    fn ask(&self, op: Op<'_>) -> Result<Reply, CliError> {
        let owned = op.argv();
        let args: Vec<&str> = owned.iter().map(String::as_str).collect();
        match &self.runner {
            Runner::Child => self.child(op, &args),
            #[cfg(test)]
            Runner::Fake(fake) => Ok(fake.answer(op.name())),
        }
    }

    fn child(&self, op: Op<'_>, args: &[&str]) -> Result<Reply, CliError> {
        let started = if op.spec().reaches_remote {
            env::remote_command(&self.repo, args)?.output()
        } else {
            env::local_command(&self.repo, args).output()
        };
        // The OS reason separates a missing binary from a permission problem
        // from a fork failure, and a support case needs the difference.
        started
            .map(Reply::from)
            .map_err(|err| Self::fail(op, &format!("could not run git: {err}")))
    }

    /// Separate from [`Self::fail`] because the child exited 0: a reader told
    /// that git "failed" checks the exit code, finds success, and concludes
    /// oakum is wrong.
    fn unanswered(op: Op<'_>, reply: &Reply) -> CliError {
        Self::phrase(op, |what| match reply.diagnostic() {
            Some(said) => format!("git {what} answered nothing while reporting: {said}"),
            None => format!("git {what} exited 0 without answering"),
        })
    }

    fn fail(op: Op<'_>, detail: &str) -> CliError {
        Self::phrase(op, |what| format!("git {what} failed: {detail}"))
    }

    /// One place decides `unverified` versus a plain error, so a new message
    /// cannot pick the wrong one.
    fn phrase(op: Op<'_>, message: impl FnOnce(&str) -> String) -> CliError {
        let what = match op.operand() {
            Some(operand) => format!("{} {operand}", op.name()),
            None => op.name().to_owned(),
        };
        let message = message(&what);
        match op.spec().outcome {
            Outcome::Verification => CliError::unverified(format!("unverified: {message}")),
            Outcome::Action => CliError::new(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Answer::{Always, Never, Sometimes};
    use super::Outcome::{Action, Verification};
    use super::{split_nul_paths, Answer, CliError, Git, Op, Outcome, Reply};

    /// The shapes below all arrive as "git exited non-zero" or "git printed
    /// nothing", and telling them apart is the whole of the three-outcome rule.
    /// A real repository produces them only through a shell shim on `PATH`,
    /// which `tests/check.rs` does and which runs on unix alone.
    #[test]
    fn exit_one_with_nothing_written_is_how_git_says_absent() {
        assert_eq!(
            Git::answering([("config --get-regexp tagopt", Reply::absent())])
                .optional_text(Op::TagOptRemotes)
                .expect("absent"),
            None
        );
        assert!(!Git::answering([("rev-parse --verify", Reply::absent())])
            .predicate(Op::RefExists {
                reference: "refs/tags/v1.0.0",
            })
            .expect("absent"));
    }

    /// The other three ways a child can exit without answering. Each was a
    /// surviving mutant: drop any conjunct of `said_no` and a git that could not
    /// look reports "absent", which decides whether a tag exists.
    #[test]
    fn only_exit_one_with_both_streams_empty_means_absent() {
        let diagnosed = Git::answering([(
            "config --get-regexp tagopt",
            Reply::failed(1, "fatal: not a git repository"),
        )])
        .optional_text(Op::TagOptRemotes)
        .expect_err("a diagnosed exit must not read as absence");
        assert!(
            matches!(diagnosed, CliError::Unverified { .. }),
            "{diagnosed:?}"
        );
        assert!(
            diagnosed.to_string().contains("not a git repository"),
            "{diagnosed}"
        );

        let silent = Git::answering([(
            "config --get-regexp tagopt",
            Reply::exactly(Some(128), b"", b""),
        )])
        .optional_text(Op::TagOptRemotes)
        .expect_err("exit 128 is not exit 1");
        assert!(silent.to_string().contains("exit 128"), "{silent}");

        let spoke = Git::answering([(
            "config --get-regexp tagopt",
            Reply::exactly(Some(1), b"remote.origin.tagopt --no-tags", b""),
        )])
        .optional_text(Op::TagOptRemotes)
        .expect_err("a child that wrote an answer did not say no");
        assert!(spoke.to_string().contains("exit 1"), "{spoke}");

        let killed = Git::answering([("rev-parse --verify", Reply::was_signalled())])
            .predicate(Op::RefExists {
                reference: "refs/tags/v1.0.0",
            })
            .expect_err("a signal is not an answer of no");
        assert!(
            killed.to_string().contains("terminated by a signal"),
            "{killed}"
        );
    }

    /// The fake keys on these, and it matches them exactly, so the only thing
    /// keying needs from them is that no two collide. `rev-parse --verify` and
    /// `rev-parse --verify refs/tags` are the close pair — one is a character
    /// prefix of the other, which is what made an argv-prefix key need rules
    /// about how much of a command line counts.
    #[test]
    fn every_operation_has_its_own_name() {
        let mut names: Vec<&str> = Op::EVERY.iter().map(Op::name).collect();
        let listed = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            listed,
            "two operations share a name: {names:?}"
        );
    }

    /// Git's trace settings write to stderr unless pointed at a file, and every
    /// rule above treats stderr as a diagnostic. Measured on git 2.55: `1`,
    /// `2`, and `true` all print to stderr, an unrecognised value warns and
    /// then prints to stderr, and only an absolute path is written to a file.
    #[test]
    fn only_a_trace_that_names_a_file_is_left_in_place() {
        for value in ["1", "2", "true", "relative.log", ""] {
            assert!(
                !super::traces_to_a_file(std::ffi::OsStr::new(value)),
                "{value:?} reaches our stderr"
            );
        }
        assert!(super::traces_to_a_file(std::ffi::OsStr::new(
            "/tmp/oakum-trace.event"
        )));
    }

    /// Asserted against the command `untrace` actually builds. Written against
    /// its own literals instead, this test passed with `untrace` gutted to an
    /// empty body — the whole suite did.
    #[test]
    fn untrace_silences_every_channel_and_leaves_the_rest_alone() {
        let inherited = [
            ("GIT_TRACE", "1"),
            ("GIT_TRACE_PACKET", "1"),
            ("GIT_TRACE_A_CHANNEL_ADDED_LATER", "1"),
            ("GIT_TRACE2_EVENT", "relative.log"),
            ("GIT_TRACE2_PERF", "/tmp/oakum-trace.perf"),
        ]
        .map(|(name, value)| {
            (
                std::ffi::OsString::from(name),
                std::ffi::OsString::from(value),
            )
        });
        let mut command = std::process::Command::new("git");
        command.env("GIT_TERMINAL_PROMPT", "0");
        super::untrace_from(&mut command, inherited);

        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let setting = |name: &str| {
            envs.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };

        // Removed outright, including one nobody enumerated.
        for name in [
            "GIT_TRACE",
            "GIT_TRACE_PACKET",
            "GIT_TRACE_A_CHANNEL_ADDED_LATER",
        ] {
            assert_eq!(setting(name), Some(None), "{name} still reaches the child");
        }
        // Trace2 is set off rather than removed, because git config can turn it
        // back on where an environment edit cannot reach.
        assert_eq!(
            setting("GIT_TRACE2_EVENT"),
            Some(Some(String::from("0"))),
            "config-driven trace2 is still live"
        );
        // A trace2 channel the caller pointed at a file is left entirely alone —
        // the answer has to come from the environment handed in, not the
        // process's own, or an exported value decides for an environment that
        // never mentioned it.
        assert_eq!(
            setting("GIT_TRACE2_PERF"),
            None,
            "a trace2 file target was overridden"
        );
        assert_eq!(
            setting("GIT_TERMINAL_PROMPT"),
            Some(Some(String::from("0")))
        );
    }

    /// And matching stops at token boundaries. `RefExists` asks for
    /// `refs/tags/v1.0.0`; `LocalTagCommit` asks for the same string with
    /// `^{}` appended, so a character-wise prefix would let a script for one
    /// answer the other's child — the substitution this fake exists to expose.
    #[test]
    #[should_panic(expected = "nothing scripted answers")]
    fn a_command_that_only_shares_a_character_prefix_does_not_answer() {
        let git = Git::answering([(
            "rev-parse --verify --quiet refs/tags/v1.0.0",
            Reply::said("cafebabe"),
        )]);
        let _ = git.optional_text(Op::LocalTagCommit { tag: "v1.0.0" });
    }

    /// One of the two rules: a child that always prints and printed nothing did
    /// not answer, whatever it exited with. Keying this on the outcome class
    /// left it off `predicate` and `optional_text` entirely, which is how a
    /// config read that never ran came back as "no remote suppresses tags".
    #[test]
    fn a_child_that_always_prints_and_printed_nothing_is_refused_on_every_accessor() {
        let warning = "warning: unable to access '/etc/gitconfig': Permission denied";
        let asked = Git::answering([("config --get-regexp tagopt", Reply::warned(warning))])
            .optional_text(Op::TagOptRemotes)
            .expect_err("optional_text");
        assert!(matches!(asked, CliError::Unverified { .. }), "{asked:?}");
        assert!(asked.to_string().contains("Permission denied"), "{asked}");

        let voted = Git::answering([("rev-parse --verify", Reply::warned(warning))])
            .predicate(Op::RefExists {
                reference: "refs/tags/v1.0.0",
            })
            .expect_err("predicate");
        assert!(voted.to_string().contains("answered nothing"), "{voted}");

        let read = Git::answering([("rev-parse HEAD", Reply::warned(warning))])
            .text(Op::Head)
            .expect_err("text");
        assert!(read.to_string().contains("Permission denied"), "{read}");

        // Nothing on either stream: `rev-parse --verify --quiet` reports absence
        // as exit 1, so exit 0 in silence is a wrapper, not an answer.
        let mute = Git::answering([(
            "config --get-regexp tagopt",
            Reply::exactly(Some(0), b"", b""),
        )])
        .optional_text(Op::TagOptRemotes)
        .expect_err("silence is not absence");
        assert!(
            mute.to_string().contains("exited 0 without answering"),
            "{mute}"
        );
    }

    /// The second rule. `for-each-ref` legitimately prints nothing — a
    /// repository can have no reachable tags — but a diagnostic alongside that
    /// silence means the emptiness was never established (ADR-0014).
    #[test]
    fn a_verification_that_reported_nothing_while_warning_is_not_an_empty_look() {
        let err = Git::answering([(
            "for-each-ref --merged HEAD",
            Reply::warned("warning: refname 'v1.0.0' is ambiguous"),
        )])
        .text(Op::ReachableTags)
        .expect_err("a warned look must not read as no tags");
        assert!(matches!(err, CliError::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("ambiguous"), "{err}");
    }

    /// `Answer::Sometimes`: the emptiness is a real answer, and a diagnostic
    /// alongside it is not. These two gate `oakum release`, and a `status` or
    /// `log` that never ran must not read as "clean" and "no skip-ci marker".
    #[test]
    fn a_refusal_gate_that_never_looked_is_not_a_clean_answer() {
        for (command, op) in [
            ("status --porcelain", Op::WorktreeStatus),
            ("log -1", Op::HeadMessage),
            ("diff-tree", Op::CommitPaths { hash: "cafebabe" }),
        ] {
            let err = Git::answering([(command, Reply::warned("fatal: could not read index"))])
                .text(op)
                .expect_err("{op:?} must not read as an empty answer");
            assert!(err.to_string().contains("could not read index"), "{err}");
        }
    }

    /// `Answer::Never`: silence proves nothing either way, so no rule is drawn
    /// from it. A successful `git push` writes its whole report to stderr and
    /// nothing to stdout, and `check-ref-format` writes nothing on either
    /// verdict — refusing these would fail every release.
    #[test]
    fn work_that_answers_through_its_exit_code_is_not_refused_for_writing_to_stderr() {
        Git::answering([(
            "push",
            Reply::warned("To github.com:oakoss/oakum.git\n * [new tag] v1.0.0 -> v1.0.0"),
        )])
        .run(Op::PushTag {
            remote: "origin",
            tag: "v1.0.0",
        })
        .expect("push reports through stderr on success");

        assert!(Git::answering([(
            "check-ref-format",
            Reply::warned("warning: unable to access '/etc/gitconfig'"),
        )])
        .predicate(Op::ValidRefName {
            reference: "v1.0.0",
        })
        .expect("check-ref-format reports through its exit code"));
    }

    /// The guard asks the accessor doing the reading, because the two draw the
    /// line in different places and one rule is wrong for one of them. `text`
    /// trims — `str::trim`, so Unicode `White_Space`, not the ASCII subset —
    /// while `paths` keeps every non-empty NUL record byte-for-byte.
    #[test]
    fn each_accessor_draws_the_silence_line_where_it_reads() {
        // Silence to `str::trim`, an answer to anything narrower: a vertical
        // tab, a no-break space, a line separator. `\x0B` is the one that was
        // live — `u8::is_ascii_whitespace` rejects it, and a `status` writing it
        // alongside a fatal passed the dirty-worktree gate.
        for stdout in ["\u{0B}", "\u{A0}", "\u{2028}"] {
            let err = Git::answering([(
                "status --porcelain",
                Reply::exactly(
                    Some(0),
                    stdout.as_bytes(),
                    b"fatal: could not read the index",
                ),
            )])
            .text(Op::WorktreeStatus)
            .expect_err("whitespace is not a clean worktree");
            assert!(
                err.to_string().contains("could not read the index"),
                "{err}"
            );
        }

        // `-z` turns quoting off, so a file named " " arrives as a one-byte
        // record and is a filename, not silence. Only a genuinely empty record
        // means nothing was listed.
        let listed = Git::answering([(
            "diff --name-only",
            Reply::exactly(
                Some(0),
                b" \0",
                b"warning: unable to access '/etc/gitconfig'",
            ),
        )])
        .paths(Op::ChangedPaths { from: "v1.0.0" })
        .expect("a whitespace filename is a filename");
        assert_eq!(listed, [" "]);

        let none = Git::answering([(
            "diff --name-only",
            Reply::exactly(
                Some(0),
                b"\0\0",
                b"warning: unable to access '/etc/gitconfig'",
            ),
        )])
        .paths(Op::ChangedPaths { from: "v1.0.0" })
        .expect_err("empty records listed nothing");
        assert!(none.to_string().contains("answered nothing"), "{none}");

        let absent = Git::answering([("rev-parse --verify refs/tags", Reply::said("  \n"))])
            .optional_text(Op::LocalTagCommit { tag: "v1.0.0" })
            .expect_err("whitespace is not a commit");
        assert!(absent.to_string().contains("without answering"), "{absent}");

        // The same trimming on the other stream: a diagnostic of only
        // whitespace has reported nothing, so an empty look stays an answer.
        assert_eq!(
            Git::answering([("remote", Reply::exactly(Some(0), b"", b" \n"))])
                .text(Op::RemoteNames)
                .expect("a blank diagnostic is not a diagnostic"),
            ""
        );
    }

    /// Every accessor trims, so a lone newline reaches the caller as `""`. The
    /// guard has to judge emptiness the same way or it is one byte wide.
    #[test]
    fn whitespace_on_stdout_is_not_an_answer() {
        let err = Git::answering([(
            "config --get-regexp tagopt",
            Reply::exactly(Some(0), b" \n\t", b"fatal: bad config line 9"),
        )])
        .optional_text(Op::TagOptRemotes)
        .expect_err("whitespace is not an answer");
        assert!(err.to_string().contains("bad config line 9"), "{err}");

        let looked = Git::answering([(
            "status --porcelain",
            Reply::exactly(Some(0), b"\n", b"fatal: could not read index"),
        )])
        .text(Op::WorktreeStatus)
        .expect_err("nor for a gate whose empty answer means clean");
        assert!(
            looked.to_string().contains("could not read index"),
            "{looked}"
        );
    }

    /// The one measured case of git writing to stderr while answering
    /// correctly: a `core.fsmonitor` hook it cannot execute makes it fall back,
    /// print `fatal: cannot exec ...`, and exit 0. Overriding the setting on
    /// the child removes the diagnostic and leaves the answer byte-identical,
    /// which is what lets the rule above stay fail-closed.
    #[test]
    fn the_worktree_read_overrides_a_broken_fsmonitor_rather_than_tolerating_it() {
        let argv = Op::WorktreeStatus.argv();
        assert_eq!(&argv[..2], ["-c", "core.fsmonitor=false"], "{argv:?}");
    }

    /// The other side of the same rule, and the reason it is not simply "any
    /// stderr is a failure". Both shapes are ones git produces: `for-each-ref`
    /// lists the good tags while reporting a broken ref, and a successful `git
    /// push` writes its whole report to stderr.
    #[test]
    fn a_child_that_answered_or_had_nothing_to_answer_is_not_refused() {
        let listed = Git::answering([(
            "for-each-ref --merged HEAD",
            Reply::said_and_warned(
                "refs/tags/v1.0.0\0commit\0cafebabe\0\0",
                "warning: ignoring broken ref refs/tags/junk",
            ),
        )])
        .text(Op::ReachableTags)
        .expect("a warning alongside an answer leaves the answer standing");
        assert!(listed.contains("v1.0.0"), "{listed}");

        Git::answering([(
            "push",
            Reply::warned("To github.com:oakoss/oakum.git\n * [new tag] v1.0.0"),
        )])
        .run(Op::PushTag {
            remote: "origin",
            tag: "v1.0.0",
        })
        .expect("push reports through stderr on success");
    }

    #[test]
    fn a_silent_failure_and_a_signal_do_not_render_alike() {
        let silent = Git::answering([("rev-parse HEAD", Reply::exactly(Some(128), b"", b""))])
            .text(Op::Head)
            .expect_err("exit 128");
        assert!(
            silent.to_string().contains("exit 128 with no diagnostic"),
            "{silent}"
        );
        let killed = Git::answering([("rev-parse HEAD", Reply::was_signalled())])
            .text(Op::Head)
            .expect_err("signalled");
        assert!(
            killed.to_string().contains("terminated by a signal"),
            "{killed}"
        );
    }

    /// `Spec::lossy` decides this, and both sides of it are worth pinning: a
    /// commit message survives a stray byte, an object name does not.
    #[test]
    fn only_a_lossy_read_accepts_bytes_that_are_not_utf8() {
        let message = Git::answering([("log -1", Reply::said(b"fix: caf\xff\n".to_vec()))])
            .text(Op::HeadMessage)
            .expect("a commit message is read lossily");
        assert_eq!(message, "fix: caf\u{fffd}");
        let err = Git::answering([("rev-parse HEAD", Reply::said(b"\xff".to_vec()))])
            .text(Op::Head)
            .expect_err("an object name must be valid UTF-8");
        assert!(err.to_string().contains("not valid UTF-8"), "{err}");
    }

    /// A failed action stays a plain error even though the same runner handles
    /// the verifications above.
    #[test]
    fn a_failed_action_is_not_unverified() {
        let err = Git::answering([("tag", Reply::failed(128, "fatal: tag already exists"))])
            .run(Op::AnnotatedTag {
                name: "v1.0.0",
                commit: "HEAD",
            })
            .expect_err("tag failed");
        assert!(matches!(err, CliError::Other(_)), "{err:?}");
    }

    /// The axes of every operation, stated rather than sampled.
    const AXES: [(Op<'static>, Outcome, bool, Answer, bool); 19] = [
        (Op::ReachableTags, Verification, false, Sometimes, false),
        (Op::IsShallow, Verification, false, Always, false),
        (Op::TagOptRemotes, Verification, false, Always, false),
        (Op::RemoteNames, Verification, false, Sometimes, false),
        (
            Op::AdvertisedTags { remote: "origin" },
            Verification,
            true,
            Sometimes,
            false,
        ),
        (
            Op::ChangedPaths { from: "v1.0.0" },
            Verification,
            false,
            Sometimes,
            false,
        ),
        (Op::Head, Action, false, Always, false),
        (
            Op::RemoteUrl { remote: "origin" },
            Action,
            false,
            Always,
            false,
        ),
        (Op::MergeBase { tip: "main" }, Action, false, Always, false),
        (
            Op::Commits { from: "v1.0.0" },
            Action,
            false,
            Sometimes,
            true,
        ),
        (
            Op::CommitPaths { hash: "cafebabe" },
            Action,
            false,
            Sometimes,
            false,
        ),
        (
            Op::CommitParents { hash: "cafebabe" },
            Action,
            false,
            Always,
            false,
        ),
        (
            Op::LocalTagCommit { tag: "v1.0.0" },
            Action,
            false,
            Always,
            false,
        ),
        (Op::WorktreeStatus, Action, false, Sometimes, false),
        (Op::HeadMessage, Action, false, Sometimes, true),
        (
            Op::RefExists {
                reference: "refs/tags/v1.0.0",
            },
            Action,
            false,
            Always,
            false,
        ),
        (
            Op::ValidRefName {
                reference: "v1.0.0",
            },
            Action,
            false,
            Never,
            false,
        ),
        (
            Op::AnnotatedTag {
                name: "v1.0.0",
                commit: "HEAD",
            },
            Action,
            false,
            Never,
            false,
        ),
        (
            Op::PushTag {
                remote: "origin",
                tag: "v1.0.0",
            },
            Action,
            true,
            Never,
            false,
        ),
    ];

    /// The three tests this replaces sampled three to six variants by hand,
    /// leaving ten operations with no assertion on any axis.
    ///
    /// What stops a new variant inheriting a neighbour's answers is `spec`
    /// having one arm per variant, measured: appending `| Self::NewProbe` to a
    /// group there is a compile error, while adding the variant to neither this
    /// table nor `Op::EVERY` passes — both are hand-written, so the length
    /// check only fires once someone lists it.
    #[test]
    fn every_operation_states_every_axis() {
        assert_eq!(AXES.len(), Op::EVERY.len(), "a new operation needs a row");
        for ((op, outcome, reaches_remote, answer, lossy), listed) in
            AXES.into_iter().zip(Op::EVERY)
        {
            assert_eq!(
                format!("{op:?}"),
                format!("{listed:?}"),
                "the table and `Op::EVERY` are in different orders"
            );
            let spec = op.spec();
            assert_eq!(spec.outcome, outcome, "{op:?} outcome");
            assert_eq!(spec.reaches_remote, reaches_remote, "{op:?} reaches_remote");
            assert_eq!(spec.answer, answer, "{op:?} answer");
            assert_eq!(spec.lossy, lossy, "{op:?} lossy");
        }
    }

    /// A failure has to say which remote or ref it was about; the subcommand
    /// alone cannot distinguish two configured remotes.
    #[test]
    fn a_failure_names_what_it_was_pointed_at() {
        assert_eq!(
            Op::AdvertisedTags { remote: "upstream" }
                .operand()
                .as_deref(),
            Some("upstream")
        );
        assert_eq!(
            Op::ChangedPaths { from: "v1.0.0" }.operand().as_deref(),
            Some("v1.0.0...HEAD")
        );
        assert_eq!(Op::ReachableTags.operand(), None);
    }

    /// `-z` turns quoting off, so a path carrying newlines, boundary
    /// whitespace, or non-ASCII bytes must arrive byte-for-byte.
    #[test]
    fn nul_records_are_preserved_exactly() {
        let parsed = split_nul_paths(b"pkg/a b\0pkg/\n weird \0pkg/caf\xc3\xa9.rs\0")
            .expect("valid utf-8 records");
        assert_eq!(parsed, ["pkg/a b", "pkg/\n weird ", "pkg/caf\u{e9}.rs"]);
        assert_eq!(split_nul_paths(b"").expect("empty"), Vec::<String>::new());
    }

    #[test]
    fn a_non_utf8_record_is_refused_rather_than_lossily_rewritten() {
        assert_eq!(split_nul_paths(b"pkg/\xff.bin\0"), None);
    }
}
