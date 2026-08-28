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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

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

/// Which way a remote operation talks to its remote. A push and a fetch can go
/// to different places, so an operation judged by the wrong URL gets a note
/// naming a transport it never uses. Whether a remote is contacted at all is
/// the `Option` around this, decided by the same match in [`Op::contact`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Direction {
    Fetch,
    Push,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Contact<'a> {
    remote: &'a str,
    direction: Direction,
}

impl Contact<'_> {
    /// Keyed by both: a remote can fetch over one transport and push over
    /// another, so by name alone the fetch note swallows the push one.
    fn key(self) -> (String, Direction) {
        (self.remote.to_owned(), self.direction)
    }
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

/// What the runner needs about an operation beyond its remote, which
/// [`Op::contact`] carries.
struct Spec {
    outcome: Outcome,
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
const SKIP_CHECKS_ATOM: &str = "%(trailers:key=skip-checks,valueonly,unfold)";

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
    /// operation. Hand-written, so a variant missing from both this and `AXES`
    /// would go unstated; `every_variant_is_listed_in_every` counts `Op`'s
    /// declarations to close that.
    #[cfg(test)]
    const EVERY: [Self; 20] = [
        Self::ReachableTags,
        Self::IsShallow,
        Self::TagOptRemotes,
        Self::RemoteNames,
        Self::AdvertisedTags { remote: "origin" },
        Self::ChangedPaths { from: "v1.0.0" },
        Self::Head,
        Self::RemoteUrl { remote: "origin" },
        Self::RemoteUrls,
        Self::MergeBase { tip: "main" },
        Self::Commits { from: "v1.0.0" },
        Self::CommitPaths { hash: "cafebabe" },
        Self::CommitParents { hash: "cafebabe" },
        Self::LocalTagCommit { tag: "v1.0.0" },
        Self::WorktreeStatus,
        Self::CommitMessage { commit: "HEAD" },
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
            Self::RemoteUrls => owned(&["remote", "-v"]),
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
            Self::CommitMessage { commit } => vec![
                String::from("log"),
                String::from("-1"),
                format!("--format=%B%x00{SKIP_CHECKS_ATOM}"),
                String::from(*commit),
            ],
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
            Self::AdvertisedTags { .. } => Spec::LOOK,
            Self::ChangedPaths { .. } => Spec::LOOK,
            Self::Head => Spec::ANSWERING_ACT,
            Self::RemoteUrl { .. } => Spec::ANSWERING_ACT,
            Self::RemoteUrls => Spec::ACT,
            Self::MergeBase { .. } => Spec::ANSWERING_ACT,
            Self::Commits { .. } => Spec::LOSSY_ACT,
            Self::CommitPaths { .. } => Spec::ACT,
            Self::CommitParents { .. } => Spec::ANSWERING_ACT,
            Self::LocalTagCommit { .. } => Spec::ANSWERING_ACT,
            Self::WorktreeStatus => Spec::ACT,
            Self::CommitMessage { .. } => Spec::LOSSY_ACT,
            Self::RefExists { .. } => Spec::ANSWERING_ACT,
            Self::ValidRefName { .. } => Spec::PERFORM,
            Self::AnnotatedTag { .. } => Spec::PERFORM,
            Self::PushTag { .. } => Spec::PERFORM,
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
            Self::RemoteUrls => "remote -v",
            Self::MergeBase { .. } => "merge-base",
            Self::Commits { .. } => "log",
            Self::CommitPaths { .. } => "diff-tree",
            Self::CommitParents { .. } => "rev-list --parents",
            Self::LocalTagCommit { .. } => "rev-parse --verify refs/tags",
            Self::WorktreeStatus => "status --porcelain",
            Self::CommitMessage { .. } => "log -1",
            Self::RefExists { .. } => "rev-parse --verify",
            Self::ValidRefName { .. } => "check-ref-format",
            Self::AnnotatedTag { .. } => "tag",
            Self::PushTag { .. } => "push",
        }
    }

    /// The remote this operation contacts and which way. Listed rather than
    /// matched with a wildcard: an operation added to the remote set and
    /// forgotten here would spawn a child with no `BatchMode` and hang on a
    /// prompt.
    const fn contact(&self) -> Option<Contact<'_>> {
        match self {
            Self::AdvertisedTags { remote } => Some(Contact {
                remote,
                direction: Direction::Fetch,
            }),
            Self::PushTag { remote, .. } => Some(Contact {
                remote,
                direction: Direction::Push,
            }),
            Self::ReachableTags
            | Self::IsShallow
            | Self::TagOptRemotes
            | Self::RemoteNames
            | Self::ChangedPaths { .. }
            | Self::Head
            | Self::RemoteUrl { .. }
            | Self::RemoteUrls
            | Self::MergeBase { .. }
            | Self::Commits { .. }
            | Self::CommitPaths { .. }
            | Self::CommitParents { .. }
            | Self::LocalTagCommit { .. }
            | Self::WorktreeStatus
            | Self::CommitMessage { .. }
            | Self::RefExists { .. }
            | Self::ValidRefName { .. }
            | Self::AnnotatedTag { .. } => None,
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
            Self::CommitMessage { commit } => owned(commit),
            Self::RefExists { reference } | Self::ValidRefName { reference } => owned(reference),
            Self::AnnotatedTag { name, .. } => owned(name),
            Self::PushTag { remote, tag } => Some(format!("{remote} {tag}")),
            Self::ReachableTags
            | Self::IsShallow
            | Self::TagOptRemotes
            | Self::RemoteNames
            | Self::RemoteUrls
            | Self::Head
            | Self::WorktreeStatus => None,
        }
    }
}

