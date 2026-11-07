//! Both sides of a Cargo workspace's root `Cargo.toml` and `Cargo.lock`, read
//! for [`root_changes`] (ADR-0039), which compares them.

use std::collections::{BTreeMap, BTreeSet};

use oakum::manifest::{
    declared_members, root_changes, CargoConfig, Declarations, RootChanges, RootFile, RootFiles,
    RootReason,
};
use oakum::plan::{Ecosystem, Package, PackageId, Workspace};
use oakum::state::Through;

use super::git::{Commit, Followed, Git};
use super::CliError;

#[derive(Debug)]
struct Paths {
    manifest: String,
    lock: String,
}

impl Paths {
    fn of(workspace: &Workspace) -> Option<Self> {
        let root = workspace.cargo_workspace_root()?;
        let at = |name: &str| {
            if root.is_empty() {
                name.to_owned()
            } else {
                format!("{root}/{name}")
            }
        };
        Some(Self {
            manifest: at("Cargo.toml"),
            lock: at("Cargo.lock"),
        })
    }

    fn path(&self, file: RootFile) -> &str {
        match file {
            RootFile::Manifest => &self.manifest,
            RootFile::Lock => &self.lock,
        }
    }
}

/// What a change to the root files reached; empty when neither changed.
#[derive(Debug, Default)]
pub(super) struct RootReach {
    changes: RootChanges,
    paths: Option<Paths>,
}

impl RootReach {
    /// The packages to walk [`oakum::plan::reach`] from: the members a root
    /// change named, and for a workspace-wide one every Cargo package `managed`
    /// accepts.
    pub(super) fn changed(
        &self,
        workspace: &Workspace,
        managed: impl Fn(&Package) -> bool,
    ) -> BTreeSet<PackageId> {
        let mut changed: BTreeSet<PackageId> = self.changes.members.keys().cloned().collect();
        if !self.changes.everywhere.is_empty() {
            changed.extend(
                cargo_packages(workspace)
                    .filter(|package| managed(package))
                    .map(|package| package.id().clone()),
            );
        }
        changed
    }

    /// Each managed package a root change reached, with every reason that
    /// reached it: a lock row through one internal crate and a manifest entry
    /// through another both land on the package that ships them.
    pub(super) fn through(
        &self,
        workspace: &Workspace,
        managed: impl Fn(&Package) -> bool,
    ) -> BTreeMap<PackageId, Vec<Through>> {
        let Some(paths) = &self.paths else {
            return BTreeMap::new();
        };
        let through = |reason: &RootReason| Through {
            file: paths.path(reason.file).to_owned(),
            name: reason.name.clone(),
        };
        let mut out: BTreeMap<PackageId, BTreeSet<Through>> = BTreeMap::new();
        for (origin, reasons) in &self.changes.members {
            for hit in oakum::plan::reach(workspace, [origin], &managed)
                .managed
                .into_keys()
            {
                out.entry(hit)
                    .or_default()
                    .extend(reasons.iter().map(through));
            }
        }
        if !self.changes.everywhere.is_empty() {
            for package in cargo_packages(workspace).filter(|package| managed(package)) {
                out.entry(package.id().clone())
                    .or_default()
                    .extend(self.changes.everywhere.iter().map(through));
            }
        }
        out.into_iter()
            .map(|(id, through)| (id, through.into_iter().collect()))
            .collect()
    }
}

/// Whether `changed` lists the workspace's root `Cargo.toml` or `Cargo.lock`,
/// asked before the reads that compare them.
pub(super) fn touched(workspace: &Workspace, changed: &[String]) -> bool {
    Paths::of(workspace).is_some_and(|paths| {
        changed
            .iter()
            .any(|path| *path == paths.manifest || *path == paths.lock)
    })
}

fn cargo_packages(workspace: &Workspace) -> impl Iterator<Item = &Package> {
    workspace
        .packages()
        .filter(|package| package.id().ecosystem == Ecosystem::Cargo)
}

