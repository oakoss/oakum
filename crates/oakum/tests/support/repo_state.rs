//! Repository-state snapshots for write-ownership integration tests (okm-aib).
//!
//! Captures the worktree, repository-local git config, hooks, and HEAD/status
//! so a command cannot mutate manifests, lockfiles, or git metadata without a
//! test noticing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::fixture::git_output;

/// Observed repository state at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    worktree: BTreeMap<PathBuf, Vec<u8>>,
    git_config: Option<Vec<u8>>,
    hooks: BTreeSet<String>,
    git_head: String,
    git_porcelain: String,
}

impl RepoState {
    /// Capture every file outside `.git`, plus `.git/config`, hooks, and HEAD/status.
    pub fn capture(root: &Path) -> Self {
        let (git_head, git_porcelain) = capture_git(root);
        Self {
            worktree: collect_worktree(root),
            git_config: read_optional(&root.join(".git/config")),
            hooks: collect_hooks(root),
            git_head,
            git_porcelain,
        }
    }

    /// Fail when anything in the observed state differs from `before`.
    pub fn assert_unchanged(before: &Self, root: &Path, context: &str) {
        let after = Self::capture(root);
        assert_eq!(before, &after, "{context}: repository state changed");
    }

    /// Fail when the delta is not exactly the listed new worktree paths.
    pub fn assert_only_new_files(before: &Self, root: &Path, allowed: &[PathBuf], context: &str) {
        let after = Self::capture(root);
        let delta = before.diff(&after);
        assert!(
            delta.removed.is_empty()
                && delta.modified.is_empty()
                && delta.git_write_metadata_unchanged,
            "{context}: unexpected worktree or git metadata change: {delta:?}"
        );
        let allowed: BTreeSet<_> = allowed.iter().cloned().collect();
        assert_eq!(
            delta.added, allowed,
            "{context}: worktree delta must match the allowlist"
        );
    }
}

#[derive(Debug)]
struct Delta {
    added: BTreeSet<PathBuf>,
    removed: BTreeSet<PathBuf>,
    modified: BTreeSet<PathBuf>,
    git_metadata_unchanged: bool,
    git_write_metadata_unchanged: bool,
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
            git_metadata_unchanged: self.git_config == other.git_config
                && self.hooks == other.hooks
                && self.git_head == other.git_head
                && self.git_porcelain == other.git_porcelain,
            git_write_metadata_unchanged: self.git_config == other.git_config
                && self.hooks == other.hooks
                && self.git_head == other.git_head,
        }
    }
}

fn capture_git(root: &Path) -> (String, String) {
    if !root.join(".git/HEAD").is_file() {
        return (String::new(), String::new());
    }
    let head = git_output(root, &["rev-parse", "HEAD"]);
    if !head.status.success() {
        return (String::new(), String::new());
    }
    let porcelain = git_output(root, &["status", "--porcelain"]);
    if !porcelain.status.success() {
        return (String::new(), String::new());
    }
    (
        String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        String::from_utf8_lossy(&porcelain.stdout).into_owned(),
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

fn collect_hooks(root: &Path) -> BTreeSet<String> {
    let hooks_dir = root.join(".git/hooks");
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