/// What oakum established about a remote's URL set. Three independent facts
/// rather than one verdict: a push contacts every URL listed, so one set can
/// reach ssh and run a helper at once, and collapsing the facts into a single
/// answer is what lost the helper note for exactly that mix.
#[derive(Clone, Debug)]
struct Reach {
    ssh: bool,
    /// The remote runs a helper command — `<transport>::<address>`, or a URL
    /// scheme git does not implement natively, which falls through to
    /// `git-remote-<scheme>`. Measured: the helper inherits `GIT_SSH_COMMAND`
    /// and applies none of it, so `BatchMode` never reaches whatever it runs.
    helper: bool,
    /// The listing could not be read. The note still prints under an
    /// unprotected transport — it is advisory, and withholding it because the
    /// check failed is the quieter wrong answer — and an unread URL is never
    /// evidence of a safe transport.
    unread: Option<CliError>,
}

/// `remote -v` lines folded into per-remote, per-direction URL lists. Each
/// line's name half is matched against the configured remote names — a name
/// can itself contain a tab (config-made), so splitting on the first tab
/// could hand one remote's URL to another. The URL is everything up to the
/// trailing direction marker, so a URL containing whitespace survives. A
/// remote with no URL for a direction renders as a bare `name\t` line
/// (measured on a pushurl-only remote) and is simply absent; any other
/// unrecognized shape fails the whole parse, because a listing oakum cannot
/// read is a look that did not happen, not an empty answer.
type RemoteUrlList = Vec<((String, Direction), String)>;

fn parse_remote_urls(listing: &str, names: &[&str]) -> Result<RemoteUrlList, CliError> {
    let mut folded: RemoteUrlList = Vec::new();
    for line in listing.lines() {
        let name = names
            .iter()
            .filter(|name| {
                line.strip_prefix(**name)
                    .is_some_and(|rest| rest.starts_with('\t'))
            })
            .max_by_key(|name| name.len());
        let Some(&name) = name else {
            return Err(CliError::new(format!(
                "unparseable `git remote -v` line {line:?}"
            )));
        };
        let rest = &line[name.len() + 1..];
        if rest.trim().is_empty() {
            continue;
        }
        let parsed = if let Some(url) = rest.strip_suffix(" (fetch)") {
            Some((url, Direction::Fetch))
        } else {
            rest.strip_suffix(" (push)")
                .map(|url| (url, Direction::Push))
        };
        let Some((url, direction)) = parsed else {
            return Err(CliError::new(format!(
                "unparseable `git remote -v` line {line:?}"
            )));
        };
        let key = (name.to_owned(), direction);
        match folded.iter_mut().find(|(entry, _)| *entry == key) {
            Some((_, urls)) => {
                urls.push('\n');
                urls.push_str(url);
            }
            None => folded.push((key, url.to_owned())),
        }
    }
    Ok(folded)
}

fn classify(urls: Result<&str, &CliError>) -> Reach {
    match urls {
        // Any of them: a push reaches every URL listed, so one over ssh — or
        // one selecting a helper — is enough to make its note apply.
        Ok(urls) => Reach {
            ssh: urls.lines().any(reaches_over_ssh),
            helper: urls.lines().any(runs_a_helper),
            unread: None,
        },
        Err(err) => {
            // The advisory note is not rendering a verification verdict, so
            // the embedded error sheds its outcome token.
            let detail = err.to_string();
            let detail = detail.strip_prefix("unverified: ").unwrap_or(&detail);
            Reach {
                ssh: false,
                helper: false,
                unread: Some(CliError::new(format!(
                    "oakum could not read that remote's URL ({detail})"
                ))),
            }
        }
    }
}

/// Every note the remote's URL set owes, so independent hazards cannot
/// swallow each other: a helper speaks in both transport columns, because
/// `BatchMode` never reaches what a helper runs; ssh and an unread listing
/// speak only when the transport could not be protected.
///
/// Separate from saying them, so the text is assertable without a real
/// child's stderr.
fn notes_for(contact: Contact<'_>, reach: &Reach, batch: &env::BatchSsh) -> Vec<String> {
    let remote = contact.remote;
    let reason = batch.unprotected_reason();
    let mut notes = Vec::new();
    if reach.helper {
        let mut note = format!(
            "the remote {remote:?} runs a helper command oakum cannot inspect; \
             its ssh `BatchMode` does not reach whatever that helper runs, so \
             a prompt can still block."
        );
        if let Some(reason) = reason {
            use std::fmt::Write;
            let _ = write!(
                note,
                " The ssh transport could not be protected either: {reason}."
            );
        }
        notes.push(note);
    }
    if let Some(why) = &reach.unread {
        let mut note = format!(
            "oakum could not establish what transport {remote:?} uses ({why}); \
             if the remote runs a helper, a prompt may still block."
        );
        if let Some(reason) = reason {
            use std::fmt::Write;
            let _ = write!(
                note,
                " The ssh transport could not be protected either: {reason}."
            );
        }
        notes.push(note);
    }
    if let Some(reason) = reason {
        if reach.ssh {
            notes.push(format!(
                "oakum cannot refuse ssh prompts for the transport {remote:?} \
                 uses: {reason}. A prompt can still block."
            ));
        }
    }
    notes
}

/// Whether git would reach this remote over ssh.
///
/// The note about a transport that cannot take `BatchMode` is only about ssh,
/// but the transport resolves from the environment before any remote URL is
/// known. Gating on the transport alone prints it for `https://` and `file://`
/// remotes, where ssh is never invoked and no prompt it describes can occur.
fn reaches_over_ssh(url: &str) -> bool {
    if let Some((scheme, _)) = url.split_once("://") {
        // Matched exactly, as git matches its own table. Measured with
        // `GIT_TRACE=1 GIT_SSH_COMMAND=<marker> git ls-remote`: `git+ssh://` and
        // `ssh+git://` do reach ssh, and `SSH://` does not — it falls through to
        // a `git-remote-SSH` helper.
        return matches!(scheme, "ssh" | "git+ssh" | "ssh+git");
    }
    let Some(at) = scp_separator(url) else {
        return false;
    };
    // `<transport>::<address>` names a remote helper: measured, `git ls-remote
    // -- a::b` reports `remote helper 'a'` and never runs ssh. An empty host is
    // still ssh, though — `:oakum.git` dials one.
    if url[at..].starts_with("::") {
        return false;
    }
    !dos_drive(&url[..at])
}

