//! Repository-state snapshots for write-ownership integration tests (okm-aib).
//!
//! Detects mutations to manifests, lockfiles, and git metadata that a command
//! was not supposed to make.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::fixture::git_output;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    worktree: BTreeMap<PathBuf, Vec<u8>>,
    git_config: Option<Vec<u8>>,
    hooks: BTreeSet<String>,
    git_head: String,
    git_porcelain: String,
    git_tags: BTreeSet<String>,
}

impl RepoState {
    pub fn capture(root: &Path) -> Self {
        let Some(common) = git_common_dir(root) else {
            return Self {
                worktree: collect_worktree(root),
                git_config: None,
                hooks: BTreeSet::new(),
                git_head: String::new(),
                git_porcelain: String::new(),
                git_tags: BTreeSet::new(),
            };
        };
        let (git_head, git_porcelain) = capture_git(root);
        Self {
            worktree: collect_worktree(root),
            git_config: read_optional(&common.join("config")),
            hooks: collect_hooks(&common),
            git_head,
            git_porcelain,
            git_tags: capture_git_tags(root),
        }
    }

    pub fn assert_unchanged(before: &Self, root: &Path, context: &str) {
        let after = Self::capture(root);
        assert_eq!(before, &after, "{context}: repository state changed");
    }

    pub fn assert_only_new_files(before: &Self, root: &Path, allowed: &[PathBuf], context: &str) {
        let after = Self::capture(root);
        let delta = before.diff(&after);
        assert!(
            delta.removed.is_empty()
                && delta.modified.is_empty()
                && delta.git_write_metadata_unchanged
                && delta.git_tags_unchanged,
            "{context}: unexpected worktree or git change: {delta:?}"
        );
        let allowed: BTreeSet<_> = allowed.iter().cloned().collect();
        assert_eq!(
            delta.added, allowed,
            "{context}: worktree delta must match the allowlist"
        );
    }

    pub fn assert_allowed_delta(
        before: &Self,
        root: &Path,
        allowed_added: &[PathBuf],
        allowed_modified: &[PathBuf],
        allowed_removed: &[PathBuf],
        allowed_new_tags: &[String],
        context: &str,
    ) {
        let after = Self::capture(root);
        let delta = before.diff(&after);
        assert!(
            delta.git_write_metadata_unchanged,
            "{context}: git config, hooks, or HEAD changed: {delta:?}"
        );

        if delta.added.is_empty() && delta.modified.is_empty() && delta.removed.is_empty() {
            assert!(
                delta.git_porcelain_unchanged,
                "{context}: git porcelain changed with no worktree byte delta: {delta:?}"
            );
        }

        let added: BTreeSet<_> = allowed_added.iter().cloned().collect();
        let modified: BTreeSet<_> = allowed_modified.iter().cloned().collect();
        let removed: BTreeSet<_> = allowed_removed.iter().cloned().collect();
        assert_eq!(delta.added, added, "{context}: unexpected added paths");
        assert_eq!(
            delta.modified, modified,
            "{context}: unexpected modified paths"
        );
        assert_eq!(
            delta.removed, removed,
            "{context}: unexpected removed paths"
        );

        let mut expected_tags = before.git_tags.clone();
        for tag in allowed_new_tags {
            assert!(expected_tags.insert(tag.clone()), "duplicate tag {tag}");
        }
        assert_eq!(
            after.git_tags, expected_tags,
            "{context}: unexpected tag delta"
        );
    }
}

#[derive(Debug)]
struct Delta {
    added: BTreeSet<PathBuf>,
    removed: BTreeSet<PathBuf>,
    modified: BTreeSet<PathBuf>,
    git_write_metadata_unchanged: bool,
    git_porcelain_unchanged: bool,
    git_tags_unchanged: bool,
}

impl RepoState {
    fn diff(&self, other: &Self) -> Delta {
        let mut added = BTreeSet::new();
        let mut removed = BTreeSet::new();
        let mut modified = BTreeSet::new();

        for key in self
            .worktree
            .keys()
            .chain(other.worktree.keys())
            .collect::<BTreeSet<_>>()
        {
            match (self.worktree.get(key), other.worktree.get(key)) {
                (None, Some(_)) => {
                    added.insert(key.clone());
                }
                (Some(_), None) => {
                    removed.insert(key.clone());
                }
                (Some(left), Some(right)) if left != right => {
                    modified.insert(key.clone());
                }
                _ => {}
            }
        }

        Delta {
            added,
            removed,
            modified,
            git_write_metadata_unchanged: self.git_config == other.git_config
                && self.hooks == other.hooks
                && self.git_head == other.git_head,
            git_porcelain_unchanged: self.git_porcelain == other.git_porcelain,
            git_tags_unchanged: self.git_tags == other.git_tags,
        }
    }
}

/// Where config and hooks live, or `None` when `root` holds no repository:
/// no `.git`, or a `.git` directory without `HEAD`, as the suites fake one.
/// A `.git` file (a linked worktree, `--separate-git-dir`) is asked of git.
fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.join("HEAD").is_file() {
        return Some(dot_git);
    }
    if !dot_git.is_file() {
        return None;
    }
    let common = answered(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    Some(PathBuf::from(common.trim()))
}

fn capture_git_tags(root: &Path) -> BTreeSet<String> {
    answered(root, &["tag", "-l"])
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// HEAD and porcelain. An unborn HEAD reads empty but still has an index to
/// compare.
fn capture_git(root: &Path) -> (String, String) {
    let head = git_output(root, &["rev-parse", "--verify", "-q", "HEAD"]);
    let head = match head.status.code() {
        Some(0) => String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        Some(1) => String::new(),
        _ => could_not_read(root, &["rev-parse", "--verify", "-q", "HEAD"], &head),
    };
    (head, answered(root, &["status", "--porcelain"]))
}

/// Panics rather than returning empty: two snapshots that both failed to look
/// would compare equal.
fn answered(root: &Path, args: &[&str]) -> String {
    let output = git_output(root, args);
    if !output.status.success() {
        could_not_read(root, args, &output);
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn could_not_read(root: &Path, args: &[&str], output: &std::process::Output) -> ! {
    panic!(
        "git could not read the repository at {}: `git {}` exited {}: {}",
        root.display(),
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn collect_worktree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    walk_worktree(root, root, &mut files).expect("walk worktree");
    files
}

fn walk_worktree(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_worktree(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("worktree path stays under root");
            assert_single_component_segments(rel);
            out.insert(rel.to_path_buf(), fs::read(&path)?);
        }
    }
    Ok(())
}

fn collect_hooks(common: &Path) -> BTreeSet<String> {
    let hooks_dir = common.join("hooks");
    let Ok(entries) = fs::read_dir(hooks_dir) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".sample") {
                return None;
            }
            entry.file_type().ok().filter(std::fs::FileType::is_file)?;
            Some(name)
        })
        .collect()
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => panic!("read {}: {err}", path.display()),
    }
}

fn assert_single_component_segments(rel: &Path) {
    for component in rel.components() {
        assert!(
            matches!(component, Component::Normal(_)),
            "repo-state paths must be normal segments, got {}",
            rel.display()
        );
    }
}
