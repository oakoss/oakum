//! Stable repository identity shared by CLI commands.
//!
//! The held [`Dir`] is the repository. [`Self::path`] is the ambient name it
//! was opened from, used as the containment prefix for absolute symlink
//! targets. Subprocess callers go through [`Self::ambient_path`], which
//! refuses if that name no longer refers to the same directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

use super::CliError;

pub(super) struct Repository {
    path: PathBuf,
    dir: Dir,
}

impl Repository {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn dir(&self) -> &Dir {
        &self.dir
    }

    /// Path for subprocesses (`cargo`, `pnpm`, `git`). Fails closed if a
    /// rename or replacement has made this name point at a different tree.
    pub(super) fn ambient_path(&self) -> Result<&Path, Box<dyn std::error::Error>> {
        confirm_ambient(&self.dir, &self.path)?;
        Ok(&self.path)
    }
}

/// Git children call this at spawn so a replacement after `Git::at_repository`
/// cannot split git I/O onto a different tree.
pub(super) fn confirm_ambient(dir: &Dir, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let held = dir.dir_metadata().map_err(|err| {
        CliError::new(format!(
            "failed to inspect the repository capability: {err}"
        ))
    })?;
    let ambient = fs::metadata(path).map_err(|err| {
        CliError::new(format!(
            "repository root is no longer the directory originally opened ({}): {err}",
            path.display()
        ))
    })?;
    if !same_identity(&held, &ambient) {
        return Err(Box::new(CliError::new(format!(
            "repository root is no longer the directory originally opened ({})",
            path.display()
        ))));
    }
    Ok(())
}

pub(super) fn discover() -> Result<Repository, Box<dyn std::error::Error>> {
    discover_from(&std::env::current_dir()?)
}

