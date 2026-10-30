//! Tags reachable from HEAD. ADR-0014: `git for-each-ref --merged HEAD`, not
//! every tag in the repository. A maintenance branch's history is its release
//! line; listing all tags would mix in `main`'s.
//!
//! Git I/O stays in the binary (ADR-0002). `oakum::tags` parses names; it does
//! not talk to git.

use std::collections::{BTreeMap, BTreeSet};

use super::deliver_out;
use super::git::{Commit, Git, Op};
use super::CliError;

/// Uses the process cwd so git, not oakum, walks to `.git`. Plumbing tests
/// can point at a non-repo and get a failure instead of the parent checkout.
pub(super) fn run() -> Result<(), CliError> {
    let repo = std::env::current_dir().map_err(|err| CliError::unverified(err.to_string()))?;
    for group in reachable_tags(&Git::at(&repo))? {
        for tag in group.tags() {
            deliver_out(&format!("{}\t{tag}", group.commit()))
                .map_err(|err| CliError::undelivered("tag listing", &err))?;
        }
    }
    Ok(())
}

/// Tag names on one commit, sorted and unique.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitTags {
    commit: String,
    tags: Vec<String>,
}

impl CommitTags {
    fn new(commit: String, tags: BTreeSet<String>) -> Result<Self, CliError> {
        if commit.is_empty() {
            return Err(CliError::unverified(
                "unverified: empty commit for a reachable tag group",
            ));
        }
        if tags.is_empty() {
            return Err(CliError::unverified(
                "unverified: empty tag set for a reachable commit",
            ));
        }
        Ok(Self {
            commit,
            tags: tags.into_iter().collect(),
        })
    }

    pub(crate) fn commit(&self) -> &str {
        &self.commit
    }

    pub(crate) fn tags(&self) -> &[String] {
        &self.tags
    }
}

/// Empty is a successful look that found nothing. A git failure, a shallow
/// clone, a tag-suppressed clone, or unparseable git output is an error, not
/// an empty list — that would collapse "we did not look" into "never
/// released" (ADR-0014).
pub(crate) fn reachable_tags(git: &Git) -> Result<Vec<CommitTags>, CliError> {
    if let Some(why) = incomplete_tag_history(git, TagScope::Reachable)? {
        return Err(CliError::unverified(format!("unverified: {why}")));
    }
    let pairs = reachable_tag_records(git)?;
    group_pairs(pairs)
}

/// One query returns every reachable tag with its peeled identity, so
/// discovery stays at a fixed number of Git child processes no matter how
/// many tags the repository carries.
fn reachable_tag_records(git: &Git) -> Result<Vec<(String, String)>, CliError> {
    let stdout = tag_listing(git, Op::ReachableTags)?;
    parse_ref_records(&stdout)
}

/// A listing holds every tag only if the ref store could be read: an
/// unreadable directory under `refs/tags` drops the tags in it, or all of
/// them, and git says nothing (measured, git 2.56).
fn tag_listing(git: &Git, op: Op<'_>) -> Result<String, CliError> {
    if let Some(said) = git.unreadable_reftable()? {
        return Err(CliError::unverified(format!(
            "unverified: git reads no tags from a reftable store it cannot open: {said}; repair or restore the stack, then run again"
        )));
    }
    let stdout = git.text(op)?;
    if let Some(said) = git.tag_refs_complaint()? {
        let remedy = if said.contains("error opening directory") {
            "make the tag refs readable and run again"
        } else {
            "run `git refs verify` to see why it stopped"
        };
        return Err(CliError::unverified(format!(
            "unverified: git may have listed only some of the tags, because `git refs verify` reports: {said}; {remedy}"
        )));
    }
    Ok(stdout)
}

/// The names alone, behind the same gates as [`reachable_tags`].
pub(crate) fn reachable_tag_names(git: &Git) -> Result<BTreeSet<String>, CliError> {
    if let Some(why) = incomplete_tag_history(git, TagScope::Reachable)? {
        return Err(CliError::unverified(format!("unverified: {why}")));
    }
    Ok(reachable_tag_records(git)?
        .into_iter()
        .map(|(_, name)| name)
        .collect())
}

/// Every tag ref with its recorded object, the refs a reachable walk drops
/// included: one on unreachable history, one whose object is missing.
pub(crate) fn all_tag_objects(git: &Git) -> Result<Vec<(String, String)>, CliError> {
    if let Some(why) = incomplete_tag_history(git, TagScope::All)? {
        return Err(CliError::unverified(format!("unverified: {why}")));
    }
    let stdout = tag_listing(git, Op::AllTags)?;
    let mut listed = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let parsed = line.split_once('\0').and_then(|(refname, object)| {
            let name = refname
                .strip_prefix("refs/tags/")
                .filter(|n| !n.is_empty())?;
            (!object.is_empty() && !object.contains('\0')).then_some((name, object))
        });
        let Some((name, object)) = parsed else {
            return Err(CliError::unverified(format!(
                "unverified: unparseable for-each-ref record: {line:?}"
            )));
        };
        listed.push((name.to_owned(), object.to_owned()));
    }
    Ok(listed)
}

/// Parses NUL-separated `refname, objecttype, objectname, peeled type,
/// peeled name` records into `(commit, tag name)` pairs. A lightweight tag's
/// object is the commit itself; an annotated tag — nested included — peels
/// recursively through the `%(*...)` fields. A ref that does not resolve to
/// a commit fails closed.
fn parse_ref_records(stdout: &str) -> Result<Vec<(String, String)>, CliError> {
    let mut pairs = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\0').collect();
        let [refname, objecttype, objectname, peeled_type, peeled_name] = fields.as_slice() else {
            return Err(CliError::unverified(format!(
                "unverified: unparseable for-each-ref record: {line:?}"
            )));
        };
        let name = refname
            .strip_prefix("refs/tags/")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                CliError::unverified(format!("unverified: unparseable tag ref: {refname:?}"))
            })?;
        let commit = match *objecttype {
            "commit" if !objectname.is_empty() => *objectname,
            "tag"
                if !objectname.is_empty()
                    && *peeled_type == "commit"
                    && !peeled_name.is_empty() =>
            {
                *peeled_name
            }
            _ => {
                return Err(CliError::unverified(format!(
                    "unverified: tag {name:?} does not peel to a commit"
                )));
            }
        };
        pairs.push((commit.to_owned(), name.to_owned()));
    }
    Ok(pairs)
}

