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
pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let repo = std::env::current_dir()?;
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
            return Err(CliError::new(
                "unverified: empty commit for a reachable tag group",
            ));
        }
        if tags.is_empty() {
            return Err(CliError::new(
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
        return Err(CliError::new(
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
        return Err(CliError::new(format!(
            "unverified: remote {remote:?} is configured with tagOpt --no-tags, so this clone \
             does not fetch tags; run `git config --replace-all {key} --tags`, then \
             `git fetch --tags -- {name}` before reading tags{quoting_note}"
        )));
    }
    let names = reachable_tag_names(repo)?;
    let mut pairs = Vec::new();
    for name in names {
        let commit = peel_to_commit(repo, &name)?;
        pairs.push((commit, name));
    }
    group_pairs(pairs)
}

fn reachable_tag_names(repo: &Path) -> Result<Vec<String>, CliError> {
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--merged=HEAD",
            "--format=%(refname)",
            "refs/tags",
        ])
        .current_dir(repo)
        .output()
        .map_err(|err| {
            CliError::new(format!("unverified: failed to run git for-each-ref: {err}"))
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!(
            "unverified: git for-each-ref --merged HEAD failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new("unverified: git for-each-ref output was not valid UTF-8"))?;
    let mut names = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        if line.trim().is_empty() {
            return Err(CliError::new(format!(
                "unverified: unparseable tag ref: {line:?}"
            )));
        }
        let Some(name) = line.strip_prefix("refs/tags/") else {
            return Err(CliError::new(format!(
                "unverified: unparseable tag ref: {line}"
            )));
        };
        if name.is_empty() {
            return Err(CliError::new(format!(
                "unverified: unparseable tag ref: {line}"
            )));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn peel_to_commit(repo: &Path, name: &str) -> Result<String, CliError> {
    let spec = format!("refs/tags/{name}^{{commit}}");
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "--end-of-options", &spec])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("unverified: failed to run git rev-parse: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!(
            "unverified: git rev-parse {spec} failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new("unverified: git rev-parse output was not valid UTF-8"))?;
    let commit = stdout.trim();
    if commit.is_empty() {
        return Err(CliError::new(format!(
            "unverified: git rev-parse {spec} returned an empty commit"
        )));
    }
    Ok(commit.to_owned())
}

fn is_shallow(repo: &Path) -> Result<bool, CliError> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-shallow-repository"])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("unverified: failed to run git rev-parse: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!(
            "unverified: git rev-parse --is-shallow-repository failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        CliError::new(
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
        .map_err(|err| CliError::new(format!("unverified: failed to run git config: {err}")))?;
    if !output.status.success() {
        // git config exits 1 when no key matches the pattern.
        if output.status.code() == Some(1) && output.stdout.is_empty() {
            return Ok(None);
        }
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::new(format!(
            "unverified: git config --get-regexp tagopt failed: {err}"
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new("unverified: git config output was not valid UTF-8"))?;
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
                CliError::new(format!("unverified: unparseable git config line: {line:?}"))
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
        other => Err(CliError::new(format!(
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
            return Err(CliError::new(
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
}