/// `base` against `head` for the root files among `changed`.
///
/// # Errors
///
/// A read git could not answer, or a side that does not parse: `unverified`,
/// since what the file changed is then unknown.
pub(super) fn between_commits(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    head: &Commit,
    changed: &[String],
) -> Result<RootReach, CliError> {
    compare(git, workspace, base, changed, |wanted| {
        git.blobs_or_absent(head, wanted)
    })
}

/// A commit against its first parent; a root commit's every root file is new.
///
/// # Errors
///
/// As [`between_commits`].
pub(super) fn for_commit(
    git: &Git,
    workspace: &Workspace,
    hash: &str,
    changed: &[String],
) -> Result<RootReach, CliError> {
    let commit = Commit::from_log(hash)?;
    between_commits(git, workspace, &commit.first_parent(), &commit, changed)
}

/// `base` against the working tree: a root file in `on_disk` is read by
/// `read_disk`, any other at `head`.
///
/// # Errors
///
/// As [`between_commits`], or `read_disk`'s own failure.
pub(super) fn against_worktree(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    head: &Commit,
    changed: &[String],
    on_disk: &[String],
    read_disk: impl Fn(&str) -> Result<Option<String>, CliError>,
) -> Result<RootReach, CliError> {
    compare(git, workspace, base, changed, |wanted| {
        let committed = git.blobs_or_absent(head, wanted)?;
        wanted
            .iter()
            .zip(committed)
            .map(|(path, committed)| {
                if on_disk.iter().any(|listed| listed == path) {
                    read_disk(path)
                } else {
                    Ok(committed)
                }
            })
            .collect()
    })
}

/// `texts` holds one entry per `wanted` path; a root file not wanted reads as
/// absent on both sides, which compares as unchanged.
fn side<'a>(paths: &Paths, wanted: &[&str], texts: &'a [Option<String>]) -> RootFiles<'a> {
    let text = |path: &str| {
        wanted
            .iter()
            .position(|wanted| *wanted == path)
            .and_then(|at| texts[at].as_deref())
    };
    RootFiles {
        manifest: text(&paths.manifest),
        lock: text(&paths.lock),
    }
}

fn compare(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    changed: &[String],
    head: impl FnOnce(&[&str]) -> Result<Vec<Option<String>>, CliError>,
) -> Result<RootReach, CliError> {
    let Some(paths) = Paths::of(workspace) else {
        return Ok(RootReach::default());
    };
    let wanted: Vec<&str> = [paths.manifest.as_str(), paths.lock.as_str()]
        .into_iter()
        .filter(|path| changed.iter().any(|changed| changed == path))
        .collect();
    if wanted.is_empty() {
        return Ok(RootReach::default());
    }
    let base_texts = git.blobs_or_absent(base, &wanted)?;
    let head_texts = head(&wanted)?;
    let base_members = if wanted.contains(&paths.lock.as_str()) {
        declared_at_base(git, workspace, base, &paths)?
    } else {
        BTreeMap::new()
    };
    let found = root_changes(
        workspace,
        &base_members,
        side(&paths, &wanted, &base_texts),
        side(&paths, &wanted, &head_texts),
    )
    .map_err(|err| {
        CliError::unverified(format!(
            "unverified: `{}` does not parse on one side of the comparison, so what it changed is unknown: {}",
            paths.path(err.file),
            err.message
        ))
    })?;
    Ok(RootReach {
        changes: found,
        paths: Some(paths),
    })
}