/// Which tags a reader asks for, which decides what a narrowed fetch can have
/// left out. Each reader fixes its own, so no caller can pick the wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagScope {
    /// Tags on HEAD's history. A fetch follows every tag on the branches it
    /// fetches, so a single-branch clone lists these whole (measured, okm-ap6).
    /// Not when HEAD came from a branch fetched by name into `FETCH_HEAD`
    /// alone, which follows no tags (measured); accepted as rare.
    Reachable,
    /// Every tag ref, those on branches a narrowed fetch never fetched too.
    All,
}

/// Why a local tag listing cannot be trusted to be the whole history, or
/// `None` when it can. Each condition lets `for-each-ref` succeed over a set
/// git never fetched, so a caller that reads tags without asking would take an
/// incomplete look for an empty one.
///
/// Every reader of tags asks this, so a new condition reaches all of them at
/// once rather than the one whose author remembered.
fn incomplete_tag_history(git: &Git, scope: TagScope) -> Result<Option<String>, CliError> {
    if is_shallow(git)? {
        return Ok(Some(String::from(
            "shallow clone; fetch full history before reading tags",
        )));
    }
    let remotes = remote_tag_config(git)?;
    let (remote, why) = if let Some(remote) = suppressing_remote(&remotes) {
        (
            remote,
            String::from("is configured with tagOpt --no-tags, so this clone does not fetch tags"),
        )
    } else if let Some((remote, tags)) =
        narrowed_remote(&remotes).filter(|_| scope == TagScope::All)
    {
        (
            remote,
            format!(
                "{}, so tags on branches it does not fetch may be missing",
                tags.fetches()
            ),
        )
    } else {
        return Ok(None);
    };
    // A local `--tags` override clears both conditions wherever the setting
    // lives (clone-written local key, global, or system config); an unscoped
    // `--unset` only clears a local value.
    let key = shell_quote(&format!("remote.{remote}.tagOpt"));
    let name = shell_quote(remote);
    // Quoting only appears for names carrying metacharacters, where the
    // command is POSIX-specific; say so rather than pasting it broken
    // into cmd.exe or PowerShell.
    let quoting_note = if key.contains('\'') || name.contains('\'') {
        " (commands use POSIX shell quoting; adapt for cmd.exe or PowerShell)"
    } else {
        ""
    };
    Ok(Some(format!(
        "remote {remote:?} {why}; run `git config --replace-all {key} --tags`, then \
         `git fetch --tags -- {name}` before reading tags{quoting_note}"
    )))
}

pub(super) fn is_shallow(git: &Git) -> Result<bool, CliError> {
    let stdout = git.text(Op::IsShallow)?;
    parse_is_shallow(&stdout)
}

/// Quotes a value for the copy-pasteable diagnostic. Plain names stay bare
/// so the common command also pastes cleanly into non-POSIX shells (cmd.exe,
/// PowerShell); anything else gets POSIX single-quoting.
pub(super) fn shell_quote(value: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '~');
    // A leading `~` is a home directory to the shell; inside a word, a literal.
    if !value.is_empty() && !value.starts_with('~') && value.chars().all(plain) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The two `tagOpt` values git acts on; it ignores any other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagOpt {
    Tags,
    NoTags,
}

/// What one remote's config says about which tags a fetch brings.
#[derive(Debug, Default, PartialEq, Eq)]
struct RemoteTags {
    /// The effective `tagOpt`: the last value git recognises.
    tag_opt: Option<TagOpt>,
    refspecs: Vec<String>,
}

impl RemoteTags {
    /// Whether a negative refspec takes some ref under `namespace` back out.
    fn excludes_from(&self, namespace: &str) -> bool {
        self.refspecs
            .iter()
            .filter_map(|refspec| refspec.strip_prefix('^'))
            .any(|pattern| {
                let prefix = pattern
                    .split_once('*')
                    .map_or(pattern, |(prefix, _)| prefix);
                pattern.starts_with(namespace) || namespace.starts_with(prefix)
            })
    }

    /// A refspec counts only when it stores the tags as tags:
    /// `+refs/*:refs/remotes/origin/*` fetches them under `refs/remotes`.
    fn fetches_every_tag(&self) -> bool {
        let stored = self
            .refspecs
            .iter()
            .any(|refspec| stores_every(refspec, "refs/tags/").as_deref() == Some("refs/tags/"));
        (self.tag_opt == Some(TagOpt::Tags) || stored) && !self.excludes_from("refs/tags/")
    }

    fn fetches_every_branch(&self) -> bool {
        self.refspecs
            .iter()
            .any(|refspec| stores_every(refspec, "refs/heads/").is_some())
            && !self.excludes_from("refs/heads/")
    }

    /// What the remote fetches, for the message: its first refspec stands for
    /// the rest, which can run long.
    fn fetches(&self) -> String {
        let named: Vec<&str> = self
            .refspecs
            .iter()
            .map(String::as_str)
            .filter(|refspec| !refspec.is_empty())
            .collect();
        match named.as_slice() {
            [] => String::from("fetches no branches"),
            [only] => format!("fetches only {only}"),
            [first, rest @ ..] => format!("fetches only {first} and {} more", rest.len()),
        }
    }
}

/// Where a refspec stores the refs under `namespace` when its source matches
/// every one of them, or `None`: git allows one `*` a side, so `refs/*`,
/// `refs/heads/*` and `refs/heads*` all match, and a refspec with no
/// destination stores nothing.
fn stores_every(refspec: &str, namespace: &str) -> Option<String> {
    let spec = refspec.strip_prefix('+').unwrap_or(refspec);
    let (source, destination) = spec.split_once(':')?;
    let (prefix, suffix) = source.split_once('*')?;
    let (stored, stored_suffix) = destination.split_once('*')?;
    let rest = namespace.strip_prefix(prefix)?;
    (suffix.is_empty() && stored_suffix.is_empty()).then(|| format!("{stored}{rest}"))
}

fn remote_tag_config(git: &Git) -> Result<BTreeMap<String, RemoteTags>, CliError> {
    match git.optional_text(Op::RemoteTagConfig)? {
        Some(stdout) => parse_remote_tags(&stdout),
        None => Ok(BTreeMap::new()),
    }
}

