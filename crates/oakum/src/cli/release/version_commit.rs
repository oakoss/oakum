//! Where a pending version's tag goes: the commit that set it.

use std::path::Path;

use oakum::tags::{Declared, VersionCommit};

use super::super::changelog;
use super::super::config::LoadedConfig;
use super::super::git::{BlobKind, Commit, Followed, Git, Op};
use super::super::preconditions::PendingRelease;
use super::super::CliError;
use super::changelog_read_failed;

/// The commit that set `item`'s pending version, where its tag goes: tagged at
/// `HEAD`, a commit landing before a delayed `release` would sit inside the
/// tag and never be planned.
pub(super) fn version_commit_for(
    git: &Git,
    workspace: &oakum::plan::Workspace,
    config: &LoadedConfig,
    item: &PendingRelease,
    head: &Commit,
) -> Result<Commit, CliError> {
    let Some(package) = workspace.get(item.id()) else {
        return Err(CliError::new(format!(
            "{}: not in this workspace, so its version commit cannot be found",
            item.id()
        )));
    };
    let manifest = package.manifest_path();
    let root = match package.id().ecosystem {
        oakum::plan::Ecosystem::Cargo => Some(
            workspace
                .cargo_root_manifest()
                .unwrap_or_else(|| String::from("Cargo.toml")),
        ),
        oakum::plan::Ecosystem::Npm => None,
    };
    let root = root.filter(|root| *root != manifest);
    let mut paths = vec![manifest.as_str()];
    paths.extend(root.as_deref());
    let writers = git.first_parent_writers(&paths)?;
    let declared = writers.into_iter().map(|commit| {
        declared_version_at(
            git,
            &commit,
            package.id().ecosystem,
            &manifest,
            root.as_deref(),
        )
        .map(|version| (commit, version))
    });
    let owner = changelog::Owner::of(workspace, config, package);
    match oakum::tags::version_commit(declared, item.version())? {
        // A version set by hand before its section was written: the commit
        // that added it changed no manifest, so the walk cannot find it. A
        // bare heading there in a shared changelog may be a sibling's, and
        // history cannot be renamed, so it counts as no section.
        VersionCommit::SetAt(commit)
            if changelog_section_at(git, head, package, owner, true)?
                && !changelog_section_at(git, &commit, package, owner, false)? =>
        {
            Err(CliError::unverified(format!(
                "unverified: {} became {} at {}, which has no changelog section for it, so which commit to tag cannot be told; tag the release by hand, then run release",
                item.id().name,
                item.version(),
                commit.as_str()
            )))
        }
        VersionCommit::SetAt(commit) => Ok(commit),
        // `HEAD`'s manifest names the version, so a walk that cannot find it
        // read something other than this history.
        VersionCommit::NotDeclared => Err(CliError::unverified(format!(
            "unverified: no commit along {}'s first parents sets {} to {}, so its tag has nowhere to go; tag the commit that set it by hand",
            head.as_str(),
            manifest,
            item.version()
        ))),
        VersionCommit::Unreadable(commit) => Err(CliError::unverified(format!(
            "unverified: {} at {} names no version oakum can read, so where {} became {} cannot be told; tag the commit that set it by hand",
            root.as_deref()
                .map_or_else(|| manifest.clone(), |root| format!("{manifest} or {root}")),
            commit.as_str(),
            item.id().name,
            item.version()
        ))),
    }
}

/// Whether the package's changelog at `commit` has a section for its version.
/// Read from git, never the working tree, which index flags and sparse
/// checkouts can leave unlike `HEAD`. `refuse` refuses a changelog that
/// cannot tell (a shared bare heading, an open HTML block), which only a
/// later commit can fix.
fn changelog_section_at(
    git: &Git,
    commit: &Commit,
    package: &oakum::plan::Package,
    owner: changelog::Owner,
    refuse: bool,
) -> Result<bool, CliError> {
    let path = changelog::changelog_repo_path(package);
    let read_failed = |err: &CliError| {
        changelog_read_failed(&path, commit, "to place the tag", "no tag was created", err)
    };
    let bytes = match git.followed_blob(commit, &path).map_err(|err| read_failed(&err))? {
        Followed::File(bytes) => bytes,
        Followed::Absent | Followed::Dangling => return Ok(false),
        Followed::Unfollowed(why) => {
            return Err(CliError::unverified(format!(
                "unverified: {path} at {} is {why}, so whether it held the section cannot be read; tag the release by hand, then run release",
                commit.as_str()
            )))
        }
    };
    let text = String::from_utf8(bytes)
        .map_err(|_| read_failed(&CliError::new("its text is not valid UTF-8")))?;
    if refuse {
        return changelog::section_or_refusal(Path::new(&path), &text, owner, package.version());
    }
    Ok(changelog::holds_version_section(
        &text,
        owner,
        package.version(),
    ))
}

fn declared_version_at(
    git: &Git,
    commit: &Commit,
    ecosystem: oakum::plan::Ecosystem,
    manifest: &str,
    root: Option<&str>,
) -> Result<Declared, CliError> {
    let read = |path: &str| -> Result<Option<String>, CliError> {
        match git.blob_kind(commit, path)? {
            BlobKind::Absent => Ok(None),
            BlobKind::Other => git.blob(Op::BlobText { commit, path }).map(Some),
        }
    };
    let Some(text) = read(manifest)? else {
        return Ok(Declared::Absent);
    };
    oakum::tags::declared(ecosystem, &text, || match root {
        Some(root) => read(root),
        None => Ok(Some(text.clone())),
    })
}