/// Each Cargo member as `base`'s manifests declare it, read from git so no
/// cargo runs on that side.
fn declared_at_base(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    paths: &Paths,
) -> Result<BTreeMap<PackageId, Declarations>, CliError> {
    let members: Vec<&Package> = cargo_packages(workspace).collect();
    let manifests: Vec<String> = members
        .iter()
        .map(|package| package.manifest_path())
        .collect();
    let configs = config_paths(workspace.cargo_workspace_root().unwrap_or_default());
    let wanted: Vec<&str> = std::iter::once(paths.manifest.as_str())
        .chain(manifests.iter().map(String::as_str))
        .chain(configs.iter().map(String::as_str))
        .collect();
    let texts = through_links(git, base, &wanted, git.blobs_or_absent(base, &wanted)?)?;
    let (root, rest) = texts.split_first().unwrap_or((&None, &[]));
    let (manifest_texts, config_texts) = rest.split_at(manifests.len());
    let config = CargoConfig::new(
        nearest_configs(config_texts)
            .into_iter()
            .chain(home_config().as_deref()),
        registry_variables(),
    );
    let manifests: Vec<(&Package, Option<&str>)> = members
        .into_iter()
        .zip(manifest_texts.iter().map(Option::as_deref))
        .collect();
    declared_members(workspace, root.as_deref(), &manifests, &config).map_err(|err| {
        CliError::unverified(format!(
            "unverified: `{}` does not parse at the base, so which `{}` rows the members shipped there is unknown: {}",
            err.path, paths.lock, err.message
        ))
    })
}

/// git holds a symlink as its target's path, which is no TOML, so such a text is
/// read again through the link; a link reaching no file in `base` declares
/// nothing there.
fn through_links(
    git: &Git,
    base: &Commit,
    paths: &[&str],
    texts: Vec<Option<String>>,
) -> Result<Vec<Option<String>>, CliError> {
    paths
        .iter()
        .zip(texts)
        .map(|(path, text)| match text {
            Some(text) if text.parse::<toml_edit::DocumentMut>().is_err() => {
                Ok(match git.followed_blob(base, path)? {
                    Followed::File(bytes) => Some(String::from_utf8(bytes).unwrap_or(text)),
                    Followed::Absent | Followed::Dangling | Followed::Unfollowed(_) => None,
                })
            }
            text => Ok(text),
        })
        .collect()
}

/// `.cargo/config` and `.cargo/config.toml` in `root` and each directory above
/// it in the repository, nearest first, in pairs.
fn config_paths(root: &str) -> Vec<String> {
    let mut dir = root;
    let mut paths = Vec::new();
    loop {
        for name in [".cargo/config", ".cargo/config.toml"] {
            paths.push(if dir.is_empty() {
                name.to_owned()
            } else {
                format!("{dir}/{name}")
            });
        }
        if dir.is_empty() {
            return paths;
        }
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

/// One text a directory, from [`config_paths`]' pairs: cargo reads
/// `.cargo/config` over `.cargo/config.toml` when both exist.
fn nearest_configs(texts: &[Option<String>]) -> Vec<&str> {
    texts
        .chunks(2)
        .filter_map(|pair| pair.iter().find_map(Option::as_deref))
        .collect()
}

/// `$CARGO_HOME`'s config, read from disk: it is no side's, and both share it.
fn home_config() -> Option<String> {
    let home = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".cargo")))?;
    ["config", "config.toml"]
        .into_iter()
        .find_map(|name| std::fs::read_to_string(home.join(name)).ok())
}

fn registry_variables() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, _)| name.starts_with("CARGO_REGISTRIES_") && name.ends_with("_INDEX"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_are_read_from_the_workspace_root_up() {
        assert_eq!(
            config_paths("rust/ws"),
            [
                "rust/ws/.cargo/config",
                "rust/ws/.cargo/config.toml",
                "rust/.cargo/config",
                "rust/.cargo/config.toml",
                ".cargo/config",
                ".cargo/config.toml",
            ]
        );
        assert_eq!(config_paths(""), [".cargo/config", ".cargo/config.toml"]);
    }

    #[test]
    fn a_directory_holding_both_configs_is_read_from_the_bare_one() {
        let texts = [
            Some(String::from("bare")),
            Some(String::from("toml")),
            None,
            Some(String::from("parent")),
            None,
            None,
        ];
        assert_eq!(nearest_configs(&texts), ["bare", "parent"]);
    }
}