/// Each remote's tag settings from `git config -z --get-regexp` output
/// (`<key>\n<value>\0` entries for `remote.<name>.url`, `.tagopt` and
/// `.fetch`; a url alone still names the remote, and `-z` keeps a name
/// holding a space whole). Values print in ascending precedence order
/// (system, global, local), so the last recognised `tagopt` per remote is
/// the effective one, while `fetch` is multi-valued and every refspec counts.
fn parse_remote_tags(stdout: &str) -> Result<BTreeMap<String, RemoteTags>, CliError> {
    let mut remotes: BTreeMap<String, RemoteTags> = BTreeMap::new();
    for entry in stdout.split('\0') {
        let entry = entry.trim_start_matches('\n');
        if entry.is_empty() {
            continue;
        }
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        let unparseable = || {
            CliError::unverified(format!(
                "unverified: unparseable git config entry: {entry:?}"
            ))
        };
        let rest = key.strip_prefix("remote.").ok_or_else(unparseable)?;
        let (name, field) = rest
            .rsplit_once('.')
            .filter(|(name, _)| !name.is_empty())
            .ok_or_else(unparseable)?;
        let remote = remotes.entry(name.to_owned()).or_default();
        match (field, value) {
            ("tagopt", "--tags") => remote.tag_opt = Some(TagOpt::Tags),
            ("tagopt", "--no-tags") => remote.tag_opt = Some(TagOpt::NoTags),
            ("url" | "tagopt", _) => {}
            ("fetch", _) => remote.refspecs.push(value.to_owned()),
            _ => return Err(unparseable()),
        }
    }
    Ok(remotes)
}

/// A clone made with `git clone --no-tags` is non-shallow but records
/// `remote.<name>.tagOpt = --no-tags`, so its empty tag list means "we did
/// not fetch", not "never released". Any remote counts: the setting may have
/// been applied after the clone, to whichever remote the tags came from. A
/// refspec storing every tag still fetches them, as `git clone --mirror`
/// writes.
fn suppressing_remote(remotes: &BTreeMap<String, RemoteTags>) -> Option<&str> {
    remotes
        .iter()
        .find(|(_, tags)| tags.tag_opt == Some(TagOpt::NoTags) && !tags.fetches_every_tag())
        .map(|(name, _)| name.as_str())
}

/// `origin`, else the first remote by name, when it fetches neither every
/// branch nor every tag, as `git clone --single-branch` leaves it: git
/// follows only the tags on the branches it fetches, so a release tagged on
/// another branch never arrives (measured, okm-ap6). May, not does: a tag on
/// the fetched branch still arrives, so this reports rather than claims a
/// loss. Config says what the next fetch brings, not what the last one did,
/// so a remote widened but not yet fetched reads as whole.
fn narrowed_remote(remotes: &BTreeMap<String, RemoteTags>) -> Option<(&str, &RemoteTags)> {
    let (name, tags) = remotes
        .get_key_value("origin")
        .or_else(|| remotes.iter().next())?;
    (!tags.fetches_every_branch() && !tags.fetches_every_tag()).then_some((name.as_str(), tags))
}

fn parse_is_shallow(stdout: &str) -> Result<bool, CliError> {
    match stdout.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(CliError::unverified(format!(
            "unverified: git rev-parse --is-shallow-repository returned {other:?}"
        ))),
    }
}

fn group_pairs(
    pairs: impl IntoIterator<Item = (String, String)>,
) -> Result<Vec<CommitTags>, CliError> {
    let mut by_commit: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (commit, name) in pairs {
        if commit.is_empty() || name.is_empty() {
            return Err(CliError::unverified(
                "unverified: empty commit or tag name in a reachable pair",
            ));
        }
        by_commit.entry(commit).or_default().insert(name);
    }
    by_commit
        .into_iter()
        .map(|(commit, tags)| CommitTags::new(commit, tags))
        .collect()
}

/// Empty is a successful look. A git or parse failure is unverified, not
/// empty (ADR-0014).
pub(crate) fn remote_tag_names(git: &Git, remote: &str) -> Result<BTreeSet<String>, CliError> {
    Ok(remote_tag_commits(git, remote)?.0.into_keys().collect())
}

/// Peeled `^{}` lines win so an annotated tag maps to the commit, not the tag object.
pub(crate) fn remote_tag_commits(git: &Git, remote: &str) -> Result<Advertised, CliError> {
    let stdout = git.text(Op::AdvertisedTags { remote })?;
    parse_ls_remote_tag_commits(&stdout)
}

/// Peeled tip commits `ls-remote` advertised, keyed by tag name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Advertised(BTreeMap<String, Commit>);

impl Advertised {
    pub(crate) fn points_at(&self, tag: &str, commit: &Commit) -> bool {
        self.0.get(tag).is_some_and(|at| at == commit)
    }

    pub(crate) fn contains_tag(&self, tag: &str) -> bool {
        self.0.contains_key(tag)
    }

    pub(crate) fn get(&self, tag: &str) -> Option<&Commit> {
        self.0.get(tag)
    }

    pub(crate) fn tag_names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

pub(crate) fn first_remote(git: &Git) -> Result<Option<String>, CliError> {
    let stdout = git.text(Op::RemoteNames)?;
    Ok(preferred_remote(&stdout))
}

fn preferred_remote(stdout: &str) -> Option<String> {
    let remotes: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect();
    if remotes.iter().any(|name| name == "origin") {
        return Some(String::from("origin"));
    }
    remotes.into_iter().next()
}

/// Peeled `^{}` suffixes are stripped so a peeled-only listing still yields
/// the tag name.
#[cfg(test)]
pub(crate) fn parse_ls_remote_tags(stdout: &str) -> Result<BTreeSet<String>, CliError> {
    Ok(parse_ls_remote_tag_commits(stdout)?.0.into_keys().collect())
}

fn parse_ls_remote_tag_commits(stdout: &str) -> Result<Advertised, CliError> {
    let mut commits = BTreeMap::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((sha, reference)) = line.split_once('\t') else {
            return Err(CliError::unverified(format!(
                "unverified: unparseable ls-remote line {line:?}"
            )));
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            return Err(CliError::unverified(format!(
                "unverified: ls-remote ref is not a tag: {reference:?}"
            )));
        };
        if let Some(name) = name.strip_suffix("^{}") {
            if name.is_empty() {
                return Err(CliError::unverified(
                    "unverified: ls-remote advertised an empty tag name",
                ));
            }
            commits.insert(String::from(name), Commit::from_advertised(sha)?);
            continue;
        }
        if name.is_empty() {
            return Err(CliError::unverified(
                "unverified: ls-remote advertised an empty tag name",
            ));
        }
        if let std::collections::btree_map::Entry::Vacant(slot) = commits.entry(String::from(name))
        {
            slot.insert(Commit::from_advertised(sha)?);
        }
    }
    Ok(Advertised(commits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::git::Reply;

    /// The full listing parses `refs/tags/` records only, and a record that
    /// is not one is a refused look — the caller decides release state from
    /// this list, so a half-read listing must not read as a short one.
    #[test]
    fn a_malformed_all_tags_record_is_unverified_not_skipped() {
        let listed = all_tag_objects(&Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (
                "for-each-ref refs/tags",
                Reply::said("refs/tags/v0.1.0\0cafe1234\n\nrefs/tags/pkg/v1.0.0\0beef5678"),
            ),
            (VERIFY, Reply::said("")),
        ]))
        .expect("well-formed records parse");
        assert_eq!(
            listed,
            [
                (String::from("v0.1.0"), String::from("cafe1234")),
                (String::from("pkg/v1.0.0"), String::from("beef5678")),
            ]
        );

        for (label, line) in [
            ("no separator", "refs/tags/v0.1.0 cafe1234"),
            ("not a tag ref", "refs/heads/v0.1.0\0cafe1234"),
            ("empty name", "refs/tags/\0cafe1234"),
            ("empty object", "refs/tags/v0.1.0\0"),
        ] {
            let err = all_tag_objects(&Git::answering([
                (SHALLOW, Reply::said("false")),
                (SUPPRESSION, Reply::absent()),
                (STORE, Reply::said(FILES_STORE)),
                ("for-each-ref refs/tags", Reply::said(line)),
                (VERIFY, Reply::said("")),
            ]))
            .expect_err(label);
            assert!(
                matches!(err, CliError::Unverified { .. }),
                "{label}: {err:?}"
            );
            assert!(err.to_string().contains("unparseable"), "{label}: {err}");
        }
    }