/// Whether a remote runs a helper command: `<transport>::<address>`, or a URL
/// scheme git does not implement natively, which selects `git-remote-<scheme>`
/// exactly as the `::` form does — measured, `marker://addr` and
/// `marker::addr` run the same helper with the same uninspectable transport.
/// The native list is exact and case-sensitive, as git matches its own table:
/// `SSH://` reaches `git-remote-SSH`, not ssh.
fn runs_a_helper(url: &str) -> bool {
    if names_a_helper(url) {
        return true;
    }
    match url.split_once("://") {
        Some((scheme, _)) => !matches!(
            scheme,
            "ssh" | "git+ssh" | "ssh+git" | "http" | "https" | "git" | "file" | "ftp" | "ftps"
        ),
        None => false,
    }
}

/// Whether a remote names a `<transport>::<address>` helper, whose transport is
/// whatever command that helper runs.
fn names_a_helper(url: &str) -> bool {
    scp_separator(url).is_some_and(|at| url[at..].starts_with("::"))
}

/// The colon separating host from path in an scp-like remote, if there is one.
///
/// A bracketed IPv6 literal carries colons of its own and can sit after
/// userinfo, so `user@[::1]:repo.git` is separated by the colon *after* the
/// bracket — measured, git dials that over ssh. A colon reached after a slash
/// belongs to the path.
fn scp_separator(url: &str) -> Option<usize> {
    let mut bracketed = false;
    for (at, character) in url.char_indices() {
        match character {
            // Only a bracket that closes opens a literal. Unmatched, it is an
            // ordinary character in a hostname: measured, `git ls-remote --
            // 'foo[bar:baz'` dials ssh with host `foo[bar`.
            '[' if url[at..].contains(']') => bracketed = true,
            ']' => bracketed = false,
            ':' if !bracketed => return Some(at),
            '/' => return None,
            _ => {}
        }
    }
    None
}

