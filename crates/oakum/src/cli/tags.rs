//! Tags reachable from HEAD. ADR-0014: `git for-each-ref --merged HEAD`, not
//! every tag in the repository. A maintenance branch's history is its release
//! line; listing all tags would mix in `main`'s.
//!
//! Git I/O stays in the binary (ADR-0002). `oakum::tags` parses names; it does
//! not talk to git.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use super::CliError;

/// Uses the process cwd so git, not oakum, walks to `.git`. Plumbing tests
/// can point at a non-repo and get a failure instead of the parent checkout.
pub(super) fn run() -> Result<(), CliError> {
    let repo = std::env::current_dir().map_err(|err| CliError::unverified(err.to_string()))?;
    for group in reachable_tags(&repo)? {
        for tag in group.tags() {
            println!("{}\t{tag}", group.commit());
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
pub(crate) fn reachable_tags(repo: &Path) -> Result<Vec<CommitTags>, CliError> {
    if is_shallow(repo)? {
        return Err(CliError::unverified(
            "unverified: shallow clone; fetch full history before reading tags",
        ));
    }
    if let Some(remote) = tag_suppressed_remote(repo)? {
        // A local `--tags` override clears suppression wherever it lives
        // (clone-written local key, global, or system config); an unscoped
        // `--unset` only clears a local value.
        let key = shell_quote(&format!("remote.{remote}.tagOpt"));
        let name = shell_quote(&remote);
        // Quoting only appears for names carrying metacharacters, where the
        // command is POSIX-specific; say so rather than pasting it broken
        // into cmd.exe or PowerShell.
        let quoting_note = if key.contains('\'') || name.contains('\'') {
            " (commands use POSIX shell quoting; adapt for cmd.exe or PowerShell)"
        } else {
            ""
        };
        return Err(CliError::unverified(format!(
            "unverified: remote {remote:?} is configured with tagOpt --no-tags, so this clone \
             does not fetch tags; run `git config --replace-all {key} --tags`, then \
             `git fetch --tags -- {name}` before reading tags{quoting_note}"
        )));
    }
    let pairs = reachable_tag_records(repo)?;
    group_pairs(pairs)
}

/// One query returns every reachable tag with its peeled identity, so
/// discovery stays at a fixed number of Git child processes no matter how
/// many tags the repository carries.
fn reachable_tag_records(repo: &Path) -> Result<Vec<(String, String)>, CliError> {
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--merged=HEAD",
            "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)",
            "refs/tags",
        ])
        .current_dir(repo)
        .output()
        .map_err(|err| {
            CliError::unverified(format!("unverified: failed to run git for-each-ref: {err}"))
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::unverified(format!(
            "unverified: git for-each-ref --merged HEAD failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        CliError::unverified("unverified: git for-each-ref output was not valid UTF-8")
    })?;
    parse_ref_records(&stdout)
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

fn is_shallow(repo: &Path) -> Result<bool, CliError> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-shallow-repository"])
        .current_dir(repo)
        .output()
        .map_err(|err| {
            CliError::unverified(format!("unverified: failed to run git rev-parse: {err}"))
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::unverified(format!(
            "unverified: git rev-parse --is-shallow-repository failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        CliError::unverified(
            "unverified: git rev-parse --is-shallow-repository output was not valid UTF-8",
        )
    })?;
    parse_is_shallow(&stdout)
}

/// Quotes a value for the copy-pasteable diagnostic. Plain names stay bare
/// so the common command also pastes cleanly into non-POSIX shells (cmd.exe,
/// PowerShell); anything else gets POSIX single-quoting.
fn shell_quote(value: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/');
    if !value.is_empty() && value.chars().all(plain) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A clone made with `git clone --no-tags` is non-shallow but records
/// `remote.<name>.tagOpt = --no-tags`, so its empty tag list means "we did
/// not fetch", not "never released". Reading the effective config also
/// catches the setting applied after the clone.
fn tag_suppressed_remote(repo: &Path) -> Result<Option<String>, CliError> {
    let output = Command::new("git")
        .args(["config", "--get-regexp", r"^remote\..*\.tagopt$"])
        .current_dir(repo)
        .output()
        .map_err(|err| {
            CliError::unverified(format!("unverified: failed to run git config: {err}"))
        })?;
    if !output.status.success() {
        // git config exits 1 when no key matches the pattern.
        if output.status.code() == Some(1) && output.stdout.is_empty() {
            return Ok(None);
        }
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::unverified(format!(
            "unverified: git config --get-regexp tagopt failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| CliError::unverified("unverified: git config output was not valid UTF-8"))?;
    parse_tag_suppression(&stdout)
}

/// First remote whose effective `tagOpt` suppresses tag fetching, from
/// `git config --get-regexp` output (`remote.<name>.tagopt <value>` lines).
/// Values print in ascending precedence order (system, global, local), so the
/// last value per remote is the effective one — a global `--no-tags`
/// overridden by a local `--tags` does not suppress.
fn parse_tag_suppression(stdout: &str) -> Result<Option<String>, CliError> {
    let mut effective: BTreeMap<String, String> = BTreeMap::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let name = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".tagopt"))
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                CliError::unverified(format!("unverified: unparseable git config line: {line:?}"))
            })?;
        effective.insert(name.to_owned(), value.to_owned());
    }
    Ok(effective
        .into_iter()
        .find(|(_, value)| value == "--no-tags")
        .map(|(name, _)| name))
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
pub(crate) fn remote_tag_names(repo: &Path, remote: &str) -> Result<BTreeSet<String>, CliError> {
    let output = Command::new("git")
        .args(["ls-remote", "--tags", "--", remote])
        .current_dir(repo)
        .output()
        .map_err(|err| {
            CliError::unverified(format!("unverified: failed to run git ls-remote: {err}"))
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::unverified(format!(
            "unverified: git ls-remote --tags {remote} failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        CliError::unverified("unverified: git ls-remote output was not valid UTF-8")
    })?;
    parse_ls_remote_tags(&stdout)
}

pub(crate) fn first_remote(repo: &Path) -> Result<Option<String>, CliError> {
    let output = Command::new("git")
        .args(["remote"])
        .current_dir(repo)
        .output()
        .map_err(|err| {
            CliError::unverified(format!("unverified: failed to run git remote: {err}"))
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::unverified(format!(
            "unverified: git remote failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| CliError::unverified("unverified: git remote output was not valid UTF-8"))?;
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
pub(crate) fn parse_ls_remote_tags(stdout: &str) -> Result<BTreeSet<String>, CliError> {
    let mut names = BTreeSet::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((_, reference)) = line.split_once('\t') else {
            return Err(CliError::unverified(format!(
                "unverified: unparseable ls-remote line {line:?}"
            )));
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            return Err(CliError::unverified(format!(
                "unverified: ls-remote ref is not a tag: {reference:?}"
            )));
        };
        let name = name.strip_suffix("^{}").unwrap_or(name);
        if name.is_empty() {
            return Err(CliError::unverified(
                "unverified: ls-remote advertised an empty tag name",
            ));
        }
        names.insert(String::from(name));
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

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