    /// Each gate holds only if the read it guards never happens, and "never
    /// happened" is not something a repository can be asked to report. The
    /// scripted [`Git`] answers by operation rather than by position, so every
    /// read is scripted whether or not the gate should let it run: a gate that
    /// stops holding shows up in [`Git::asked`] rather than by knocking the
    /// answers out of alignment.
    const SHALLOW: &str = "rev-parse --is-shallow-repository";
    const SUPPRESSION: &str = "config --get-regexp url|tagopt|fetch";
    const TAGS: &str = "for-each-ref --merged HEAD";
    const VERIFY: &str = "refs verify";
    const STORE: &str = "rev-parse --show-ref-format";
    const FILES_STORE: &str = "files\n/repo/.git";
    const GIT_DIR: &str = "rev-parse --git-dir";

    #[test]
    fn a_shallow_clone_is_refused_before_any_tag_is_read() {
        let git = Git::answering([
            (SHALLOW, Reply::said("true")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (TAGS, Reply::said("")),
        ]);
        let err =
            reachable_tags(&git).expect_err("a shallow clone must not read as never-released");
        assert!(err.to_string().contains("shallow"), "{err}");
        assert_eq!(git.asked(), [SHALLOW]);
    }

    #[test]
    fn a_tag_suppressed_remote_is_refused_before_any_tag_is_read() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (
                SUPPRESSION,
                Reply::said(z("remote.origin.tagopt --no-tags")),
            ),
            (TAGS, Reply::said("")),
        ]);
        let err =
            reachable_tags(&git).expect_err("a --no-tags clone must not read as never-released");
        assert!(err.to_string().contains("tagOpt --no-tags"), "{err}");
        assert_eq!(git.asked(), [SHALLOW, SUPPRESSION]);
    }

    /// The suppression read is the one that leaks quietly: it is the only
    /// `Spec::LOOK` reached through `optional_text`, so a config read that
    /// warned and answered nothing used to return "no remote suppresses tags".
    #[test]
    fn a_warning_on_the_suppression_read_is_not_a_clone_that_fetches_tags() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (
                SUPPRESSION,
                Reply::warned("warning: unable to access '/etc/gitconfig': Permission denied"),
            ),
            (TAGS, Reply::said("")),
        ]);
        let err = reachable_tags(&git)
            .expect_err("a config read that never answered must not read as no suppression");
        assert!(matches!(err, CliError::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("Permission denied"), "{err}");
        assert_eq!(git.asked(), [SHALLOW, SUPPRESSION]);
    }

    /// The other half of the same rule. Every read answered, no tags found,
    /// and the ref store verified: the repository was looked at, so empty is
    /// the answer rather than `unverified` (ADR-0014).
    #[test]
    fn a_repository_that_answered_every_read_and_has_no_tags_is_empty_not_unverified() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (TAGS, Reply::said("")),
            (VERIFY, Reply::said("")),
        ]);
        let tags = reachable_tags(&git).expect("an empty look");
        assert!(tags.is_empty(), "{tags:?}");
        assert_eq!(git.asked().len(), 5, "{:?}", git.asked());
        assert!(git.asked()[3].starts_with(TAGS), "{:?}", git.asked());
    }

    /// An unreadable `refs/tags` lists nothing and says nothing; only the
    /// ref store's own check notices (measured, git 2.56).
    #[test]
    fn an_empty_listing_over_an_unreadable_tag_store_is_unverified() {
        let warned = "warning: error opening directory '.git/refs/tags/': Permission denied";
        for op in ["reachable", "all"] {
            let git = Git::answering([
                (SHALLOW, Reply::said("false")),
                (SUPPRESSION, Reply::absent()),
                (STORE, Reply::said(FILES_STORE)),
                (TAGS, Reply::said("")),
                ("for-each-ref refs/tags", Reply::said("")),
                (VERIFY, Reply::warned(warned)),
            ]);
            let err = if op == "reachable" {
                reachable_tags(&git).map(|_| ()).expect_err(op)
            } else {
                all_tag_objects(&git).map(|_| ()).expect_err(op)
            };
            assert!(matches!(err, CliError::Unverified { .. }), "{op}: {err:?}");
            assert!(err.to_string().contains(warned), "{op}: {err}");
        }
    }

    /// A scratch git dir holding a reftable stack whose tables carry `bodies`.
    fn reftable_dir(label: &str, bodies: &[&[u8]]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("oakum-reftable-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("reftable")).expect("stack");
        let mut list = String::new();
        for (i, body) in bodies.iter().enumerate() {
            let name = format!("{i}.ref");
            std::fs::write(dir.join("reftable").join(&name), body).expect("table");
            list.push_str(&name);
            list.push('\n');
        }
        std::fs::write(dir.join("reftable/tables.list"), list).expect("list");
        dir
    }

    fn reftable_git(common: &std::path::Path, git_dir: &std::path::Path) -> Git {
        Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (
                STORE,
                Reply::said(format!("reftable\n{}", common.display())),
            ),
            (GIT_DIR, Reply::said(git_dir.display().to_string())),
            (TAGS, Reply::said("")),
            ("for-each-ref refs/tags", Reply::said("")),
            (VERIFY, Reply::said("")),
        ])
    }

    /// Every way git loads a reftable stack as empty reads as unverified on
    /// both listings, named by the file; a stack git can read lists as usual.
    #[test]
    fn a_reftable_stack_git_cannot_load_is_unverified() {
        let healthy = reftable_dir("healthy", &[b"REFT\x01"]);
        let cases: [(&str, std::path::PathBuf, &str); 5] = [
            (
                "no stack",
                {
                    let dir = reftable_dir("no-stack", &[]);
                    std::fs::remove_dir_all(dir.join("reftable")).expect("drop");
                    dir
                },
                "tables.list` could not be opened",
            ),
            (
                "missing table",
                {
                    let dir = reftable_dir("missing-table", &[b"REFT\x01"]);
                    std::fs::remove_file(dir.join("reftable/0.ref")).expect("drop");
                    dir
                },
                "0.ref` could not be opened",
            ),
            (
                "not a table",
                reftable_dir("not-a-table", &[b"REFT\x01", b"oops"]),
                "1.ref` is not a reftable table",
            ),
            (
                "short table",
                reftable_dir("short-table", &[b"RE"]),
                "0.ref` could not be read",
            ),
            (
                "a directory",
                {
                    let dir = reftable_dir("dir-table", &[]);
                    std::fs::create_dir(dir.join("reftable/0.ref")).expect("dir");
                    std::fs::write(dir.join("reftable/tables.list"), "0.ref\n").expect("list");
                    dir
                },
                "0.ref` is not a regular file",
            ),
        ];
        for (label, dir, said) in &cases {
            for op in ["reachable", "all"] {
                let git = reftable_git(dir, dir);
                let err = if op == "reachable" {
                    reachable_tags(&git).map(|_| ()).expect_err(label)
                } else {
                    all_tag_objects(&git).map(|_| ()).expect_err(label)
                };
                assert!(
                    matches!(err, CliError::Unverified { .. }),
                    "{label} {op}: {err:?}"
                );
                assert!(err.to_string().contains(said), "{label} {op}: {err}");
            }
        }
        // A linked worktree's own stack loads with the common one.
        let worktree = reftable_dir("worktree", &[b"oops"]);
        let err = reachable_tags(&reftable_git(&healthy, &worktree)).expect_err("worktree");
        assert!(
            err.to_string().contains("0.ref` is not a reftable table"),
            "{err}"
        );
        for op in ["reachable", "all"] {
            let git = reftable_git(&healthy, &healthy);
            let found = if op == "reachable" {
                reachable_tags(&git).expect("healthy").len()
            } else {
                all_tag_objects(&git).expect("healthy").len()
            };
            assert_eq!(found, 0, "{op}");
        }
        for (_, dir, _) in &cases {
            let _ = std::fs::remove_dir_all(dir);
        }
        let _ = std::fs::remove_dir_all(&healthy);
        let _ = std::fs::remove_dir_all(&worktree);
    }

    /// The format line has no newline, so a git dir whose path holds one is
    /// read whole; a git before 2.45 echoes the flags and is not reftable.
    #[test]
    fn the_ref_store_answer_is_split_once() {
        // Windows names cannot hold a newline or end in a space.
        #[cfg(unix)]
        for name in ["line\nbreak", "trailing "] {
            let parent =
                std::env::temp_dir().join(format!("oakum-reftable-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&parent);
            std::fs::create_dir_all(parent.join("reftable")).expect("stack");
            std::fs::write(parent.join("reftable/0.ref"), b"REFT\x01").expect("table");
            std::fs::write(parent.join("reftable/tables.list"), "0.ref\n").expect("list");
            let tags = reachable_tags(&reftable_git(&parent, &parent)).expect(name);
            assert!(tags.is_empty(), "{tags:?}");
            let _ = std::fs::remove_dir_all(&parent);
        }

        let old = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (
                STORE,
                Reply::said("--show-ref-format\n--path-format=absolute\n/repo/.git"),
            ),
            (TAGS, Reply::said("")),
            (VERIFY, Reply::said("")),
        ]);
        let tags = reachable_tags(&old).expect("an older git");
        assert!(tags.is_empty(), "{tags:?}");

        let one_line = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said("reftable")),
        ]);
        let err = reachable_tags(&one_line).expect_err("a one-line answer");
        assert!(matches!(err, CliError::Unverified { .. }), "{err:?}");
    }

    /// A git before 2.47 has no `refs verify`, and its complaint names no tag
    /// ref, so an empty listing stays an empty listing there.
    #[test]
    fn a_git_without_refs_verify_keeps_an_empty_listing() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (TAGS, Reply::said("")),
            (
                VERIFY,
                Reply::failed(1, "git: 'refs' is not a git command. See 'git --help'."),
            ),
        ]);
        let tags = reachable_tags(&git).expect("an empty look");
        assert!(tags.is_empty(), "{tags:?}");
    }

    /// A listing that is not empty can still be short: an unreadable
    /// subdirectory under `refs/tags` drops its tags and git says nothing
    /// (measured, git 2.56). The store is wherever the git dir is: a
    /// submodule's under `.git/modules/`, a separate git dir's anywhere, and
    /// a tag directory may itself end in `.git` (each measured). Whatever
    /// cannot be placed, a quote split by a newline in the path or a git dir
    /// kept under a folder `refs/tags`, is unverified too.
    #[test]
    fn a_listing_beside_an_unopened_tag_directory_is_unverified() {
        for (quoted, named) in [
            (".git/refs/tags/rel/", ".git/refs/tags/rel/"),
            (
                "/w/.git/modules/sub/refs/tags/rel/",
                "modules/sub/refs/tags/rel/",
            ),
            ("/w/meta/refs/tags/rel/", "/w/meta/refs/tags/rel/"),
            ("/w/me'ta/refs/tags/rel/", "/w/me'ta/refs/tags/rel/"),
            ("/w/meta /refs/tags/rel/", "/w/meta /refs/tags/rel/"),
            (".git/refs/tags/pkg.git/", ".git/refs/tags/pkg.git/"),
            ("/w/line\nbreak/.git/refs/tags/rel/", "'/w/line"),
            ("/w/meta'\nbreak/refs/tags/rel/", "'/w/meta'"),
            ("/w/me': ta\nbreak/refs/tags/rel/", "'/w/me': ta"),
            ("/w/line\nbreak/.git/refs/heads/", "'/w/line"),
            (
                "/srv/refs/tags/repo/.git/refs/heads/",
                "/srv/refs/tags/repo/",
            ),
        ] {
            let warned = format!("warning: error opening directory '{quoted}': Permission denied");
            let git = Git::answering([
                (SHALLOW, Reply::said("false")),
                (SUPPRESSION, Reply::absent()),
                (STORE, Reply::said(FILES_STORE)),
                (TAGS, Reply::said("refs/tags/v1.0.0\0commit\0cafebabe\0\0")),
                (VERIFY, Reply::warned(&warned)),
            ]);
            let err = reachable_tags(&git).expect_err(quoted);
            assert!(
                matches!(err, CliError::Unverified { .. }),
                "{quoted}: {err:?}"
            );
            assert!(err.to_string().contains(named), "{err}");
        }
    }

    /// What `refs verify` says about a ref's form, or about refs outside
    /// `refs/tags`, names refs it read: the listing stands (each measured on
    /// git 2.56 but the branch case).
    #[test]
    fn a_verify_warning_about_form_keeps_the_listing() {
        for said in [
            "warning: refs/tags/v0.9.0: refMissingNewline: misses LF at the end",
            "warning: refs/tags/alias: symlinkRef: use deprecated symbolic link for symref",
            "error: refs/tags/.DS_Store: badRefName: invalid refname format",
            "error: refs/heads/x: badRefContent: garbage",
            "warning: error opening directory '.git/refs/tags-backup/': Permission denied",
            "warning: error opening directory '/w/.git/refs/heads/': Permission denied",
            "warning: error opening directory '/w/tags/refs/heads/': Permission denied",
        ] {
            let git = Git::answering([
                (SHALLOW, Reply::said("false")),
                (SUPPRESSION, Reply::absent()),
                (STORE, Reply::said(FILES_STORE)),
                (TAGS, Reply::said("")),
                (VERIFY, Reply::failed(255, said)),
            ]);
            let tags = reachable_tags(&git).expect(said);
            assert!(tags.is_empty(), "{said}: {tags:?}");
        }
    }

    /// A check killed before it finished vouches for nothing.
    #[test]
    fn a_verify_killed_by_a_signal_is_unverified() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (TAGS, Reply::said("")),
            (VERIFY, Reply::was_signalled()),
        ]);
        let err = reachable_tags(&git).expect_err("unconfirmed");
        assert!(err.to_string().contains("terminated by a signal"), "{err}");
        assert!(err.to_string().contains("to see why it stopped"), "{err}");
        assert!(!err.to_string().contains("readable"), "{err}");
    }

    #[test]
    fn a_warning_on_the_tag_read_is_not_an_empty_tag_list() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (
                TAGS,
                Reply::warned("warning: ignoring broken ref refs/tags/v1.0.0"),
            ),
        ]);
        let err = reachable_tags(&git).expect_err("a warned tag read must not read as no tags");
        assert!(err.to_string().contains("broken ref"), "{err}");
        let asked = git.asked();
        assert_eq!(asked.len(), 4, "{asked:?}");
        assert!(asked[3].starts_with(TAGS), "{asked:?}");
    }

    #[test]
    fn an_annotated_tag_peels_to_its_commit_through_the_whole_read() {
        let git = Git::answering([
            (SHALLOW, Reply::said("false")),
            (SUPPRESSION, Reply::absent()),
            (STORE, Reply::said(FILES_STORE)),
            (
                TAGS,
                Reply::said("refs/tags/v1.0.0\0tag\0deadbeef\0commit\0cafebabe"),
            ),
            (VERIFY, Reply::said("")),
        ]);
        let groups = reachable_tags(&git).expect("one reachable tag");
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].commit(), "cafebabe");
        assert_eq!(groups[0].tags(), ["v1.0.0"]);
    }

    #[test]
    fn hyphen_and_slash_on_one_commit_group_together() {
        let grouped = group_pairs([
            ("aa".into(), "linesmith/v0.2.0".into()),
            ("aa".into(), "linesmith-core-v0.2.0".into()),
            ("bb".into(), "v0.1.0".into()),
        ])
        .expect("ok");
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].commit(), "aa");
        assert_eq!(
            grouped[0].tags(),
            &[
                "linesmith-core-v0.2.0".to_string(),
                "linesmith/v0.2.0".to_string()
            ]
        );
        assert_eq!(grouped[1].tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn empty_commit_or_name_is_unverified() {
        let err = group_pairs([(String::new(), "v0.1.0".into())]).expect_err("empty commit");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err = group_pairs([("aa".into(), String::new())]).expect_err("empty name");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err = CommitTags::new(String::new(), BTreeSet::from(["v".into()])).expect_err("new");
        assert!(err.to_string().contains("empty commit"), "{err}");
        let err = CommitTags::new("aa".into(), BTreeSet::new()).expect_err("empty tags");
        assert!(err.to_string().contains("empty tag set"), "{err}");
    }

    #[test]
    fn parse_ref_records_resolves_lightweight_and_annotated_tags() {
        let pairs = parse_ref_records(concat!(
            "refs/tags/v0.1.0\0commit\0aa\0\0\n",
            "refs/tags/v0.2.0\0tag\0tt\0commit\0bb\n",
        ))
        .expect("parse");
        assert_eq!(
            pairs,
            vec![
                ("aa".to_string(), "v0.1.0".to_string()),
                ("bb".to_string(), "v0.2.0".to_string()),
            ]
        );
        assert_eq!(parse_ref_records("").expect("empty"), vec![]);
    }

    #[test]
    fn parse_ref_records_refuses_non_commit_tags() {
        let err = parse_ref_records("refs/tags/blob\0blob\0bb\0\0\n").expect_err("blob");
        assert!(
            err.to_string().contains("does not peel to a commit"),
            "{err}"
        );
        let err = parse_ref_records("refs/tags/tb\0tag\0tt\0blob\0bb\n").expect_err("tag of blob");
        assert!(
            err.to_string().contains("does not peel to a commit"),
            "{err}"
        );
        let err = parse_ref_records("refs/tags/v1\0commit\0\0\0\n").expect_err("empty object");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err =
            parse_ref_records("refs/tags/v1\0tag\0\0commit\0bb\n").expect_err("empty tag object");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn parse_ref_records_refuses_malformed_records() {
        let err = parse_ref_records("refs/tags/v1\0commit\0aa\n").expect_err("field count");
        assert!(err.to_string().contains("unparseable"), "{err}");
        let err =
            parse_ref_records("refs/heads/main\0commit\0aa\0\0\n").expect_err("not a tag ref");
        assert!(err.to_string().contains("unparseable tag ref"), "{err}");
        let err = parse_ref_records("refs/tags/\0commit\0aa\0\0\n").expect_err("empty name");
        assert!(err.to_string().contains("unparseable tag ref"), "{err}");
    }

    /// `git config -z` output from `key value` lines, the key ending at the
    /// first space, so the tables read like `git config --list`.
    fn z(lines: &str) -> String {
        lines.lines().fold(String::new(), |mut out, line| {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            out.push_str(key);
            out.push('\n');
            out.push_str(value);
            out.push('\0');
            out
        })
    }

    fn parse_tag_suppression(lines: &str) -> Result<Option<String>, CliError> {
        Ok(suppressing_remote(&parse_remote_tags(&z(lines))?).map(str::to_owned))
    }

    fn narrowed(lines: &str) -> Option<String> {
        narrowed_remote(&parse_remote_tags(&z(lines)).expect("parse"))
            .map(|(name, _)| name.to_owned())
    }

    /// `-z` keeps a remote name holding a space whole, as a hand-written
    /// `[remote "my fork"]` section makes one.
    #[test]
    fn a_remote_name_with_a_space_parses() {
        let remotes =
            parse_remote_tags("remote.my fork.url\n../up\0remote.my fork.fetch\n+refs/heads/*:refs/remotes/my fork/*\0")
                .expect("parse");
        assert_eq!(remotes.keys().collect::<Vec<_>>(), ["my fork"]);
        assert!(remotes["my fork"].fetches_every_branch());
    }

    /// One branch, a list of them, a glob under one directory, an excluded
    /// branch, or a url with no refspec all leave tags behind; any glob over
    /// every branch, every tag or every ref does not, in either order, nor
    /// does `tagOpt --tags`.
    #[test]
    fn a_remote_fetching_part_of_its_branches_is_narrowed() {
        let one = "+refs/heads/main:refs/remotes/origin/main";
        for part in [
            format!("remote.origin.fetch {one}\n"),
            format!("remote.origin.fetch {one}\nremote.origin.fetch +refs/heads/next:refs/remotes/origin/next\n"),
            String::from("remote.origin.fetch +refs/heads/release/*:refs/remotes/origin/release/*\n"),
            String::from("remote.origin.fetch +refs/heads/*-stable:refs/remotes/origin/*-stable\n"),
            String::from("remote.origin.fetch +refs/heads/*:refs/remotes/origin/*\nremote.origin.fetch ^refs/heads/release/*\n"),
            String::from("remote.origin.url https://example.invalid/r\n"),
            String::from("remote.origin.fetch \n"),
            String::from("remote.origin.fetch refs/heads/*\n"),
            format!("remote.origin.fetch {one}\nremote.origin.tagopt bogus\n"),
            format!("remote.origin.fetch {one}\nremote.origin.tagopt --tags\nremote.origin.fetch ^refs/tags/v1\n"),
        ] {
            assert_eq!(narrowed(&part).as_deref(), Some("origin"), "{part:?}");
        }
        for whole in [
            String::from("remote.origin.fetch +refs/heads/*:refs/remotes/origin/*\n"),
            String::from("remote.origin.fetch refs/heads/*:refs/remotes/origin/*\n"),
            String::from("remote.origin.fetch +refs/heads*:refs/remotes/origin*\n"),
            String::from("remote.origin.fetch +refs/*:refs/*\n"),
            format!("remote.origin.fetch {one}\nremote.origin.fetch +refs/tags/*:refs/tags/*\n"),
            format!("remote.origin.fetch +refs/tags/*:refs/tags/*\nremote.origin.fetch {one}\n"),
            format!("remote.origin.fetch {one}\nremote.origin.tagopt --tags\n"),
            String::from("remote.origin.fetch +refs/heads/*:refs/remotes/origin/*\nremote.origin.fetch ^refs/pull/*\n"),
            String::new(),
        ] {
            assert_eq!(narrowed(&whole), None, "{whole:?}");
        }
    }

    /// `origin` is the remote tags are read from when it exists, with or
    /// without a refspec; another remote narrowed to one branch, as
    /// `git remote add -t` leaves it, is not a missing tag set.
    #[test]
    fn only_the_tag_remote_is_judged() {
        let fork = "remote.backup.fetch +refs/heads/main:refs/remotes/backup/main\nremote.origin.fetch +refs/heads/*:refs/remotes/origin/*\n";
        assert_eq!(narrowed(fork), None);
        let bare_origin = "remote.backup.fetch +refs/heads/*:refs/remotes/backup/*\nremote.origin.url https://example.invalid/r\n";
        assert_eq!(narrowed(bare_origin).as_deref(), Some("origin"));
        let without_origin = "remote.a.fetch +refs/heads/main:refs/remotes/a/main\nremote.b.fetch +refs/heads/*:refs/remotes/b/*\n";
        assert_eq!(narrowed(without_origin).as_deref(), Some("a"));
        let dotted = "remote.my.fork.fetch +refs/heads/main:refs/remotes/my.fork/main\n";
        assert_eq!(narrowed(dotted).as_deref(), Some("my.fork"));
    }

    /// A narrowed fetch still followed every tag on the branch it fetched,
    /// so only a reader of every tag is told some may be missing.
    #[test]
    fn a_narrowed_remote_leaves_only_the_full_tag_set_incomplete() {
        let config = "remote.origin.fetch +refs/heads/main:refs/remotes/origin/main\nremote.origin.fetch +refs/heads/next:refs/remotes/origin/next\n";
        let git = || {
            Git::answering([
                (SHALLOW, Reply::said("false")),
                (SUPPRESSION, Reply::said(z(config))),
            ])
        };
        assert_eq!(
            incomplete_tag_history(&git(), TagScope::Reachable).expect("look"),
            None
        );
        let why = incomplete_tag_history(&git(), TagScope::All)
            .expect("look")
            .expect("narrowed");
        assert!(
            why.contains("fetches only +refs/heads/main:refs/remotes/origin/main and 1 more"),
            "{why}"
        );
        assert!(why.contains("may be missing"), "{why}");
        assert!(
            why.contains("git config --replace-all remote.origin.tagOpt --tags"),
            "{why}"
        );
        assert!(why.contains("git fetch --tags -- origin"), "{why}");
    }

    #[test]
    fn reachable_names_carry_the_same_gates() {
        let git = Git::answering([(SHALLOW, Reply::said("true")), (TAGS, Reply::said(""))]);
        let err = reachable_tag_names(&git).expect_err("shallow");
        assert!(err.to_string().contains("shallow clone"), "{err}");
    }

    #[test]
    fn a_remote_with_no_refspec_fetches_no_branches() {
        let tags =
            parse_remote_tags(&z("remote.origin.url u\nremote.origin.fetch \n")).expect("parse");
        assert_eq!(tags["origin"].fetches(), "fetches no branches");
    }

    /// git ignores a `tagOpt` it does not know, so an earlier `--no-tags`
    /// still holds; a mirror's refspec fetches every tag whatever `tagOpt`
    /// says (`git clone --mirror` writes both).
    #[test]
    fn suppression_follows_what_git_fetches() {
        assert_eq!(
            parse_tag_suppression("remote.origin.tagopt --no-tags\nremote.origin.tagopt bogus\n")
                .expect("parse")
                .as_deref(),
            Some("origin")
        );
        assert_eq!(
            parse_tag_suppression(
                "remote.origin.fetch +refs/*:refs/*\nremote.origin.tagopt --no-tags\n"
            )
            .expect("parse"),
            None
        );
        assert_eq!(
            parse_tag_suppression("remote.my.fork.tagopt --no-tags\n")
                .expect("parse")
                .as_deref(),
            Some("my.fork")
        );
        for unstored in [
            "remote.origin.fetch +refs/*:refs/remotes/origin/*\nremote.origin.tagopt --no-tags\n",
            "remote.origin.fetch +refs/tags/*:refs/tags/*\nremote.origin.fetch ^refs/tags/v1\nremote.origin.tagopt --no-tags\n",
        ] {
            assert_eq!(
                parse_tag_suppression(unstored).expect("parse").as_deref(),
                Some("origin"),
                "{unstored:?}"
            );
        }
    }

    #[test]
    fn parse_tag_suppression_finds_a_no_tags_remote() {
        let found = parse_tag_suppression("remote.origin.tagopt --no-tags\n").expect("parse");
        assert_eq!(found.as_deref(), Some("origin"));
        let found = parse_tag_suppression(
            "remote.origin.tagopt --tags\nremote.upstream.tagopt --no-tags\n",
        )
        .expect("parse");
        assert_eq!(found.as_deref(), Some("upstream"));
    }

    #[test]
    fn parse_tag_suppression_accepts_fetching_configs() {
        assert_eq!(parse_tag_suppression("").expect("empty"), None);
        assert_eq!(
            parse_tag_suppression("remote.origin.tagopt --tags\n").expect("tags"),
            None
        );
    }

    #[test]
    fn parse_tag_suppression_honors_last_value_per_remote() {
        let overridden =
            parse_tag_suppression("remote.origin.tagopt --no-tags\nremote.origin.tagopt --tags\n")
                .expect("parse");
        assert_eq!(
            overridden, None,
            "a local --tags overrides a global --no-tags"
        );
        let suppressed =
            parse_tag_suppression("remote.origin.tagopt --tags\nremote.origin.tagopt --no-tags\n")
                .expect("parse");
        assert_eq!(suppressed.as_deref(), Some("origin"));
    }

    #[test]
    fn shell_quote_neutralizes_metacharacters_and_leaves_plain_names_bare() {
        assert_eq!(shell_quote("origin"), "origin");
        assert_eq!(shell_quote("remote.origin.tagOpt"), "remote.origin.tagOpt");
        assert_eq!(shell_quote("foo$(cmd)"), "'foo$(cmd)'");
        assert_eq!(shell_quote("fo'o"), r"'fo'\''o'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("HEAD~1"), "HEAD~1");
        assert_eq!(shell_quote("~1"), "'~1'");
        assert_eq!(
            shell_quote("HEAD@{2026-10-23 17:00:00 +0000}"),
            "'HEAD@{2026-10-23 17:00:00 +0000}'"
        );
    }

    #[test]
    fn parse_tag_suppression_rejects_unparseable_lines() {
        let err = parse_tag_suppression("garbage\n").expect_err("garbage");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err = parse_tag_suppression("remote..tagopt --no-tags\n").expect_err("empty name");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn parse_is_shallow_rejects_anything_but_true_or_false() {
        assert!(parse_is_shallow("true\n").expect("true"));
        assert!(!parse_is_shallow("false\n").expect("false"));
        let err = parse_is_shallow("yes\n").expect_err("yes");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn parse_ls_remote_tags_skips_peeled_suffix_and_keeps_the_name() {
        let names = parse_ls_remote_tags(
            "abc\trefs/tags/v0.1.0\n\
             abc\trefs/tags/v0.1.0^{}\n\
             def\trefs/tags/pkg/v0.2.0\n",
        )
        .expect("parse");
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["pkg/v0.2.0", "v0.1.0"]
        );
        let peeled_only = parse_ls_remote_tags("abc\trefs/tags/v0.1.0^{}\n").expect("peeled-only");
        assert_eq!(peeled_only.into_iter().collect::<Vec<_>>(), vec!["v0.1.0"]);
        let empty = parse_ls_remote_tags("").expect("empty look");
        assert!(empty.is_empty());
        let commits = parse_ls_remote_tag_commits(
            "tagobj\trefs/tags/v0.1.0\n\
             commit\trefs/tags/v0.1.0^{}\n",
        )
        .expect("peeled wins");
        assert_eq!(commits.get("v0.1.0").map(Commit::as_str), Some("commit"));
    }

    #[test]
    fn points_at_requires_both_tag_and_commit() {
        let advertised = parse_ls_remote_tag_commits("aaa\trefs/tags/v1\n").expect("parse");
        let aaa = Commit::from_advertised("aaa").expect("aaa");
        let bbb = Commit::from_advertised("bbb").expect("bbb");
        assert!(advertised.points_at("v1", &aaa));
        assert!(!advertised.points_at("v1", &bbb));
        assert!(!advertised.points_at("v2", &aaa));
    }

    #[test]
    fn parse_ls_remote_tags_rejects_empty_commit_id() {
        let err = parse_ls_remote_tags("\trefs/tags/v0.1.0\n").expect_err("empty sha");
        assert!(err.to_string().contains("empty commit"), "{err}");
    }

    #[test]
    fn parse_ls_remote_tags_rejects_malformed_lines() {
        let err = parse_ls_remote_tags("no-tab\n").expect_err("no tab");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err = parse_ls_remote_tags("abc\trefs/heads/main\n").expect_err("not a tag");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err = parse_ls_remote_tags("abc\trefs/tags/\n").expect_err("empty name");
        assert!(err.to_string().contains("unverified"), "{err}");
        let err = parse_ls_remote_tags("abc\trefs/tags/^{}\n").expect_err("peeled empty");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn preferred_remote_picks_origin_even_when_it_is_not_first() {
        assert_eq!(
            preferred_remote("extra\norigin\n").as_deref(),
            Some("origin")
        );
        assert_eq!(preferred_remote("upstream\n").as_deref(), Some("upstream"));
        assert_eq!(preferred_remote(""), None);
    }
}
