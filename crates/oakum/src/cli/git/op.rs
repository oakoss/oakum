//! Every git operation oakum performs, and the axes that describe one.
//!
//! [`Op`] is closed so the set of git operations in the crate is a list one can
//! read, and [`Op::shape`] is one table with a row per operation, so no
//! operation can state its argv and leave its outcome class, name, remote, or
//! operand to a default. The runner in the parent module reads a shape; it
//! never asks an operation a question one axis at a time.

use super::{CliError, Commit};

/// Where a failed child lands in the three-outcome vocabulary (AGENTS.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Outcome {
    /// Informs a verification, so a failure to look is `unverified` — never
    /// silently "nothing to report".
    Verification,
    /// Does work, so a failure is a plain error.
    Action,
}

/// Which way a remote operation talks to its remote. A push and a fetch can go
/// to different places, so an operation judged by the wrong URL gets a note
/// naming a transport it never uses. Whether a remote is contacted at all is
/// the `Option` around this, decided in [`Op::shape`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Direction {
    Fetch,
    Push,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Contact<'a> {
    pub(super) remote: &'a str,
    pub(super) direction: Direction,
}

impl Contact<'_> {
    /// Keyed by both: a remote can fetch over one transport and push over
    /// another, so by name alone the fetch note swallows the push one.
    pub(super) fn key(self) -> (String, Direction) {
        (self.remote.to_owned(), self.direction)
    }
}

/// What a successful child writes to stdout, which is what decides the meaning
/// of one that wrote nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Answer {
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

/// What the runner needs about an operation beyond its remote, which
/// [`OpShape::contact`] carries.
pub(super) struct Spec {
    outcome: Outcome,
    pub(super) answer: Answer,
    /// Free-form commit text, which git does not promise is UTF-8: a commit
    /// object written verbatim by another tool carries raw bytes that `git log`
    /// passes straight through. Replacing one with U+FFFD beats refusing to read
    /// the message at all.
    pub(super) lossy: bool,
}

impl Spec {
    const LOOK: Self = Self {
        outcome: Outcome::Verification,
        answer: Answer::Sometimes,
        lossy: false,
    };
    const ANSWERING_LOOK: Self = Self {
        answer: Answer::Always,
        ..Self::LOOK
    };
    const ACT: Self = Self {
        outcome: Outcome::Action,
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
}

/// The trailer half of [`Op::CommitMessage`]'s format. A git too old to know
/// it echoes the specifier verbatim at exit 0 (measured on 2.55 with an
/// unknown option), which is what `commit_text`'s guard catches: the whole
/// trailers half equals the atom while the message itself does not carry it.
/// A genuine trailer value quoting the specifier appears in both halves, so
/// it stays a value.
pub(super) const SKIP_CHECKS_ATOM: &str = "%(trailers:key=skip-checks,valueonly,unfold)";

/// Every git operation oakum performs.
#[derive(Clone, Copy, Debug)]
pub(in crate::cli) enum Op<'a> {
    /// Tags reachable from HEAD with their peeled identity (ADR-0014).
    ReachableTags,
    /// Every ref under `refs/tags` with its recorded object: a `--merged`
    /// walk silently drops a ref whose object is missing or unreachable
    /// (measured, git 2.55), so only this listing sees those tags.
    AllTags,
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
    /// Every remote's fetch and push URLs in one child. `remote.<name>.pushurl`
    /// can point somewhere else entirely and can be set more than once —
    /// measured, `remote -v` lists every one, and applies `insteadOf` rewrites
    /// exactly as `get-url` does.
    RemoteUrls,
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
    /// The full message of one commit, a NUL, then the values of its
    /// `skip-checks` trailers as git parses them — one child answers both the
    /// bracketed-annotation scan and the trailer question, with git's own
    /// parser as the trailer authority rather than an approximation of it.
    CommitMessage {
        commit: &'a str,
    },
    RefExists {
        reference: &'a str,
    },
    ValidRefName {
        reference: &'a str,
    },
    /// Every path under `.github/workflows` in one commit's tree,
    /// NUL-separated. Exits 0 with nothing written when the path is absent
    /// there (measured, git 2.55): "no workflows at that commit" is a
    /// completed look, not a failure.
    WorkflowTree {
        commit: &'a Commit,
    },
    BlobText {
        commit: &'a Commit,
        path: &'a str,
    },
    /// The tree entry for `path` in `commit` as `<mode> <type> <object>\t<path>`:
    /// empty output when absent, a diagnostic and exit 128 when the commit
    /// itself is unknown. The pathspec is literal, so `[` in a path is not a
    /// glob.
    TreeEntry {
        commit: &'a Commit,
        path: &'a str,
    },
    /// The commit that added `path`, as `hash NUL author NUL email NUL
    /// subject`; empty when the file was never committed. `--ignore-missing`
    /// makes an unborn HEAD empty output too rather than exit 128 (measured,
    /// git 2.55). The pathspec is literal.
    FileAddedBy {
        path: &'a str,
    },
    /// The commit slot takes a [`Commit`], not a committish: only an id a git
    /// read produced can name where the one operation that writes a ref points
    /// it, so a tag name cannot compile into the slot.
    AnnotatedTag {
        name: &'a str,
        commit: &'a Commit,
    },
    PushTag {
        remote: &'a str,
        tag: &'a str,
    },
}