/// Whether a one-letter prefix is a drive rather than a hostname, which is true
/// only on Windows: measured here, `x:r.git` and `C:\repos\oakum` both reach git
/// over ssh, so a single-letter prefix is a legitimate host off Windows. The
/// Windows side follows git's DOS-drive handling and is inferred — this platform
/// cannot exercise it, and no test covers that branch.
fn dos_drive(host: &str) -> bool {
    cfg!(windows) && host.len() == 1 && host.starts_with(|first: char| first.is_ascii_alphabetic())
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

    /// The status first, then git's own words when it wrote any: `git push`
    /// writes its success banner to stderr before a signal can kill it, so a
    /// diagnostic alone renders a signal death as that banner (measured).
    fn detail(&self) -> String {
        let status = match self.code {
            Some(code) => format!("exit {code}"),
            None => String::from("terminated by a signal"),
        };
        match self.diagnostic() {
            Some(said) => format!("{status}: {said}"),
            None => format!("{status} with no diagnostic"),
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

/// A commit's message and git's parse of its `skip-checks` trailer values,
/// one value per line.
#[derive(Debug)]
pub(super) struct CommitText {
    pub(super) message: String,
    pub(super) skip_checks: String,
}

/// Runs git in one repository.
pub(super) struct Git {
    repo: PathBuf,
    runner: Runner,
    /// Resolved on the first child and reused. The answer comes from the
    /// process environment and the repository config, neither of which changes
    /// while oakum runs, so resolving it per child costs a `git config` spawn
    /// each time.
    ///
    /// The failure is cached too, and travels as the bare reason so the caller
    /// phrases it: an operation that needed the transport turns it into an
    /// `unverified` error, one that did not says it plainly. Pre-wrapped, both
    /// phrasings land in the same line and contradict each other.
    transport: OnceLock<std::sync::Arc<Result<env::BatchSsh, String>>>,
    /// What each named remote's listed URLs established, per direction, so
    /// the notes are asked about the remote in hand.
    remote_ssh: Mutex<BTreeMap<(String, Direction), Reach>>,
    /// Notes already said, keyed by their text: distinct fetch and push notes
    /// each land, while a byte-identical one says itself once.
    warned: Mutex<BTreeSet<String>>,
}

impl Git {
    pub(super) fn at(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            runner: Runner::Child,
            transport: OnceLock::new(),
            remote_ssh: Mutex::new(BTreeMap::new()),
            warned: Mutex::new(BTreeSet::new()),
        }
    }

    /// Answers from a script instead of a repository, each keyed by the command
    /// it answers. The path is never read.
    #[cfg(test)]
    pub(super) fn answering(replies: impl IntoIterator<Item = (&'static str, Reply)>) -> Self {
        Self {
            repo: PathBuf::new(),
            runner: Runner::Fake(fake::Fake::answering(replies)),
            transport: OnceLock::new(),
            remote_ssh: Mutex::new(BTreeMap::new()),
            warned: Mutex::new(BTreeSet::new()),
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

    /// One commit's message and its `skip-checks` trailer values, split
    /// where [`Op::CommitMessage`]'s format puts the separator. Parsed here,
    /// beside the format that creates the framing, so no caller re-inherits
    /// the contract — and a reply that lacks the separator, or still carries
    /// the unexpanded specifier (a git too old to parse trailers), is an
    /// error rather than a silent "no trailers": guessing there would release
    /// a commit whose workflow GitHub suppresses.
    pub(super) fn commit_text(&self, commit: &str) -> Result<CommitText, CliError> {
        let raw = self.text(Op::CommitMessage { commit })?;
        let Some((message, trailers)) = raw.split_once('\0') else {
            return Err(CliError::new(format!(
                "git log for `{commit}` omitted the trailer separator oakum's \
                 format requests"
            )));
        };
        if trailers.trim() == SKIP_CHECKS_ATOM && !message.contains(SKIP_CHECKS_ATOM) {
            return Err(CliError::new(format!(
                "this git did not parse the skip-checks trailers for \
                 `{commit}`; oakum cannot tell whether the commit suppresses \
                 CI"
            )));
        }
        Ok(CommitText {
            message: message.to_owned(),
            skip_checks: trailers.to_owned(),
        })
    }

    /// The ssh transport for this repository, resolved once per process: it
    /// is a property of the environment and the repository config, neither of
    /// which changes while oakum runs, and `Git` values are constructed
    /// throughout the cli — per-instance caching re-probed on every one now
    /// that every child carries the transport.
    fn transport(&self) -> Result<&env::BatchSsh, &str> {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex, OnceLock};
        type Resolved = Mutex<HashMap<std::path::PathBuf, Arc<Result<env::BatchSsh, String>>>>;
        static RESOLVED: OnceLock<Resolved> = OnceLock::new();
        self.transport
            .get_or_init(|| {
                let mut resolved = RESOLVED
                    .get_or_init(|| Mutex::new(HashMap::new()))
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                resolved
                    .entry(self.repo.clone())
                    .or_insert_with(|| Arc::new(env::batch_transport(&self.repo)))
                    .clone()
            })
            .as_ref()
            .as_ref()
            .map_err(String::as_str)
    }

    /// Each distinct note lands once. The transport resolves the same way
    /// for every child, so without this an N-tag release repeats it 1 + 2N
    /// times.
    fn say_once(&self, note: &str) {
        self.say_once_with(note, env::warn);
    }

    /// Takes the sayer, because the rollback below is otherwise the one branch
    /// no test can drive: `say_once` reaches stderr through `env::warn`, and a
    /// refused write there is not reproducible in process.
    ///
    /// The lock spans the write: released first, a second caller skips on a
    /// reservation about to be rolled back and both stay silent. `warn`
    /// re-enters nothing, so holding it cannot deadlock the way it would around
    /// a spawn, and with `Git` driven from one thread it orders a race that
    /// cannot yet happen.
    fn say_once_with(&self, note: &str, say: impl FnOnce(&str) -> bool) -> bool {
        let key = note.to_owned();
        let mut warned = self
            .warned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !warned.insert(key.clone()) {
            return false;
        }
        // A stderr that could not take the line must not consume the one chance
        // to say it. A write that failed partway still leaves its fragment, and
        // the retry then says the whole note again; a fragment plus the note
        // beats losing the note.
        let said = say(note);
        if !said {
            warned.remove(&key);
        }
        said
    }

    /// Whether the note about ssh prompts applies to the remote this operation
    /// contacts. One `remote -v` child lists every remote's fetch and push
    /// URLs — pushurl included, so a remote that fetches over https and
    /// pushes over ssh still warns on the push — and fills the cache for all
    /// of them, so the unconditional read costs one spawn per run rather than
    /// one per operation. Neither URL changes while oakum runs.
    ///
    /// A failed read is cached alongside a settled one, which is safe only
    /// while the sole consumer is an advisory note — an unread verdict from a
    /// transient signal is cached exactly like an established one.
    fn remote_reach(&self, contact: Contact<'_>) -> Reach {
        let key = contact.key();
        if let Some(answer) = self.remembered_reach(&key) {
            return answer;
        }
        // The listing child spawns before the lock is taken: a `Mutex` is not
        // reentrant, so holding it across the spawn deadlocks outright — no
        // error, no output — if the listing operation is ever itself classed
        // as contacting a remote. Two callers racing here duplicate one cheap
        // read rather than hanging.
        let parsed = self.text(Op::RemoteNames).and_then(|names| {
            let names: Vec<&str> = names.lines().collect();
            let listing = self.text(Op::RemoteUrls)?;
            parse_remote_urls(&listing, &names)
        });
        let mut cache = self
            .remote_ssh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let answer = match parsed {
            Ok(urls) => {
                for (entry, urls) in urls {
                    cache.insert(entry, classify(Ok(&urls)));
                }
                cache.get(&key).cloned().unwrap_or_else(|| {
                    let direction = match contact.direction {
                        Direction::Fetch => "fetch",
                        Direction::Push => "push",
                    };
                    classify(Err(&CliError::new(format!(
                        "the listing shows no {direction} URL for remote {:?}",
                        contact.remote
                    ))))
                })
            }
            Err(err) => classify(Err(&err)),
        };
        cache.insert(key, answer.clone());
        answer
    }

    /// A cache, so a torn entry is not a correctness problem: a poisoned lock is
    /// recovered rather than turned into a second panic that hides the first.
    fn remembered_reach(&self, key: &(String, Direction)) -> Option<Reach> {
        self.remote_ssh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    fn child(&self, op: Op<'_>, args: &[&str]) -> Result<Reply, CliError> {
        // Every child carries the transport: git opens sockets on its own
        // schedule — a partial clone's `diff` lazily fetches over ssh from an
        // op typed local (measured) — so protection cannot key on the
        // classification, and an unreadable ssh configuration stops every
        // operation rather than guessing away the user's key or proxy.
        let batch = self
            .transport()
            .map_err(|detail| Self::unreadable_transport(op, detail))?;
        if let Some(contact) = op.contact() {
            // Unconditional: a helper remote owes its note even when the
            // transport composed, because `BatchMode` never reaches what
            // a helper runs.
            let reach = self.remote_reach(contact);
            for note in notes_for(contact, &reach, batch) {
                self.say_once(&note);
            }
        }
        let started = env::deadlined_command(&self.repo, args, batch)
            .output()
            .map_err(|failure| match failure {
                // The OS reason separates a missing binary from a
                // permission problem from a fork failure, and a
                // support case needs the difference.
                env::RemoteFailure::Spawn(err) => {
                    Self::fail(op, &format!("could not run git: {err}"))
                }
                env::RemoteFailure::BadDeadline(reason) => Self::fail(op, &reason),
                env::RemoteFailure::Wait(err) => Self::fail(
                    op,
                    &format!("git started but waiting on it failed ({err}); oakum killed it"),
                ),
                env::RemoteFailure::Read(err) => Self::fail(
                    op,
                    &format!("git ran but its output could not be read: {err}"),
                ),
                env::RemoteFailure::DrainStalled { limit, status } => Self::fail(
                    op,
                    &format!(
                        "git exited ({status}) but something it spawned still held its \
                         output open {}s later, so the answer could not be collected; a \
                         credential helper or an ssh control master is the likely cause. \
                         Set OAKUM_REMOTE_DEADLINE (seconds) to wait longer",
                        limit.as_secs()
                    ),
                ),
                env::RemoteFailure::Deadline { limit } => Self::fail(
                    op,
                    &format!(
                        "gave up after {}s with no answer and killed the \
                         child; a credential helper, an interactive \
                         ProxyCommand, or a signing program that prompts can \
                         block past every prompt oakum \
                         suppresses. Set OAKUM_REMOTE_DEADLINE (seconds) \
                         if this remote is legitimately slow",
                        limit.as_secs()
                    ),
                ),
            })?;
        Ok(Reply::from(started))
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
        let note = Self::credentials_note(op, detail).unwrap_or("");
        Self::phrase(op, |what| format!("git {what} failed: {detail}{note}"))
    }

    /// oakum empties the askpass chain so a credential prompt cannot hang a
    /// release, which makes a credential-starved remote child a state oakum
    /// caused; the note names the way out.
    fn credentials_note(op: Op<'_>, detail: &str) -> Option<&'static str> {
        op.contact()?;
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

    /// One place decides `unverified` versus a plain error, so a new message
    /// cannot pick the wrong one.
    /// Routed through [`Self::phrase`] like every other git failure, so it names
    /// the operation and its remote and takes the operation's own outcome
    /// class: a `push` that never ran is a plain failure, not a verification
    /// that could not look.
    ///
    /// States the cause and prescribes nothing. Setting `GIT_SSH_COMMAND` reads
    /// like the fix and is not one — `transport` probes the config before it
    /// consults the environment, so the failure propagates either way. Fixing
    /// that ordering is okm-7za.7.
    fn unreadable_transport(op: Op<'_>, detail: &str) -> CliError {
        Self::phrase(op, |what| {
            format!(
                "git {what} needs an ssh configuration oakum could not read \
                 ({detail}); it will not guess a transport, because \
                 GIT_SSH_COMMAND outranks every other source and guessing would \
                 replace a key or proxy the user configured"
            )
        })
    }

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
    use super::Direction;
    use super::Outcome::{Action, Verification};
    use super::{split_nul_paths, Answer, CliError, Contact, Git, Op, Outcome, Reach, Reply};

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

    /// A cached failure is handed to later callers unchanged, and the class is
    /// decided where it is needed rather than carried: an operation that wanted
    /// the transport reports `unverified`, one that did not says it plainly.
    #[test]
    fn a_cached_transport_failure_is_repeated_verbatim() {
        let git = Git::at("/nonexistent");
        git.transport
            .set(Err(String::from("git config was killed by a signal")).into())
            .expect("the cache starts empty");
        for _ in 0..3 {
            assert_eq!(
                git.transport().expect_err("a cached failure"),
                "git config was killed by a signal"
            );
        }
        let raised = Git::unreadable_transport(
            Op::AdvertisedTags { remote: "origin" },
            "git config was killed by a signal",
        );
        assert!(matches!(raised, CliError::Unverified { .. }), "{raised:?}");
        assert!(
            raised.to_string().contains("killed by a signal"),
            "{raised}"
        );
    }

    /// Which remotes the ssh-prompt note applies to: not `https://` or
    /// `file://`, where ssh is never invoked and the prompt it warns about
    /// cannot happen.
    ///
    /// Every case was measured rather than reasoned about, with `GIT_TRACE=1
    /// GIT_SSH_COMMAND=<marker> git ls-remote <url>` against git 2.55.
    #[test]
    fn only_an_ssh_remote_can_stop_at_an_ssh_prompt() {
        for url in [
            "ssh://git@github.com/oakoss/oakum.git",
            // Aliases git accepts for the same transport.
            "git+ssh://git@github.com/oakoss/oakum.git",
            "ssh+git://git@github.com/oakoss/oakum.git",
            "git@github.com:oakoss/oakum.git",
            "github.com:oakoss/oakum.git",
            // A single-letter host is a host, not a drive.
            "x:oakum.git",
        ] {
            assert!(super::reaches_over_ssh(url), "{url} reaches git over ssh");
        }
        for url in [
            // Git's scheme table is case-sensitive: this reaches a
            // `git-remote-SSH` helper, not ssh.
            "SSH://git@github.com/oakoss/oakum.git",
            "https://github.com/oakoss/oakum.git",
            "http://github.com/oakoss/oakum.git",
            "git://github.com/oakoss/oakum.git",
            "file:///srv/mirrors/oakum.git",
            "/srv/mirrors/oakum.git",
            "../sibling.git",
            // A colon after a slash belongs to the path.
            "./odd:name.git",
        ] {
            assert!(!super::reaches_over_ssh(url), "{url} does not use ssh");
        }
    }

    /// Remote-helper syntax and an empty host, both measured against git 2.55:
    /// `git ls-remote -- a::b` reports `remote helper 'a'` and never runs ssh,
    /// while `:oakum.git` does dial one, so an empty host is a remote that can
    /// block.
    #[test]
    fn helper_syntax_is_not_ssh_but_an_empty_host_is() {
        for url in [
            ":oakum.git",
            "[::1]:demo.git",
            // An unmatched bracket is part of the host, not a literal.
            "foo[bar:baz",
            "a[b:c",
            // The literal can sit after userinfo, where a bracket check anchored
            // at the start does not see it.
            "user@[::1]:repo.git",
            "git@[2001:db8::1]:oakum.git",
        ] {
            assert!(super::reaches_over_ssh(url), "{url} reaches git over ssh");
        }
        for url in ["a::b", "::a"] {
            assert!(!super::reaches_over_ssh(url), "{url} names a remote helper");
        }
    }

    /// A `<helper>::<address>` remote runs a command oakum cannot inspect, and
    /// an `ext::` one can invoke ssh itself without `BatchMode` — measured, one
    /// did, because a helper inherits `GIT_SSH_COMMAND` and applies none of it.
    /// Not ssh, but not established either, so it must not read as safe.
    #[test]
    fn a_helper_remote_is_unestablished_rather_than_not_ssh() {
        for url in ["a::b", "::a", "ext::ssh -p 22 git@h git-upload-pack r.git"] {
            assert!(super::names_a_helper(url), "{url} names a helper");
            assert!(
                !super::reaches_over_ssh(url),
                "{url} is not itself an ssh URL"
            );
        }
        for url in [
            "git@github.com:oakoss/oakum.git",
            "https://github.com/oakoss/oakum.git",
            "ssh://git@github.com/oakoss/oakum.git",
            "/srv/mirrors/oakum.git",
        ] {
            assert!(!super::names_a_helper(url), "{url} names no helper");
        }
    }

    /// A drive letter is a path on Windows and a hostname everywhere else.
    /// Measured here: `git ls-remote 'C:\repos\oakum'` invokes ssh.
    #[cfg(not(windows))]
    #[test]
    fn a_drive_letter_is_a_hostname_off_windows() {
        assert!(super::reaches_over_ssh(r"C:\repos\oakum"));
        assert!(super::reaches_over_ssh("C:/repos/oakum"));
    }

    /// The remote an operation contacts and which way, which decides which URL
    /// the note is asked about. Only the two operations that reach a remote
    /// have one.
    #[test]
    fn only_the_remote_operations_contact_one() {
        assert_eq!(
            Op::AdvertisedTags { remote: "upstream" }.contact(),
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
            .contact(),
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
                op.contact().is_none(),
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
            ("log -1", Op::CommitMessage { commit: "HEAD" }),
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
        // `git push` writes its success banner to stderr before a signal can
        // kill it; the banner must not stand in for the death (measured — the
        // banner alone was the whole reported reason).
        let banner = Git::answering([(
            "push",
            Reply::exactly(None, b"", b"To /private/tmp/origin.git\n"),
        )])
        .run(Op::PushTag {
            remote: "origin",
            tag: "v1.0.0",
        })
        .expect_err("a signal death with stderr");
        assert!(
            banner.to_string().contains("terminated by a signal"),
            "{banner}"
        );
        assert!(
            banner.to_string().contains("To /private/tmp/origin.git"),
            "git's words stay as evidence: {banner}"
        );
    }

    /// `Spec::lossy` decides this, and both sides of it are worth pinning: a
    /// commit message survives a stray byte, an object name does not.
    #[test]
    fn only_a_lossy_read_accepts_bytes_that_are_not_utf8() {
        let message = Git::answering([("log -1", Reply::said(b"fix: caf\xff\n".to_vec()))])
            .text(Op::CommitMessage { commit: "HEAD" })
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

    /// oakum empties the askpass chain on every git child, so a credential
    /// failure there is a state oakum caused; the report must name the way
    /// out while keeping git's text as the evidence.
    #[test]
    fn a_credential_starved_remote_failure_names_the_fix() {
        // One case per matched phrasing, each carrying only its own pattern,
        // so dropping any single arm fails a distinct assertion.
        for starved in [
            "fatal: could not read Username for 'https://gitlab.com': prompts off",
            "fatal: could not read Password for 'https://gitlab.com': prompts off",
            "fatal: Authentication failed for 'https://gitlab.com/'",
            "fatal: Username not available: terminal prompts disabled",
        ] {
            let err = Git::answering([("ls-remote --tags", Reply::failed(128, starved))])
                .text(Op::AdvertisedTags { remote: "origin" })
                .expect_err("a starved remote read fails");
            let text = err.to_string();
            assert!(text.contains(starved), "{text}");
            assert!(text.contains("credential helper"), "{text}");
            assert!(text.contains("gh auth setup-git"), "{text}");
            assert!(text.starts_with("unverified:"), "{text}");
        }

        let err = Git::answering([(
            "ls-remote --tags",
            Reply::failed(128, "fatal: repository not found"),
        )])
        .text(Op::AdvertisedTags { remote: "origin" })
        .expect_err("an unrelated remote failure");
        assert!(!err.to_string().contains("credential helper"), "{err}");

        let starved =
            "fatal: could not read Username for 'https://gitlab.com': terminal prompts disabled";
        let err = Git::answering([("log -1", Reply::failed(128, starved))])
            .text(Op::CommitMessage { commit: "HEAD" })
            .expect_err("a local child never gets the note");
        assert!(!err.to_string().contains("credential helper"), "{err}");
    }

    /// A remote can fetch over https and push over ssh, so the direction picks
    /// its URLs from the listing. This is the only guard on that: the note's
    /// text is direction-agnostic, so no process-level test can tell a fetch
    /// note from a push one, and swapping the directions passes every
    /// integration suite.
    #[test]
    fn the_listing_keeps_fetch_and_push_urls_apart() {
        let names = ["origin", "s", "pushonly"];
        let listing = "origin\thttps://host/r.git (fetch)\n\
                       origin\tgit@host:r.git (push)\n\
                       origin\thelper::two (push)";
        let parsed = super::parse_remote_urls(listing, &names).expect("parses");
        let urls = |direction: Direction| {
            parsed
                .iter()
                .find(|((name, at), _)| name == "origin" && *at == direction)
                .map(|(_, urls)| urls.as_str())
                .expect("listed")
        };
        assert!(!super::classify(Ok(urls(Direction::Fetch))).ssh);
        assert!(super::classify(Ok(urls(Direction::Push))).ssh);

        let err = super::parse_remote_urls("origin\thttps://host/r.git (fetched)", &names)
            .expect_err("an unknown marker fails the parse");
        assert!(err.to_string().contains("unparseable"), "{err}");
        let err = super::parse_remote_urls("stranger\thttps://host/r.git (fetch)", &names)
            .expect_err("a name outside the configured set fails the parse");
        assert!(err.to_string().contains("unparseable"), "{err}");

        let spaced = super::parse_remote_urls("s\text::sh -c \"echo hi\" (fetch)", &names)
            .expect("whitespace in a URL survives");
        assert_eq!(spaced[0].1, "ext::sh -c \"echo hi\"");

        // A pushurl-only remote renders `name\t` for its missing direction
        // (measured); that direction is absent, and the other remotes' reach
        // must not be poisoned by it.
        let partial = super::parse_remote_urls(
            "pushonly\t\npushonly\tgit@host:r.git (push)\norigin\thttps://host/r.git (fetch)",
            &names,
        )
        .expect("a bare direction line is absence, not failure");
        assert_eq!(partial.len(), 2, "{partial:?}");
    }

    /// An operation whose argv contacts a remote while `contact` answers
    /// `None` skips the ssh note — the deadline rides every child regardless.
    /// Exhaustive matching forces an answer, not a right one.
    #[test]
    fn a_network_verb_and_a_contact_agree() {
        for op in Op::EVERY {
            let argv = op.argv();
            assert_eq!(
                reaches_the_network(&argv),
                op.contact().is_some(),
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

    /// The shapes no shipping operation has yet, so walking `Op::EVERY` cannot
    /// reach them.
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
    /// both walk `Op::EVERY`, so an operation missing from it is invisible to
    /// the tests written to catch it — measured: a remote variant listed in
    /// neither table passed the whole suite.
    #[test]
    fn every_variant_is_listed_in_every() {
        let source = include_str!("mod.rs");
        let body = source
            .split_once("pub(super) enum Op<'a> {")
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
            Op::EVERY.len(),
            "`Op` declares {declared} variants and `Op::EVERY` lists {}",
            Op::EVERY.len()
        );
    }

    /// An old git echoes the trailer atom verbatim; a genuine trailer value
    /// quoting the atom also reproduces it — the difference is that the
    /// genuine one carries the atom in the message half too.
    #[test]
    fn the_unparsed_trailer_guard_spares_a_value_that_quotes_the_atom() {
        let old_git = Git::answering([(
            "log -1",
            Reply::said(format!("chore: release\0{}\n", super::SKIP_CHECKS_ATOM)),
        )]);
        let err = old_git.commit_text("cafe").expect_err("old git refuses");
        assert!(err.to_string().contains("did not parse"), "{err}");

        let quoting = Git::answering([(
            "log -1",
            Reply::said(format!(
                "chore: x\n\nskip-checks: {atom}\0{atom}\n",
                atom = super::SKIP_CHECKS_ATOM
            )),
        )]);
        let text = quoting
            .commit_text("cafe")
            .expect("a quoting value is a value");
        assert_eq!(text.skip_checks.trim(), super::SKIP_CHECKS_ATOM);
    }

    /// A remote the listing does not name was never looked at, and "we didn't
    /// look" must not become "it's fine": the fallback is unread, not safe.
    #[test]
    fn an_unlisted_remote_is_unread_not_safe() {
        let git = Git::answering([
            ("remote", Reply::said("other")),
            (
                "remote -v",
                Reply::said("other\thttps://host/r.git (fetch)"),
            ),
        ]);
        let reach = git.remote_reach(Contact {
            remote: "origin",
            direction: Direction::Fetch,
        });
        let unread = reach.unread.expect("an unlisted remote is unread");
        assert!(!reach.ssh && !reach.helper);
        assert!(
            unread.to_string().contains("shows no fetch URL"),
            "{unread}"
        );
    }

    /// A refused write leaves the note owed, so the next remote child says it.
    #[test]
    fn a_refused_note_stays_owed_until_it_lands() {
        let git = Git::answering([]);

        let mut offered = String::new();
        assert!(
            !git.say_once_with("a note", |note| {
                offered.push_str(note);
                false
            }),
            "a refused write must not report the note as said"
        );
        assert_eq!(offered, "a note", "the sayer is given the note verbatim");

        assert!(
            git.say_once_with("a note", |_| true),
            "a refused note is still owed, so the next child may say it"
        );
        assert!(
            !git.say_once_with("a note", |_| panic!("said twice")),
            "once it lands it must not repeat, once per remote child"
        );
    }

    /// A remote that pushes elsewhere owes two notes, not one — while the
    /// byte-identical note, owed by both directions, says itself once.
    #[test]
    fn distinct_notes_say_themselves_and_identical_ones_do_not_repeat() {
        let git = Git::answering([]);
        assert!(git.say_once_with("fetch note", |_| true));
        assert!(git.say_once_with("push note", |_| true));
        assert!(!git.say_once_with("fetch note", |_| panic!("repeated")));
    }

    /// A read that failed is unread, never safe, and a helper is a fact of
    /// its own — whichever spelling selects it, and even alongside ssh: the
    /// facts are independent, so neither can swallow the other.
    #[test]
    fn an_unread_url_is_never_classed_as_safe() {
        let failed = CliError::new("git exited 128");
        let unread = super::classify(Err(&failed));
        assert!(unread.unread.is_some() && !unread.ssh && !unread.helper);

        let helper = super::classify(Ok("ext::my-helper"));
        assert!(helper.helper && !helper.ssh);
        // Same helper, same hazard, both spellings — `marker://addr` runs
        // `git-remote-marker` exactly as `marker::addr` does, and git matches
        // its native table case-sensitively, so `SSH://` is a helper too.
        assert!(super::classify(Ok("marker://addr")).helper);
        assert!(super::classify(Ok("SSH://host/r.git")).helper);

        let ssh = super::classify(Ok("git@host:r.git"));
        assert!(ssh.ssh && !ssh.helper);
        let plain = super::classify(Ok("https://host/r.git"));
        assert!(!plain.ssh && !plain.helper && plain.unread.is_none());
        assert!(!super::classify(Ok("ftps://host/r.git")).helper);

        // A push reaches every URL listed, so a set that is both ssh and
        // helper keeps both facts (measured: `url` + an added helper `url`
        // both receive the push).
        let mixed = super::classify(Ok("ssh://host/a.git\nmarker://b"));
        assert!(mixed.ssh && mixed.helper);
    }

    /// A transport oakum could not read takes the operation's own outcome
    /// class: a verification that could not look is `unverified`, a push that
    /// never ran is a plain failure. That `child` refuses on it is pinned by
    /// the ssh-config tests in `tests/check.rs`, not here — and those are
    /// `#[cfg(unix)]`, so off unix nothing exercises the refusal.
    #[test]
    fn an_unreadable_transport_speaks_in_the_operations_own_voice() {
        let looked =
            Git::unreadable_transport(Op::AdvertisedTags { remote: "origin" }, "no config");
        assert!(matches!(looked, CliError::Unverified { .. }), "{looked:?}");
        assert!(looked.to_string().contains("no config"), "{looked}");

        let acted = Git::unreadable_transport(
            Op::PushTag {
                remote: "origin",
                tag: "v1.0.0",
            },
            "no config",
        );
        assert!(matches!(acted, CliError::Other(_)), "{acted:?}");
    }

    /// The whole policy, one row at a time, and the rows are independent: a
    /// helper speaks in both transport columns, ssh and an unread listing
    /// speak only when the transport could not be protected, and a set
    /// carrying two hazards says both.
    #[test]
    fn the_note_table_covers_every_reach_and_both_transport_columns() {
        use super::env::BatchSsh;
        let origin = Contact {
            remote: "origin",
            direction: Direction::Fetch,
        };
        let composed = BatchSsh::Composed(String::from("ssh -o BatchMode=yes"));
        let unprotected = BatchSsh::Unprotected(String::from("ssh.variant is opaque"));
        let plain = Reach {
            ssh: false,
            helper: false,
            unread: None,
        };
        let ssh_only = Reach {
            ssh: true,
            ..plain.clone()
        };
        let helper_only = Reach {
            helper: true,
            ..plain.clone()
        };
        let unread = Reach {
            unread: Some(CliError::new("could not read that remote's URL")),
            ..plain.clone()
        };

        assert!(super::notes_for(origin, &plain, &composed).is_empty());
        assert!(super::notes_for(origin, &plain, &unprotected).is_empty());
        assert!(super::notes_for(origin, &ssh_only, &composed).is_empty());

        // An unread listing speaks in both columns: oakum cannot rule a
        // helper out, and the helper hazard is transport-independent.
        let unread_composed = super::notes_for(origin, &unread, &composed);
        assert_eq!(unread_composed.len(), 1, "{unread_composed:?}");
        assert!(
            unread_composed[0].contains("could not establish what transport"),
            "{unread_composed:?}"
        );
        assert!(
            unread_composed[0].contains("may still block"),
            "{unread_composed:?}"
        );
        assert!(
            !unread_composed[0].contains("could not be protected"),
            "a composed transport owes no transport rider: {unread_composed:?}"
        );

        let helper_composed = super::notes_for(origin, &helper_only, &composed);
        assert_eq!(helper_composed.len(), 1, "{helper_composed:?}");
        assert!(
            helper_composed[0].contains("\"origin\""),
            "{helper_composed:?}"
        );
        assert!(helper_composed[0].contains("helper"), "{helper_composed:?}");
        assert!(
            helper_composed[0].contains("can still block"),
            "{helper_composed:?}"
        );
        let helper_unprotected = super::notes_for(origin, &helper_only, &unprotected);
        assert_eq!(helper_unprotected.len(), 1, "{helper_unprotected:?}");
        assert!(
            helper_unprotected[0].contains("ssh.variant is opaque"),
            "the transport reason must not be dropped: {helper_unprotected:?}"
        );

        let ssh = super::notes_for(origin, &ssh_only, &unprotected);
        assert_eq!(ssh.len(), 1, "{ssh:?}");
        assert!(ssh[0].contains("\"origin\""), "{}", ssh[0]);
        assert!(ssh[0].contains("ssh.variant is opaque"), "{}", ssh[0]);
        // The claim, not only the nouns: deleting the warning sentence left
        // all 27 test targets green.
        assert!(ssh[0].contains("cannot refuse ssh prompts"), "{}", ssh[0]);
        assert!(ssh[0].contains("can still block"), "{}", ssh[0]);

        let hedged = super::notes_for(origin, &unread, &unprotected);
        assert_eq!(hedged.len(), 1, "{hedged:?}");
        assert!(hedged[0].contains("may still block"), "{}", hedged[0]);
        assert!(
            hedged[0].contains("could not read that remote's URL"),
            "the note must say why it could not tell: {}",
            hedged[0]
        );
        assert!(
            hedged[0].contains("could not be protected either"),
            "an unprotected transport adds its reason: {}",
            hedged[0]
        );
        assert_ne!(ssh[0], hedged[0], "a guess must not read as a finding");

        // Two hazards, two notes: a push URL set that is ssh and helper at
        // once owes both, under an unprotected transport.
        let both = Reach {
            ssh: true,
            helper: true,
            unread: None,
        };
        let notes = super::notes_for(origin, &both, &unprotected);
        assert_eq!(notes.len(), 2, "{notes:?}");
        // ...and the helper note survives a composed transport alone.
        let notes = super::notes_for(origin, &both, &composed);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("helper"), "{notes:?}");
    }

    /// The axes of every operation, stated rather than sampled.
    const AXES: [(Op<'static>, Outcome, Option<Direction>, Answer, bool); 20] = [
        (Op::ReachableTags, Verification, None, Sometimes, false),
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
            Op::AnnotatedTag {
                name: "v1.0.0",
                commit: "HEAD",
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
    ];

    #[test]
    fn every_operation_states_every_axis() {
        assert_eq!(AXES.len(), Op::EVERY.len(), "a new operation needs a row");
        for ((op, outcome, contacts, answer, lossy), listed) in AXES.into_iter().zip(Op::EVERY) {
            assert_eq!(
                format!("{op:?}"),
                format!("{listed:?}"),
                "the table and `Op::EVERY` are in different orders"
            );
            let spec = op.spec();
            assert_eq!(spec.outcome, outcome, "{op:?} outcome");
            assert_eq!(
                op.contact().map(|contact| contact.direction),
                contacts,
                "{op:?} contacts"
            );
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