pub(super) fn discover_from(start: &Path) -> Result<Repository, Box<dyn std::error::Error>> {
    let start_path = fs::canonicalize(start)?;
    let start_dir = Dir::open_ambient_dir(&start_path, ambient_authority())?;
    let mut path = start_path.clone();
    let mut dir = start_dir.try_clone()?;
    loop {
        match dir.symlink_metadata(".git") {
            Ok(_) => return Ok(Repository { path, dir }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(Box::new(err)),
        }
        if !path.pop() {
            return Ok(Repository {
                path: start_path,
                dir: start_dir,
            });
        }
        dir = dir.open_parent_dir(ambient_authority())?;
    }
}

#[cfg(unix)]
fn same_identity(held: &cap_std::fs::Metadata, ambient: &std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt as CapExt;
    use std::os::unix::fs::MetadataExt as StdExt;
    CapExt::dev(held) == StdExt::dev(ambient) && CapExt::ino(held) == StdExt::ino(ambient)
}

#[cfg(not(unix))]
fn same_identity(held: &cap_std::fs::Metadata, ambient: &std::fs::Metadata) -> bool {
    held.is_dir() && ambient.is_dir() && same_mtime(held.modified(), ambient.modified())
}

/// Unix tests compile this so CI type-checks the Windows mtime compare.
#[cfg(any(test, not(unix)))]
fn same_mtime(
    held: Result<cap_std::time::SystemTime, io::Error>,
    ambient: Result<std::time::SystemTime, io::Error>,
) -> bool {
    held.ok().map(cap_std::time::SystemTime::into_std) == ambient.ok()
}

#[cfg(test)]
mod same_mtime_tests {
    use super::same_mtime;
    use std::time::{Duration, SystemTime};

    #[test]
    fn cap_std_mtime_equals_std_mtime_after_into_std() {
        let std_t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let cap_t = cap_std::time::SystemTime::from_std(std_t);
        assert!(same_mtime(Ok(cap_t), Ok(std_t)));
        assert!(!same_mtime(Ok(cap_t), Ok(std_t + Duration::from_secs(1))));
        assert!(!same_mtime(
            Ok(cap_t),
            Err(std::io::Error::other("mtime unavailable"))
        ));
        assert!(!same_mtime(
            Err(std::io::Error::other("mtime unavailable")),
            Ok(std_t)
        ));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::test_fixture::Fixture;

    use super::super::add::discover_workspace;
    use super::super::fs as repo_fs;
    use super::discover_from;

    fn fixture(label: &str) -> Fixture {
        Fixture::new("repository", label)
    }

    fn git_repo(label: &str) -> Fixture {
        let root = fixture(label);
        fs::create_dir(root.join(".git")).expect("git marker");
        root
    }

    fn replace_root(root: &Path) {
        let moved = root.with_file_name("moved");
        fs::rename(root, &moved).expect("rename repository");
        fs::create_dir(root).expect("replacement root");
        fs::create_dir(root.join(".git")).expect("replacement git marker");
    }

    #[test]
    fn ambient_path_refuses_root_replacement() {
        let root = git_repo("ambient-replaced");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);

        let error = repository
            .ambient_path()
            .expect_err("replacement must fail closed");
        assert!(
            error
                .to_string()
                .contains("no longer the directory originally opened"),
            "{error}"
        );
    }

    #[test]
    fn write_after_root_replacement_lands_in_the_original_tree() {
        let root = git_repo("write-replaced");
        let repository = discover_from(&root).expect("discover repository");
        let moved = root.with_file_name("moved");
        fs::rename(&root, &moved).expect("rename repository");
        fs::create_dir(&root).expect("replacement root");
        fs::create_dir(root.join(".git")).expect("replacement git marker");

        repo_fs::write_file_exclusive(repository.dir(), Path::new("marker.txt"), "original-tree")
            .expect("write through capability");

        assert_eq!(
            fs::read_to_string(moved.join("marker.txt")).expect("original tree"),
            "original-tree"
        );
        assert!(
            !root.join("marker.txt").exists(),
            "replacement tree must not receive the write"
        );
    }

    #[test]
    fn write_via_rename_after_root_replacement_lands_in_the_original_tree() {
        let root = git_repo("rename-replaced");
        fs::write(root.join("marker.txt"), "before").expect("seed");
        let repository = discover_from(&root).expect("discover repository");
        let moved = root.with_file_name("moved");
        fs::rename(&root, &moved).expect("rename repository");
        fs::create_dir(&root).expect("replacement root");
        fs::create_dir(root.join(".git")).expect("replacement git marker");
        fs::write(root.join("marker.txt"), "replacement").expect("replacement seed");

        repo_fs::write_file_via_rename(repository.dir(), Path::new("marker.txt"), "after")
            .expect("replace through capability");

        assert_eq!(
            fs::read_to_string(moved.join("marker.txt")).expect("original tree"),
            "after"
        );
        assert_eq!(
            fs::read_to_string(root.join("marker.txt")).expect("replacement tree"),
            "replacement"
        );
    }

    #[test]
    fn commit_write_set_after_root_replacement_lands_in_the_original_tree() {
        use std::path::PathBuf;

        use super::super::write_set::{commit_write_set, PlannedWrite};

        let root = git_repo("commit-replaced");
        fs::write(root.join("marker.txt"), "before").expect("seed");
        let repository = discover_from(&root).expect("discover repository");
        let moved = root.with_file_name("moved");
        fs::rename(&root, &moved).expect("rename repository");
        fs::create_dir(&root).expect("replacement root");
        fs::create_dir(root.join(".git")).expect("replacement git marker");
        fs::write(root.join("marker.txt"), "replacement").expect("replacement seed");

        commit_write_set(
            repository.dir(),
            &[PlannedWrite::new(
                PathBuf::from("marker.txt"),
                "before",
                "after",
            )],
            &[],
        )
        .expect("commit through capability");

        assert_eq!(
            fs::read_to_string(moved.join("marker.txt")).expect("original tree"),
            "after"
        );
        assert_eq!(
            fs::read_to_string(root.join("marker.txt")).expect("replacement tree"),
            "replacement"
        );
    }

    #[test]
    fn git_at_repository_after_root_replacement_fails_closed() {
        use super::super::git::Git;

        let root = git_repo("git-bind-replaced");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);

        let Err(error) = Git::at_repository(&repository) else {
            panic!("bind must fail closed");
        };
        assert!(
            error
                .to_string()
                .contains("no longer the directory originally opened"),
            "{error}"
        );
    }

    #[test]
    fn git_child_after_root_replacement_fails_closed() {
        use super::super::git::{Git, Op};

        let root = git_repo("git-child-replaced");
        let repository = discover_from(&root).expect("discover repository");
        let git = Git::at_repository(&repository).expect("bind");
        replace_root(&root);

        let error = git
            .predicate(Op::IsShallow)
            .expect_err("spawn must fail closed");
        assert!(
            error
                .to_string()
                .contains("no longer the directory originally opened"),
            "{error}"
        );
    }

    #[test]
    fn git_at_repository_accepts_the_original_tree() {
        use super::super::git::Git;

        let root = git_repo("git-bind-ok");
        let repository = discover_from(&root).expect("discover repository");
        Git::at_repository(&repository).unwrap_or_else(|err| panic!("{err}"));
    }

    #[test]
    fn git_child_on_the_original_tree_does_not_fail_identity() {
        use super::super::git::{Git, Op};

        let root = git_repo("git-child-ok");
        let repository = discover_from(&root).expect("discover repository");
        let git = Git::at_repository(&repository).unwrap_or_else(|err| panic!("bind: {err}"));
        match git.predicate(Op::IsShallow) {
            Ok(_) => {}
            Err(error) => {
                assert!(
                    !error
                        .to_string()
                        .contains("no longer the directory originally opened"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn knope_presence_after_root_replacement_does_not_see_the_new_tree() {
        use oakum::changeset::KnopePresence;

        use super::super::add::knope_presence;

        let root = git_repo("knope-replaced");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);
        fs::write(root.join("knope.toml"), "").expect("replacement knope");

        assert_eq!(
            knope_presence(&repository).expect("inspect held tree"),
            KnopePresence::Absent
        );
    }

    #[test]
    fn knope_presence_on_held_tree_survives_root_replacement() {
        use oakum::changeset::KnopePresence;

        use super::super::add::knope_presence;

        let root = git_repo("knope-held");
        fs::write(root.join("knope.toml"), "").expect("knope");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);

        assert_eq!(
            knope_presence(&repository).expect("inspect held tree"),
            KnopePresence::Present
        );
    }

    #[test]
    fn knope_presence_surfaces_capability_escape() {
        use super::super::add::knope_presence;

        let root = git_repo("knope-escape");
        std::os::unix::fs::symlink("/etc/passwd", root.join("knope.toml")).expect("escape");
        let repository = discover_from(&root).expect("discover repository");

        let error = knope_presence(&repository).expect_err("escape must not look Absent");
        assert!(
            error.to_string().contains("failed to inspect `knope.toml`"),
            "{error}"
        );
    }

    #[test]
    fn discovery_after_root_replacement_does_not_see_the_new_tree() {
        let root = git_repo("discover-replaced");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"original\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("original manifest");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"replacement\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("replacement manifest");

        let error =
            discover_workspace(&repository).expect_err("discovery must not follow the replacement");
        assert!(
            error
                .to_string()
                .contains("no longer the directory originally opened"),
            "{error}"
        );
    }

    #[test]
    fn discovery_after_root_replacement_does_not_report_nothing_to_discover() {
        let root = git_repo("discover-empty-replaced");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"original\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("original manifest");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);

        let error = discover_workspace(&repository)
            .expect_err("empty replacement must not look like no workspace");
        let message = error.to_string();
        assert!(
            message.contains("no longer the directory originally opened"),
            "{message}"
        );
        assert!(!message.contains("nothing to discover"), "{message}");
    }
}
