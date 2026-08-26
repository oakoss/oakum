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

use std::path::PathBuf;
use std::process::Command;

use super::git_env;
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

/// Everything the runner needs about an operation, decided in one place so a new
/// variant cannot pick up a default.
struct Spec {
    outcome: Outcome,
    reaches_remote: bool,
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
        lossy: false,
    };
    const REMOTE_LOOK: Self = Self {
        reaches_remote: true,
        ..Self::LOOK
    };
    const ACT: Self = Self {
        outcome: Outcome::Action,
        reaches_remote: false,
        lossy: false,
    };
    const REMOTE_ACT: Self = Self {
        reaches_remote: true,
        ..Self::ACT
    };
    const LOSSY_ACT: Self = Self {
        lossy: true,
        ..Self::ACT
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
            Self::WorktreeStatus => {
                owned(&["status", "--porcelain", "--untracked-files=all"])
            }
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

    /// One decision per operation, covering every axis at once. Three separate
    /// classifiers each defaulted silently, and the defaults were the dangerous
    /// answers: a remote operation that reads as local loses `BatchMode` and
    /// hangs, and a read that reads as an action turns "we could not look" into
    /// a plain error.
    const fn spec(&self) -> Spec {
        match self {
            Self::ReachableTags
            | Self::IsShallow
            | Self::TagOptRemotes
            | Self::RemoteNames
            | Self::ChangedPaths { .. } => Spec::LOOK,
            Self::AdvertisedTags { .. } => Spec::REMOTE_LOOK,
            Self::Commits { .. } | Self::HeadMessage => Spec::LOSSY_ACT,
            Self::PushTag { .. } => Spec::REMOTE_ACT,
            Self::Head
            | Self::RemoteUrl { .. }
            | Self::MergeBase { .. }
            | Self::CommitPaths { .. }
            | Self::CommitParents { .. }
            | Self::LocalTagCommit { .. }
            | Self::WorktreeStatus
            | Self::RefExists { .. }
            | Self::ValidRefName { .. }
            | Self::AnnotatedTag { .. } => Spec::ACT,
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

/// `None` when a record is not UTF-8. Kept separate from the run so the decoding
/// is testable without a repository.
fn split_nul_paths(stdout: &[u8]) -> Option<Vec<String>> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8(record.to_vec()).ok())
        .collect()
}

/// Runs git in one repository.
pub(super) struct Git {
    repo: PathBuf,
}

impl Git {
    pub(super) fn at(repo: impl Into<PathBuf>) -> Self {
        Self { repo: repo.into() }
    }

    /// Trimmed stdout.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn text(&self, op: Op<'_>) -> Result<String, CliError> {
        let output = self.output(op)?;
        if op.spec().lossy {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
        String::from_utf8(output.stdout)
            .map(|text| text.trim().to_owned())
            .map_err(|_| Self::fail(op, "output is not valid UTF-8"))
    }

    /// Raw stdout, for NUL-separated paths that may not be UTF-8 as a whole.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn bytes(&self, op: Op<'_>) -> Result<Vec<u8>, CliError> {
        Ok(self.output(op)?.stdout)
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
        self.output(op)?;
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
        let output = self.spawn(op)?;
        if output.status.success() {
            return Ok(true);
        }
        if Self::said_no(&output) {
            return Ok(false);
        }
        Err(Self::fail(op, &Self::detail(&output)))
    }

    /// Git reports "absent" or "no" as exit 1 with both streams empty. A wrapper
    /// that exits 1 with a diagnostic did not look, which is not the same thing.
    fn said_no(output: &std::process::Output) -> bool {
        output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty()
    }

    /// Git's own words when it wrote any; otherwise the status, so a silent
    /// failure and a child killed by a signal do not render identically.
    fn detail(output: &std::process::Output) -> String {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            return stderr.to_owned();
        }
        match output.status.code() {
            Some(code) => format!("exit {code} with no diagnostic"),
            None => String::from("terminated by a signal"),
        }
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
        let output = self.spawn(op)?;
        if !output.status.success() {
            if Self::said_no(&output) {
                return Ok(None);
            }
            return Err(Self::fail(op, &Self::detail(&output)));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| Self::fail(op, "output is not valid UTF-8"))?;
        Ok(Some(text.trim().to_owned()).filter(|text| !text.is_empty()))
    }

    fn output(&self, op: Op<'_>) -> Result<std::process::Output, CliError> {
        let output = self.spawn(op)?;
        if !output.status.success() {
            return Err(Self::fail(op, &Self::detail(&output)));
        }
        // A look that reported nothing while warning is not an empty answer.
        if op.spec().outcome == Outcome::Verification
            && output.stdout.is_empty()
            && !output.stderr.is_empty()
        {
            return Err(Self::fail(op, &Self::detail(&output)));
        }
        Ok(output)
    }

    fn spawn(&self, op: Op<'_>) -> Result<std::process::Output, CliError> {
        let owned = op.argv();
        let args: Vec<&str> = owned.iter().map(String::as_str).collect();
        let started = if op.spec().reaches_remote {
            git_env::remote_command(&self.repo, &args)?.output()
        } else {
            let mut command = Command::new("git");
            command
                .args(&args)
                .current_dir(&self.repo)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_ASKPASS", "");
            command.output()
        };
        // The OS reason separates a missing binary from a permission problem
        // from a fork failure, and a support case needs the difference.
        started.map_err(|err| Self::fail(op, &format!("could not run git: {err}")))
    }

    fn fail(op: Op<'_>, detail: &str) -> CliError {
        let what = match op.operand() {
            Some(operand) => format!("{} {operand}", op.name()),
            None => op.name().to_owned(),
        };
        match op.spec().outcome {
            Outcome::Verification => {
                CliError::unverified(format!("unverified: git {what} failed: {detail}"))
            }
            Outcome::Action => CliError::new(format!("git {what} failed: {detail}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{split_nul_paths, Op, Outcome};

    #[test]
    fn reads_that_feed_a_verification_are_unverified() {
        for op in [
            Op::ReachableTags,
            Op::IsShallow,
            Op::TagOptRemotes,
            Op::RemoteNames,
            Op::AdvertisedTags { remote: "origin" },
            Op::ChangedPaths { from: "main" },
        ] {
            assert_eq!(op.spec().outcome, Outcome::Verification, "{op:?}");
        }
    }

    #[test]
    fn work_that_failed_is_a_plain_error() {
        for op in [
            Op::Head,
            Op::WorktreeStatus,
            Op::AnnotatedTag {
                name: "v1.0.0",
                commit: "HEAD",
            },
            Op::PushTag {
                remote: "origin",
                tag: "v1.0.0",
            },
        ] {
            assert_eq!(op.spec().outcome, Outcome::Action, "{op:?}");
        }
    }

    /// The set that must carry prompt suppression; okm-6mz measured what
    /// happens when one is missed.
    #[test]
    fn only_ls_remote_and_push_reach_a_remote() {
        assert!(
            Op::AdvertisedTags { remote: "origin" }
                .spec()
                .reaches_remote
        );
        assert!(
            Op::PushTag {
                remote: "origin",
                tag: "v1"
            }
            .spec()
            .reaches_remote
        );
        for op in [
            Op::Head,
            Op::RemoteNames,
            Op::RemoteUrl { remote: "origin" },
        ] {
            assert!(!op.spec().reaches_remote, "{op:?}");
        }
    }

    /// The two log reads carry commit messages; everything else carries
    /// identifiers git generates, which are ASCII by construction. `refuse_skip_ci`
    /// reads `%B` to look for a marker substring, and has no business failing on
    /// one stray byte.
    #[test]
    fn only_the_commit_log_decodes_lossily() {
        for op in [Op::Commits { from: "v1" }, Op::HeadMessage] {
            assert!(op.spec().lossy, "{op:?}");
        }
        for op in [Op::Head, Op::ReachableTags, Op::WorktreeStatus] {
            assert!(!op.spec().lossy, "{op:?}");
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
