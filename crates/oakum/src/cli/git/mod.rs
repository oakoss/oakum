//! Every git child oakum spawns.
//!
//! The environment, the outcome vocabulary, and the stdout decoding live here
//! rather than at each call site. Written per site, all three drift: prompt
//! suppression reached three of sixteen children before okm-6mz, and whether a
//! failure was `unverified` or a plain error was decided by which module the
//! caller happened to be in.
//!
//! [`Op`] and the axes that describe one live in [`op`]. A call here turns an
//! operation into a shape once and reads every axis off that one value, so the
//! argv a child runs and the class its failure takes cannot come from two
//! different answers.

mod env;
#[cfg(test)]
mod fake;
mod op;
mod reach;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use cap_std::fs::Dir;

use super::CliError;
pub(super) use op::Op;
use op::{Answer, Contact, Direction, OpShape, SKIP_CHECKS_ATOM};

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

    /// Git's own words, when it wrote any. A child that failed after explaining
    /// itself on stdout counts: reading stderr alone rendered that as `exit 128
    /// with no diagnostic`, which tells a reader git said nothing and sends
    /// them to inspect a git that did explain itself. Only for a failure, so a
    /// successful op's stdout data never leaks into a message.
    fn diagnostic(&self) -> Option<String> {
        let wrote = |bytes: &[u8]| {
            let text = String::from_utf8_lossy(bytes).trim().to_owned();
            (!text.is_empty()).then_some(text)
        };
        wrote(&self.stderr).or_else(|| (!self.succeeded()).then(|| wrote(&self.stdout)).flatten())
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

/// A commit id, kept distinct from a ref name so the two cannot be transposed
/// at a call site: `git check-ref-format` accepts a bare sha, so validating the
/// name catches nothing. Minted by [`Git::head`], [`Git::tag_commit`], and
/// [`Self::from_advertised`] (ls-remote tips), so every value is what a
/// commit-naming git read produced.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Commit(String);

/// [`Git::blob_kind`]'s answer. `Other` is any entry that is not a symlink:
/// a regular blob, but also a tree or a gitlink, which `cat-file blob` then
/// refuses on its own.
#[derive(Debug)]
pub(super) enum BlobKind {
    Absent,
    Symlink(String),
    Other,
}

impl Commit {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    /// Same provenance rule as [`Git::head`]: the string came from a
    /// commit-naming git read.
    pub(super) fn from_advertised(sha: &str) -> Result<Self, CliError> {
        if sha.is_empty() {
            return Err(CliError::unverified(
                "unverified: ls-remote advertised an empty commit id",
            ));
        }
        Ok(Self(sha.to_owned()))
    }
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
    /// Each child re-checks that `repo` still names this directory.
    /// [`Self::at`] leaves this empty (cwd plumbing, tests).
    held: Option<Dir>,
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
    transport: OnceLock<std::sync::Arc<Result<env::BatchSsh, env::TransportUnknown>>>,
    /// What each named remote's listed URLs established, per direction, so
    /// the notes are asked about the remote in hand.
    reach_by_remote: Mutex<BTreeMap<(String, Direction), reach::Reach>>,
    /// Notes already said, keyed by their text: distinct fetch and push notes
    /// each land, while a byte-identical one says itself once.
    warned: Mutex<BTreeSet<String>>,
}

impl Git {
    pub(super) fn at(repo: impl Into<PathBuf>) -> Self {
        Self::new(repo.into(), Runner::Child, None)
    }

    pub(super) fn at_repository(
        repo: &super::repository::Repository,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let path = repo.ambient_path()?.to_path_buf();
        let held = repo.dir().try_clone().map_err(|err| {
            CliError::new(format!("failed to clone the repository capability: {err}"))
        })?;
        Ok(Self::new(path, Runner::Child, Some(held)))
    }

    /// Answers from a script instead of a repository, each keyed by the command
    /// it answers. The path is never read.
    #[cfg(test)]
    pub(super) fn answering(replies: impl IntoIterator<Item = (&'static str, Reply)>) -> Self {
        Self::new(
            PathBuf::new(),
            Runner::Fake(fake::Fake::answering(replies)),
            None,
        )
    }

    fn new(repo: PathBuf, runner: Runner, held: Option<Dir>) -> Self {
        Self {
            repo,
            held,
            runner,
            transport: OnceLock::new(),
            reach_by_remote: Mutex::new(BTreeMap::new()),
            warned: Mutex::new(BTreeSet::new()),
        }
    }

    /// Each operation the caller asked for, in order, named as [`OpShape::name`]
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

    /// What `path` is in `commit`'s tree, so a caller never parses `ls-tree`
    /// output itself. A symlink carries its target, which is the blob's text.
    ///
    /// # Errors
    ///
    /// The operations' own outcome classes.
    pub(super) fn blob_kind(&self, commit: &Commit, path: &str) -> Result<BlobKind, CliError> {
        let entries = self.paths(Op::TreeEntry { commit, path })?;
        let Some(entry) = entries.first() else {
            return Ok(BlobKind::Absent);
        };
        if entry.starts_with("120000 ") {
            let target = self.text(Op::BlobText { commit, path })?;
            return Ok(BlobKind::Symlink(target));
        }
        Ok(BlobKind::Other)
    }

    /// [`Self::text`] without the trim: a file body whose last line ends in
    /// significant whitespace (a Markdown hard break) keeps it.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn raw_text(&self, op: Op<'_>) -> Result<String, CliError> {
        let shape = op.shape();
        let reply = self.checked(&shape, Reads::Text)?;
        String::from_utf8(reply.stdout).map_err(|_| shape.fail("output is not valid UTF-8"))
    }

    /// Trimmed stdout.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn text(&self, op: Op<'_>) -> Result<String, CliError> {
        let shape = op.shape();
        let reply = self.checked(&shape, Reads::Text)?;
        if shape.spec.lossy {
            return Ok(String::from_utf8_lossy(&reply.stdout).trim().to_owned());
        }
        String::from_utf8(reply.stdout)
            .map(|text| text.trim().to_owned())
            .map_err(|_| shape.fail("output is not valid UTF-8"))
    }

    /// HEAD's commit id. With [`Self::tag_commit`], one of the two mints for
    /// [`Commit`]: kept to the commit-naming reads, so a listing or URL read
    /// cannot be wrapped as a commit.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn head(&self) -> Result<Commit, CliError> {
        self.text(Op::Head).map(Commit)
    }

    /// The commit a local tag points at, peeled; `None` when the tag is
    /// absent.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn tag_commit(&self, tag: &str) -> Result<Option<Commit>, CliError> {
        Ok(self.optional_text(Op::LocalTagCommit { tag })?.map(Commit))
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
        let shape = op.shape();
        let stdout = self.checked(&shape, Reads::Paths)?.stdout;
        split_nul_paths(&stdout).ok_or_else(|| {
            shape.fail(
                "listed a path that is not valid UTF-8; oakum cannot attribute it to a package",
            )
        })
    }

    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn run(&self, op: Op<'_>) -> Result<(), CliError> {
        self.checked(&op.shape(), Reads::Text)?;
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
        let shape = op.shape();
        let reply = self.answered(&shape, Reads::Text)?;
        if reply.succeeded() {
            return Ok(true);
        }
        if reply.said_no() {
            return Ok(false);
        }
        Err(shape.fail(&reply.detail()))
    }

    /// `Ok(None)` for a search that matched nothing: exit 1 with both streams
    /// silent, which is how `git grep` says no. Unlike [`Self::optional_text`]
    /// the answer is split on NUL rather than trimmed, so a path that is or ends
    /// in whitespace survives.
    ///
    /// A search that matched nothing and one that searched nothing look alike
    /// here; only the caller knows whether there was anything to search.
    ///
    /// # Errors
    ///
    /// The operation's own outcome class.
    pub(super) fn matched_paths(&self, op: Op<'_>) -> Result<Option<Vec<String>>, CliError> {
        let shape = op.shape();
        let reply = self.answered(&shape, Reads::Paths)?;
        if !reply.succeeded() {
            if reply.said_no() {
                return Ok(None);
            }
            return Err(shape.fail(&reply.detail()));
        }
        split_nul_paths(&reply.stdout).map(Some).ok_or_else(|| {
            shape.fail("listed a path that is not valid UTF-8; oakum cannot name it as a gate")
        })
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
        let shape = op.shape();
        let reply = self.answered(&shape, Reads::Text)?;
        if !reply.succeeded() {
            if reply.said_no() {
                return Ok(None);
            }
            return Err(shape.fail(&reply.detail()));
        }
        let text =
            String::from_utf8(reply.stdout).map_err(|_| shape.fail("output is not valid UTF-8"))?;
        Ok(Some(text.trim().to_owned()).filter(|text| !text.is_empty()))
    }

    /// The answer, with a failed child already turned into the operation's own
    /// error. Callers that read an exit code as data go through [`Self::ask`].
    fn checked(&self, shape: &OpShape<'_>, reads: Reads) -> Result<Reply, CliError> {
        let reply = self.answered(shape, reads)?;
        if reply.succeeded() {
            return Ok(reply);
        }
        Err(shape.fail(&reply.detail()))
    }

    /// The answer, refusing a child that exited 0 without giving one. What
    /// silence means is a property of the operation, not of the accessor
    /// reading it: keyed on the outcome class instead, this reached neither
    /// `predicate` nor `optional_text`, and a config read that never ran came
    /// back as "no remote suppresses tags".
    fn answered(&self, shape: &OpShape<'_>, reads: Reads) -> Result<Reply, CliError> {
        let reply = self.ask(shape)?;
        if !reply.succeeded() || reply.spoke(reads) {
            return Ok(reply);
        }
        match shape.spec.answer {
            Answer::Always => Err(Self::unanswered(shape, &reply)),
            // Any stderr disqualifies, benign text included: an `ls-remote`
            // that found no tags while ssh wrote `Warning: Permanently added
            // ... to the list of known hosts` is refused. Deliberate — nothing
            // reliably separates a benign line from a consequential one, and
            // unlike `core.fsmonitor` no setting drops the diagnostic without
            // dropping the check with it. The refusal quotes the warning, so a
            // second run resolves it.
            Answer::Sometimes if reply.diagnostic().is_some() => {
                Err(Self::unanswered(shape, &reply))
            }
            Answer::Sometimes | Answer::Never => Ok(reply),
        }
    }

    fn ask(&self, shape: &OpShape<'_>) -> Result<Reply, CliError> {
        match &self.runner {
            Runner::Child => self.child(shape),
            #[cfg(test)]
            Runner::Fake(fake) => Ok(fake.answer(shape.name)),
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
    fn transport(&self) -> Result<&env::BatchSsh, &env::TransportUnknown> {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex, OnceLock};
        type Resolved =
            Mutex<HashMap<std::path::PathBuf, Arc<Result<env::BatchSsh, env::TransportUnknown>>>>;
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
    fn remote_reach(&self, contact: Contact<'_>) -> reach::Reach {
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
            reach::parse_remote_urls(&listing, &names)
        });
        let mut cache = self
            .reach_by_remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let answer = match parsed {
            Ok(urls) => {
                for (entry, urls) in urls {
                    cache.insert(entry, reach::classify(Ok(&urls)));
                }
                cache.get(&key).cloned().unwrap_or_else(|| {
                    let direction = match contact.direction {
                        Direction::Fetch => "fetch",
                        Direction::Push => "push",
                    };
                    reach::classify(Err(&CliError::new(format!(
                        "the listing shows no {direction} URL for remote {:?}",
                        contact.remote
                    ))))
                })
            }
            Err(err) => reach::classify(Err(&err)),
        };
        cache.insert(key, answer.clone());
        answer
    }

    /// A cache, so a torn entry is not a correctness problem: a poisoned lock is
    /// recovered rather than turned into a second panic that hides the first.
    fn remembered_reach(&self, key: &(String, Direction)) -> Option<reach::Reach> {
        self.reach_by_remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    fn child(&self, shape: &OpShape<'_>) -> Result<Reply, CliError> {
        if let Some(held) = &self.held {
            super::repository::confirm_ambient(held, &self.repo)
                .map_err(|err| shape.fail(&err.to_string()))?;
        }
        // Every child carries the transport: git opens sockets on its own
        // schedule — a partial clone's `diff` lazily fetches over ssh from an
        // op typed local (measured) — so protection cannot key on the
        // classification, and an unreadable ssh configuration stops every
        // operation rather than guessing away the user's key or proxy.
        let batch = self
            .transport()
            .map_err(|unknown| shape.unreadable_transport(unknown))?;
        if let Some(contact) = shape.contact {
            // Unconditional: a helper remote owes its note even when the
            // transport composed, because `BatchMode` never reaches what
            // a helper runs.
            let reach = self.remote_reach(contact);
            for note in reach::notes_for(contact, &reach, batch) {
                self.say_once(&note);
            }
        }
        let args: Vec<&str> = shape.argv.iter().map(String::as_str).collect();
        let started = env::deadlined_command(&self.repo, &args, batch)
            .output()
            .map_err(|failure| shape.fail(&failure.to_string()))?;
        Ok(Reply::from(started))
    }

    /// Separate from [`OpShape::fail`] because the child exited 0: a reader told
    /// that git "failed" checks the exit code, finds success, and concludes
    /// oakum is wrong.
    fn unanswered(shape: &OpShape<'_>, reply: &Reply) -> CliError {
        shape.phrase(|what| match reply.diagnostic() {
            Some(said) => format!("git {what} answered nothing while reporting: {said}"),
            None => format!("git {what} exited 0 without answering"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::op::fixture_commit;
    use super::{split_nul_paths, CliError, Contact, Direction, Git, Op, Reply};

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
            .set(
                Err(super::env::TransportUnknown::SshConfig(String::from(
                    "git config was killed by a signal",
                )))
                .into(),
            )
            .expect("the cache starts empty");
        for _ in 0..3 {
            assert_eq!(
                git.transport().expect_err("a cached failure").detail(),
                "git config was killed by a signal"
            );
        }
        let raised = Op::AdvertisedTags { remote: "origin" }
            .shape()
            .unreadable_transport(&super::env::TransportUnknown::SshConfig(String::from(
                "git config was killed by a signal",
            )));
        assert!(matches!(raised, CliError::Unverified { .. }), "{raised:?}");
        assert!(
            raised.to_string().contains("killed by a signal"),
            "{raised}"
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
        let file = std::env::temp_dir().join("oakum-trace.event");
        assert!(super::traces_to_a_file(file.as_os_str()));
    }

    /// Asserted against the command `untrace` actually builds. Written against
    /// its own literals instead, this test passed with `untrace` gutted to an
    /// empty body — the whole suite did.
    #[test]
    fn untrace_silences_every_channel_and_leaves_the_rest_alone() {
        let file = std::env::temp_dir().join("oakum-trace.perf");
        let inherited = [
            (
                std::ffi::OsString::from("GIT_TRACE"),
                std::ffi::OsString::from("1"),
            ),
            (
                std::ffi::OsString::from("GIT_TRACE_PACKET"),
                std::ffi::OsString::from("1"),
            ),
            (
                std::ffi::OsString::from("GIT_TRACE_A_CHANNEL_ADDED_LATER"),
                std::ffi::OsString::from("1"),
            ),
            (
                std::ffi::OsString::from("GIT_TRACE2_EVENT"),
                std::ffi::OsString::from("relative.log"),
            ),
            (
                std::ffi::OsString::from("GIT_TRACE2_PERF"),
                file.into_os_string(),
            ),
        ];
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

    /// A path git listed but oakum cannot read as UTF-8 fails in the
    /// operation's own voice rather than being lossily rewritten into a path
    /// that misses its package. `split_nul_paths` refusing the record is
    /// tested directly; this drives one through the accessor.
    #[test]
    fn a_non_utf8_path_fails_in_the_operations_own_voice() {
        let err = Git::answering([("diff --name-only", Reply::said(b"pkg/\xff.bin\0".to_vec()))])
            .paths(Op::ChangedPaths { from: "v1.0.0" })
            .expect_err("a non-UTF-8 path cannot name a package");
        assert!(
            err.to_string().contains("cannot attribute it to a package"),
            "{err}"
        );
        assert!(
            matches!(err, CliError::Unverified { .. }),
            "`diff --name-only` is a verification: {err:?}"
        );
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
                commit: fixture_commit(),
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

    /// A transport oakum could not read takes the operation's own outcome
    /// class: a verification that could not look is `unverified`, a push that
    /// never ran is a plain failure. That `child` refuses on it is pinned by
    /// the ssh-config tests in `tests/check.rs`, not here.
    #[test]
    fn an_unreadable_transport_speaks_in_the_operations_own_voice() {
        let unknown = super::env::TransportUnknown::SshConfig(String::from("no config"));
        let looked = Op::AdvertisedTags { remote: "origin" }
            .shape()
            .unreadable_transport(&unknown);
        assert!(matches!(looked, CliError::Unverified { .. }), "{looked:?}");
        assert!(looked.to_string().contains("no config"), "{looked}");

        let acted = Op::PushTag {
            remote: "origin",
            tag: "v1.0.0",
        }
        .shape()
        .unreadable_transport(&unknown);
        assert!(matches!(acted, CliError::Other(_)), "{acted:?}");
    }

    /// A repository git will not open is not an ssh problem:
    /// [`super::env::TransportUnknown`] splits the causes apart so the message
    /// stops offering a remedy that was measured to move the failure without
    /// fixing it.
    #[test]
    fn a_repository_git_cannot_open_is_not_reported_as_an_ssh_problem() {
        let refused = Op::AdvertisedTags { remote: "origin" }
            .shape()
            .unreadable_transport(&super::env::TransportUnknown::Repository(String::from(
                "exit 128: fatal: Expected git repo version <= 1, found 99",
            )))
            .to_string();
        assert!(
            refused.contains("could not read this repository"),
            "{refused}"
        );
        assert!(refused.contains("found 99"), "{refused}");
        assert!(!refused.contains("ssh"), "{refused}");
        assert!(!refused.contains("GIT_SSH_COMMAND"), "{refused}");
    }

    /// The remedy for an unreadable ssh variable cannot be to set that
    /// variable. Measured before the arm existed: an invalid-UTF-8
    /// `GIT_SSH_COMMAND` was answered by advising that both it and
    /// `GIT_SSH_VARIANT` be set.
    #[test]
    fn an_unreadable_ssh_variable_is_not_answered_by_setting_it() {
        let said = Op::AdvertisedTags { remote: "origin" }
            .shape()
            .unreadable_transport(&super::env::TransportUnknown::SshVariable(String::from(
                "GIT_SSH_COMMAND is not valid UTF-8",
            )))
            .to_string();
        assert!(
            said.contains("GIT_SSH_COMMAND is not valid UTF-8"),
            "{said}"
        );
        assert!(said.contains("repair or unset that variable"), "{said}");
        assert!(
            !said.contains("set both GIT_SSH_COMMAND and GIT_SSH_VARIANT"),
            "circular: {said}"
        );
    }

    /// A probe that never reached git establishes nothing — not about ssh, and
    /// not about the repository. Both remedies would be diagnoses nobody made,
    /// and the ssh one was measured being offered for a git that is not
    /// installed.
    #[test]
    fn a_probe_that_never_reached_git_claims_neither_cause() {
        let unasked = Op::AdvertisedTags { remote: "origin" }
            .shape()
            .unreadable_transport(&super::env::TransportUnknown::Unasked(String::from(
                "could not run git: No such file or directory (os error 2)",
            )))
            .to_string();
        assert!(unasked.contains("could not ask git"), "{unasked}");
        assert!(unasked.contains("No such file or directory"), "{unasked}");
        assert!(!unasked.contains("GIT_SSH_COMMAND"), "{unasked}");
        assert!(
            !unasked.contains("could not read this repository"),
            "a probe that did not run says nothing about the repository: {unasked}"
        );
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