/// `Commit` holds a `String`, so no fixture can be `const`; the operation
/// tables are functions for this one slot.
#[cfg(test)]
pub(super) fn fixture_commit() -> &'static Commit {
    static FIXTURE: std::sync::OnceLock<Commit> = std::sync::OnceLock::new();
    FIXTURE.get_or_init(|| Commit(String::from("cafebabe")))
}

/// Every axis of one operation in one row, so a variant cannot state its
/// argv and leave its class, name, contact, or operand to a default.
pub(super) struct OpShape<'a> {
    pub(super) argv: Vec<String>,
    pub(super) spec: Spec,
    /// The subcommand, for diagnostics. Paired with `operand` rather than
    /// rendering the whole argv, which would repeat the flags in every message.
    pub(super) name: &'static str,
    /// The remote this operation contacts and which way. An operation that
    /// contacts a remote but says `None` here spawns a child with no
    /// `BatchMode` and hangs on a prompt.
    pub(super) contact: Option<Contact<'a>>,
    /// What the operation was pointed at, so a failure names which remote or
    /// ref it was. Every value here is oakum's own — a remote name, a ref, a
    /// range — not text git produced.
    pub(super) operand: Option<String>,
}

impl<'a> Op<'a> {
    /// The whole of one operation. No axis may default silently: a remote
    /// operation that reads as local loses `BatchMode` and hangs, and a read
    /// that reads as an action turns "we could not look" into a plain error.
    // One arm per variant, never a `|` group: an operation appended to a group
    // compiles while stating nothing and inherits whatever its neighbour
    // happened to be.
    #[expect(
        clippy::too_many_lines,
        reason = "a table, one row per operation; it grows with the enum"
    )]
    pub(super) fn shape(&self) -> OpShape<'a> {
        let owned = |parts: &[&str]| parts.iter().map(|part| (*part).to_owned()).collect();
        let named = |value: &str| Some(value.to_owned());
        match *self {
            Self::ReachableTags => OpShape {
                argv: owned(&[
                    "for-each-ref",
                    "--merged=HEAD",
                    "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)",
                    "refs/tags",
                ]),
                spec: Spec::LOOK,
                name: "for-each-ref --merged HEAD",
                contact: None,
                operand: None,
            },
            // Never `%(refname:short)`: a tag shadowed by a same-named branch
            // shortens to `tags/v1` and stops matching (measured, git 2.55).
            Self::AllTags => OpShape {
                argv: owned(&[
                    "for-each-ref",
                    "--format=%(refname)%00%(objectname)",
                    "refs/tags",
                ]),
                spec: Spec::LOOK,
                name: "for-each-ref refs/tags",
                contact: None,
                operand: None,
            },
            Self::IsShallow => OpShape {
                argv: owned(&["rev-parse", "--is-shallow-repository"]),
                spec: Spec::ANSWERING_LOOK,
                name: "rev-parse --is-shallow-repository",
                contact: None,
                operand: None,
            },
            Self::TagOptRemotes => OpShape {
                argv: owned(&["config", "--get-regexp", r"^remote\..*\.tagopt$"]),
                spec: Spec::ANSWERING_LOOK,
                name: "config --get-regexp tagopt",
                contact: None,
                operand: None,
            },
            Self::RemoteNames => OpShape {
                argv: owned(&["remote"]),
                spec: Spec::LOOK,
                name: "remote",
                contact: None,
                operand: None,
            },
            Self::AdvertisedTags { remote } => OpShape {
                argv: owned(&["ls-remote", "--tags", "--", remote]),
                spec: Spec::LOOK,
                name: "ls-remote --tags",
                contact: Some(Contact {
                    remote,
                    direction: Direction::Fetch,
                }),
                operand: named(remote),
            },
            Self::ChangedPaths { from } => OpShape {
                argv: vec![
                    String::from("diff"),
                    String::from("-z"),
                    String::from("--name-only"),
                    format!("{from}...HEAD"),
                ],
                spec: Spec::LOOK,
                name: "diff --name-only",
                contact: None,
                operand: Some(format!("{from}...HEAD")),
            },
            Self::Head => OpShape {
                argv: owned(&["rev-parse", "HEAD"]),
                spec: Spec::ANSWERING_ACT,
                name: "rev-parse HEAD",
                contact: None,
                operand: None,
            },
            Self::RemoteUrl { remote } => OpShape {
                argv: owned(&["remote", "get-url", "--", remote]),
                spec: Spec::ANSWERING_ACT,
                name: "remote get-url",
                contact: None,
                operand: named(remote),
            },
            Self::RemoteUrls => OpShape {
                argv: owned(&["remote", "-v"]),
                spec: Spec::ACT,
                name: "remote -v",
                contact: None,
                operand: None,
            },
            Self::MergeBase { tip } => OpShape {
                argv: owned(&["merge-base", tip, "HEAD"]),
                spec: Spec::ANSWERING_ACT,
                name: "merge-base",
                contact: None,
                operand: named(tip),
            },
            Self::Commits { from } => OpShape {
                argv: vec![
                    String::from("log"),
                    format!("{from}..HEAD"),
                    String::from("--reverse"),
                    String::from("--format=%H%x00%s%x00%b%x00"),
                ],
                spec: Spec::LOSSY_ACT,
                name: "log",
                contact: None,
                operand: Some(format!("{from}..HEAD")),
            },
            Self::CommitPaths { hash } => OpShape {
                argv: owned(&[
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "-z",
                    "-r",
                    "--root",
                    hash,
                ]),
                spec: Spec::ACT,
                name: "diff-tree",
                contact: None,
                operand: named(hash),
            },
            Self::CommitParents { hash } => OpShape {
                argv: owned(&["rev-list", "--parents", "-n", "1", hash]),
                spec: Spec::ANSWERING_ACT,
                name: "rev-list --parents",
                contact: None,
                operand: named(hash),
            },
            Self::LocalTagCommit { tag } => OpShape {
                argv: vec![
                    String::from("rev-parse"),
                    String::from("--verify"),
                    String::from("--quiet"),
                    format!("refs/tags/{tag}^{{}}"),
                ],
                spec: Spec::ANSWERING_ACT,
                name: "rev-parse --verify refs/tags",
                contact: None,
                operand: named(tag),
            },
            // A `core.fsmonitor` hook that cannot be executed makes git fall
            // back, answer correctly, and write `fatal: cannot exec ...` to
            // stderr while exiting 0 — indistinguishable from a status that
            // never ran. Overriding the setting removes the diagnostic and
            // leaves the answer byte-identical (measured both clean and dirty).
            Self::WorktreeStatus => OpShape {
                argv: owned(&[
                    "-c",
                    "core.fsmonitor=false",
                    "status",
                    "--porcelain",
                    "--untracked-files=all",
                ]),
                spec: Spec::ACT,
                name: "status --porcelain",
                contact: None,
                operand: None,
            },
            Self::CommitMessage { commit } => OpShape {
                argv: vec![
                    String::from("log"),
                    String::from("-1"),
                    format!("--format=%B%x00{SKIP_CHECKS_ATOM}"),
                    String::from(commit),
                ],
                spec: Spec::LOSSY_ACT,
                name: "log -1",
                contact: None,
                operand: named(commit),
            },
            // Without `--quiet`, an absent ref exits 128 with a diagnostic —
            // the same shape as an unreadable repository.
            Self::RefExists { reference } => OpShape {
                argv: owned(&["rev-parse", "--verify", "--quiet", reference]),
                spec: Spec::ANSWERING_ACT,
                name: "rev-parse --verify",
                contact: None,
                operand: named(reference),
            },
            Self::ValidRefName { reference } => OpShape {
                argv: vec![
                    String::from("check-ref-format"),
                    format!("refs/tags/{reference}"),
                ],
                spec: Spec::PERFORM,
                name: "check-ref-format",
                contact: None,
                operand: named(reference),
            },
            Self::WorkflowTree { commit } => OpShape {
                argv: vec![
                    String::from("ls-tree"),
                    String::from("--name-only"),
                    String::from("-r"),
                    String::from("-z"),
                    commit.as_str().to_owned(),
                    String::from("--"),
                    String::from(".github/workflows/"),
                ],
                spec: Spec::LOOK,
                name: "ls-tree",
                contact: None,
                operand: named(commit.as_str()),
            },
            Self::BlobText { commit, path } => OpShape {
                argv: vec![
                    String::from("cat-file"),
                    String::from("blob"),
                    format!("{}:{path}", commit.as_str()),
                ],
                spec: Spec::LOOK,
                name: "cat-file blob",
                contact: None,
                operand: Some(format!("{}:{path}", commit.as_str())),
            },
            Self::TreeEntry { commit, path } => OpShape {
                argv: vec![
                    String::from("ls-tree"),
                    String::from("-z"),
                    commit.as_str().to_owned(),
                    String::from("--"),
                    format!(":(literal){path}"),
                ],
                spec: Spec::LOOK,
                name: "ls-tree --",
                contact: None,
                operand: Some(format!("{} -- {path}", commit.as_str())),
            },
            Self::FileAddedBy { path } => OpShape {
                argv: vec![
                    String::from("log"),
                    String::from("--ignore-missing"),
                    String::from("--diff-filter=A"),
                    String::from("-n"),
                    String::from("1"),
                    String::from("--format=%H%x00%an%x00%ae%x00%s"),
                    String::from("HEAD"),
                    String::from("--"),
                    format!(":(literal){path}"),
                ],
                spec: Spec::LOOK,
                name: "log --diff-filter=A",
                contact: None,
                operand: named(path),
            },
            Self::AnnotatedTag { name, commit } => OpShape {
                argv: vec![
                    String::from("tag"),
                    String::from("-m"),
                    name.to_owned(),
                    String::from("--"),
                    name.to_owned(),
                    commit.as_str().to_owned(),
                ],
                spec: Spec::PERFORM,
                name: "tag",
                contact: None,
                operand: named(name),
            },
            Self::PushTag { remote, tag } => OpShape {
                argv: vec![
                    String::from("push"),
                    String::from("--"),
                    remote.to_owned(),
                    format!("refs/tags/{tag}"),
                ],
                spec: Spec::PERFORM,
                name: "push",
                contact: Some(Contact {
                    remote,
                    direction: Direction::Push,
                }),
                operand: Some(format!("{remote} {tag}")),
            },
        }
    }
}

impl OpShape<'_> {
    /// The operation's own failure: its name, what it was pointed at, and
    /// its outcome class.
    pub(super) fn fail(&self, detail: &str) -> CliError {
        let note = self.credentials_note(detail).unwrap_or("");
        self.phrase(|what| format!("git {what} failed: {detail}{note}"))
    }

    /// oakum empties the askpass chain so a credential prompt cannot hang a
    /// release, which makes a credential-starved remote child a state oakum
    /// caused; the note names the way out.
    fn credentials_note(&self, detail: &str) -> Option<&'static str> {
        self.contact?;
        let starved = detail.contains("terminal prompts disabled")
            || detail.contains("could not read Username")
            || detail.contains("could not read Password")
            || detail.contains("Authentication failed");
        starved.then_some(
            " (oakum disables git's credential prompts so a release cannot \
             hang on one; configure or refresh a git credential helper for \
             this remote — with the GitHub CLI, `gh auth setup-git` sets one \
             up)",
        )
    }

    /// Routed through [`OpShape::phrase`] like every other git failure, so it
    /// names the operation and its remote and takes the operation's own
    /// outcome class: a `push` that never ran is a plain failure, not a
    /// verification that could not look.
    ///
    /// States the cause. To skip an unreadable repository ssh config, set both
    /// `GIT_SSH_COMMAND` and `GIT_SSH_VARIANT` (they outrank config and skip
    /// that probe); otherwise repair the configuration. Oakum will not guess a
    /// transport.
    pub(super) fn unreadable_transport(&self, detail: &str) -> CliError {
        self.phrase(|what| {
            format!(
                "git {what} needs an ssh configuration oakum could not read \
                 ({detail}); to skip an unreadable repository ssh config, set \
                 both GIT_SSH_COMMAND and GIT_SSH_VARIANT — oakum will not \
                 guess a transport, because those variables outrank every other \
                 source and guessing would replace a key or proxy the user \
                 configured"
            )
        })
    }

    /// One place decides `unverified` versus a plain error, so a new message
    /// cannot pick the wrong one.
    pub(super) fn phrase(&self, message: impl FnOnce(&str) -> String) -> CliError {
        let what = match &self.operand {
            Some(operand) => format!("{} {operand}", self.name),
            None => self.name.to_owned(),
        };
        let message = message(&what);
        match self.spec.outcome {
            Outcome::Verification => CliError::unverified(format!("unverified: {message}")),
            Outcome::Action => CliError::new(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Answer::{Always, Never, Sometimes};
    use super::Outcome::{Action, Verification};
    use super::{fixture_commit, Answer, Contact, Direction, Op, Outcome};

    /// The remote an operation contacts and which way, which decides which URL
    /// the note is asked about. Only the two operations that reach a remote
    /// have one.
    #[test]
    fn only_the_remote_operations_contact_one() {
        assert_eq!(
            Op::AdvertisedTags { remote: "upstream" }.shape().contact,
            Some(Contact {
                remote: "upstream",
                direction: Direction::Fetch
            })
        );
        assert_eq!(
            Op::PushTag {
                remote: "upstream",
                tag: "v1.0.0"
            }
            .shape()
            .contact,
            Some(Contact {
                remote: "upstream",
                direction: Direction::Push
            })
        );
        // The operations that answer the reach question must contact nothing
        // themselves. Classed otherwise, asking one recurses into asking it
        // again — measured as a stack overflow, exit 134, with the unit suite
        // still green and only the integration suites failing.
        for op in [Op::RemoteUrl { remote: "origin" }, Op::RemoteUrls] {
            assert!(
                op.shape().contact.is_none(),
                "{op:?} answers the reach question and must not ask it"
            );
        }
    }

    /// The fake keys on these, and it matches them exactly, so the only thing
    /// keying needs from them is that no two collide. `rev-parse --verify` and
    /// `rev-parse --verify refs/tags` are the close pair — one is a character
    /// prefix of the other, which is what made an argv-prefix key need rules
    /// about how much of a command line counts.
    #[test]
    fn every_operation_has_its_own_name() {
        let every = every();
        let mut names: Vec<&str> = every.iter().map(|op| op.shape().name).collect();
        let listed = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            listed,
            "two operations share a name: {names:?}"
        );
    }

    /// The one measured case of git writing to stderr while answering
    /// correctly: a `core.fsmonitor` hook it cannot execute makes it fall back,
    /// print `fatal: cannot exec ...`, and exit 0. Overriding the setting on
    /// the child removes the diagnostic and leaves the answer byte-identical,
    /// which is what lets the rule above stay fail-closed.
    #[test]
    fn the_worktree_read_overrides_a_broken_fsmonitor_rather_than_tolerating_it() {
        let argv = Op::WorktreeStatus.shape().argv;
        assert_eq!(&argv[..2], ["-c", "core.fsmonitor=false"], "{argv:?}");
    }

    /// An operation whose argv contacts a remote while `contact` answers
    /// `None` skips the ssh note — the deadline rides every child regardless.
    /// Exhaustive matching forces an answer, not a right one.
    #[test]
    fn a_network_verb_and_a_contact_agree() {
        for op in every() {
            let shape = op.shape();
            let argv = shape.argv;
            assert_eq!(
                reaches_the_network(&argv),
                shape.contact.is_some(),
                "{op:?} runs `git {}` but disagrees about contacting a remote",
                argv.join(" ")
            );
        }
    }

    /// Read past any `-c <value>` pair: `Op::WorktreeStatus` already ships one,
    /// so a remote operation acquiring one is the established habit here, and
    /// reading `argv[0]` alone would stop seeing the verb. `remote` needs its
    /// subcommand — `remote update` reaches the network where `remote get-url`
    /// does not.
    fn reaches_the_network(argv: &[String]) -> bool {
        let mut rest = argv.iter().map(String::as_str);
        let mut verb = rest.next().unwrap_or_default();
        while verb == "-c" {
            rest.next();
            verb = rest.next().unwrap_or_default();
        }
        match verb {
            "fetch" | "push" | "ls-remote" | "clone" | "pull" => true,
            "remote" => rest.next() == Some("update"),
            _ => false,
        }
    }

    /// The shapes no shipping operation has yet, so walking the operation
    /// table cannot reach them.
    #[test]
    fn the_network_check_reads_past_config_and_subcommands() {
        let argv = |args: &[&str]| {
            args.iter()
                .copied()
                .map(String::from)
                .collect::<Vec<String>>()
        };
        for reaching in [
            &["fetch", "--tags", "--", "origin"][..],
            &["-c", "protocol.version=2", "fetch", "origin"][..],
            &["-c", "a=b", "-c", "c=d", "push", "origin"][..],
            &["remote", "update", "origin"][..],
        ] {
            assert!(
                reaches_the_network(&argv(reaching)),
                "`git {}` reaches the network",
                reaching.join(" ")
            );
        }
        for local in [
            &["remote", "get-url", "--", "origin"][..],
            &["remote"][..],
            &["-c", "core.fsmonitor=false", "status", "--porcelain"][..],
            &["-c", "a=b"][..],
            &[][..],
        ] {
            assert!(
                !reaches_the_network(&argv(local)),
                "`git {}` does not",
                local.join(" ")
            );
        }
    }

    /// `a_network_verb_and_a_contact_agree` and `every_operation_states_every_axis`
    /// both walk the operation table, so an operation missing from it is invisible to
    /// the tests written to catch it — measured: a remote variant listed in
    /// neither table passed the whole suite.
    #[test]
    fn every_variant_is_listed_in_every() {
        let source = include_str!("op.rs");
        // Two pieces, so the needle does not appear in this line at all.
        // Whole, it would match here too, and the search would depend on the
        // declaration staying above this test in the file.
        let opener = concat!("pub(in crate::cli) ", "enum Op<'a> {");
        let body = source
            .split_once(opener)
            .expect("the Op enum")
            .1
            .split_once("\n}\n")
            .expect("the end of the Op enum")
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
            every().len(),
            "`Op` declares {declared} variants and the operation table lists {}",
            every().len()
        );
    }

    /// How many operations `Op` declares. The table below states one row for
    /// each, so a new variant cannot compile into the table without its axes,
    /// and `every_variant_is_listed_in_every` ties the count back to the enum.
    const OPERATIONS: usize = 25;

    /// Every operation with the axes that describe it, stated rather than
    /// sampled. One table: an operation names its own class instead of
    /// matching a second list by position.
    #[expect(
        clippy::too_many_lines,
        reason = "a table, one row per operation; it grows with the enum"
    )]
    fn operations() -> [(Op<'static>, Outcome, Option<Direction>, Answer, bool); OPERATIONS] {
        [
            (Op::ReachableTags, Verification, None, Sometimes, false),
            (Op::AllTags, Verification, None, Sometimes, false),
            (Op::IsShallow, Verification, None, Always, false),
            (Op::TagOptRemotes, Verification, None, Always, false),
            (Op::RemoteNames, Verification, None, Sometimes, false),
            (
                Op::AdvertisedTags { remote: "origin" },
                Verification,
                Some(Direction::Fetch),
                Sometimes,
                false,
            ),
            (
                Op::ChangedPaths { from: "v1.0.0" },
                Verification,
                None,
                Sometimes,
                false,
            ),
            (Op::Head, Action, None, Always, false),
            (
                Op::RemoteUrl { remote: "origin" },
                Action,
                None,
                Always,
                false,
            ),
            (Op::RemoteUrls, Action, None, Sometimes, false),
            (Op::MergeBase { tip: "main" }, Action, None, Always, false),
            (
                Op::Commits { from: "v1.0.0" },
                Action,
                None,
                Sometimes,
                true,
            ),
            (
                Op::CommitPaths { hash: "cafebabe" },
                Action,
                None,
                Sometimes,
                false,
            ),
            (
                Op::CommitParents { hash: "cafebabe" },
                Action,
                None,
                Always,
                false,
            ),
            (
                Op::LocalTagCommit { tag: "v1.0.0" },
                Action,
                None,
                Always,
                false,
            ),
            (Op::WorktreeStatus, Action, None, Sometimes, false),
            (
                Op::CommitMessage { commit: "HEAD" },
                Action,
                None,
                Sometimes,
                true,
            ),
            (
                Op::RefExists {
                    reference: "refs/tags/v1.0.0",
                },
                Action,
                None,
                Always,
                false,
            ),
            (
                Op::ValidRefName {
                    reference: "v1.0.0",
                },
                Action,
                None,
                Never,
                false,
            ),
            (
                Op::WorkflowTree {
                    commit: fixture_commit(),
                },
                Verification,
                None,
                Sometimes,
                false,
            ),
            (
                Op::BlobText {
                    commit: fixture_commit(),
                    path: ".github/workflows/release.yml",
                },
                Verification,
                None,
                Sometimes,
                false,
            ),
            (
                Op::TreeEntry {
                    commit: fixture_commit(),
                    path: "CHANGELOG.md",
                },
                Verification,
                None,
                Sometimes,
                false,
            ),
            (
                Op::FileAddedBy {
                    path: ".changeset/one.md",
                },
                Verification,
                None,
                Sometimes,
                false,
            ),
            (
                Op::AnnotatedTag {
                    name: "v1.0.0",
                    commit: fixture_commit(),
                },
                Action,
                None,
                Never,
                false,
            ),
            (
                Op::PushTag {
                    remote: "origin",
                    tag: "v1.0.0",
                },
                Action,
                Some(Direction::Push),
                Never,
                false,
            ),
        ]
    }

    /// One of each variant, for the tests that ask something of every
    /// operation without caring about its axes.
    fn every() -> [Op<'static>; OPERATIONS] {
        operations().map(|(op, ..)| op)
    }

    #[test]
    fn every_operation_states_every_axis() {
        for (op, outcome, contacts, answer, lossy) in operations() {
            let shape = op.shape();
            assert_eq!(shape.spec.outcome, outcome, "{op:?} outcome");
            assert_eq!(
                shape.contact.map(|contact| contact.direction),
                contacts,
                "{op:?} contacts"
            );
            assert_eq!(shape.spec.answer, answer, "{op:?} answer");
            assert_eq!(shape.spec.lossy, lossy, "{op:?} lossy");
        }
    }

    /// A failure has to say which remote or ref it was about; the subcommand
    /// alone cannot distinguish two configured remotes.
    #[test]
    fn a_failure_names_what_it_was_pointed_at() {
        assert_eq!(
            Op::AdvertisedTags { remote: "upstream" }
                .shape()
                .operand
                .as_deref(),
            Some("upstream")
        );
        assert_eq!(
            Op::ChangedPaths { from: "v1.0.0" }
                .shape()
                .operand
                .as_deref(),
            Some("v1.0.0...HEAD")
        );
        assert_eq!(Op::ReachableTags.shape().operand, None);
    }
}
