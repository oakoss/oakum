//! Tags of packages the workspace no longer has. A removed package's tags are
//! history; a renamed package's tags are history its new name has not taken
//! over yet (okm-16ak).

use std::collections::{BTreeMap, BTreeSet};

use oakum::plan::{Ecosystem, Package, PackageId, Workspace};
use oakum::tags::{absent_package_tag, prefixed_package};
use semver::Version;

use super::git::{Commit, Git, Op};
use super::tags::CommitTags;
use super::CliError;

/// `groups` without the tags of retired packages, so every reader attributes
/// what is left exactly as before. `tag_managed` names the packages oakum tags:
/// only one of those can have taken over a removed package's tags.
///
/// # Errors
///
/// `unverified` when a tag's commit cannot be read, when the manifest of a
/// package that could be a rename is a symlink or does not parse there, or when
/// an absent name was renamed in place to a tagged package with no tags of its
/// own: that package's history would otherwise read as never released.
pub(super) fn without_retired(
    git: &Git,
    workspace: &Workspace,
    tag_managed: &BTreeSet<PackageId>,
    groups: Vec<CommitTags>,
) -> Result<Vec<CommitTags>, CliError> {
    let retired = retired_tags(git, workspace, tag_managed, &groups)?;
    if retired.is_empty() {
        return Ok(groups);
    }
    Ok(groups
        .into_iter()
        .filter_map(|group| group.without(&retired))
        .collect())
}

/// Each absent name's tags with the version each states, and the commit of its
/// highest version.
struct Absent<'a> {
    newest: Version,
    commit: &'a str,
    tag: &'a str,
    tags: Vec<(&'a str, Version)>,
}

fn retired_tags(
    git: &Git,
    workspace: &Workspace,
    tag_managed: &BTreeSet<PackageId>,
    groups: &[CommitTags],
) -> Result<BTreeSet<String>, CliError> {
    let absent = absent_names(groups, workspace);
    if absent.is_empty() {
        return Ok(BTreeSet::new());
    }
    let tagged: BTreeSet<PackageId> = groups
        .iter()
        .flat_map(CommitTags::tags)
        .filter_map(|tag| prefixed_package(tag, workspace))
        .collect();
    let heirs: Vec<&Package> = workspace
        .packages()
        .filter(|package| tag_managed.contains(package.id()) && !tagged.contains(package.id()))
        .collect();
    let mut retired = BTreeMap::new();
    for (name, found) in absent {
        let commit = Commit::from_tag_listing(found.commit)?;
        if let Some(heir) = renamed_to(git, &heirs, &name, &commit)? {
            let retag = found.tag.replacen(name.as_str(), heir, 1);
            return Err(CliError::unverified(format!(
                "unverified: tag `{}` names `{name}`, which this workspace no longer has; `{heir}` now sits where `{name}` was declared and has no tags of its own, so its history would read as never released. Tag its last release under its new name: `git tag {retag} {}`",
                found.tag,
                commit.as_str(),
            )));
        }
        if declared(git, &name, &commit)? {
            retired.extend(
                found
                    .tags
                    .into_iter()
                    .map(|(tag, version)| (tag.to_owned(), version)),
            );
        }
    }
    refusing_bare_twins(groups, retired)
}

/// Tags naming no workspace package, by name, each with its highest version's
/// tag and commit. Groups arrive in commit-hash order, so the highest is
/// chosen by version, never by position.
fn absent_names<'a>(
    groups: &'a [CommitTags],
    workspace: &Workspace,
) -> BTreeMap<String, Absent<'a>> {
    let mut absent: BTreeMap<String, Absent<'a>> = BTreeMap::new();
    for group in groups {
        for tag in group.tags() {
            let Some((name, version)) = absent_package_tag(tag, workspace) else {
                continue;
            };
            let entry = absent.entry(name).or_insert_with(|| Absent {
                newest: version.clone(),
                commit: group.commit(),
                tag,
                tags: Vec::new(),
            });
            if version > entry.newest {
                entry.newest = version.clone();
                entry.commit = group.commit();
                entry.tag = tag;
            }
            entry.tags.push((tag, version));
        }
    }
    absent
}

/// The package among `heirs` whose manifest declared `name` at `commit`, in
/// the directory it occupies today.
fn renamed_to<'a>(
    git: &Git,
    heirs: &[&'a Package],
    name: &str,
    commit: &Commit,
) -> Result<Option<&'a str>, CliError> {
    for heir in heirs {
        let file = match heir.id().ecosystem {
            Ecosystem::Npm => "package.json",
            Ecosystem::Cargo => "Cargo.toml",
        };
        let path = match heir.manifest_dir() {
            "" => file.to_owned(),
            dir => format!("{dir}/{file}"),
        };
        let entries = git.paths(Op::TreeEntry {
            commit,
            path: &path,
        })?;
        let Some(entry) = entries.first() else {
            continue;
        };
        if !entry.starts_with("100") {
            return Err(unreadable(name, &path, commit, "is not a regular file"));
        }
        let text = git.text(Op::BlobText {
            commit,
            path: &path,
        })?;
        match declared_name(file, &text) {
            Some(declared) if declared == name => return Ok(Some(&heir.id().name)),
            Some(_) => {}
            None => return Err(unreadable(name, &path, commit, "does not parse")),
        }
    }
    Ok(None)
}

/// Whether any manifest at `commit` declared `name`. A tag no manifest
/// explains is not a removed package's, so the leftover look keeps it.
fn declared(git: &Git, name: &str, commit: &Commit) -> Result<bool, CliError> {
    let Some(hits) = git.matched_paths(Op::ManifestsNaming { commit, name })? else {
        return Ok(false);
    };
    let prefix = format!("{}:", commit.as_str());
    for hit in &hits {
        let path = hit.strip_prefix(&prefix).unwrap_or(hit);
        let file = path.rsplit_once('/').map_or(path, |(_, file)| file);
        let text = git.text(Op::BlobText { commit, path })?;
        if declared_name(file, &text).as_deref() == Some(name) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn unreadable(name: &str, path: &str, commit: &Commit, why: &str) -> CliError {
    CliError::unverified(format!(
        "unverified: `{path}` {why} at {}, where tags for `{name}`, a package this workspace no longer has, were cut; oakum cannot tell whether `{name}` was renamed",
        commit.as_str()
    ))
}

/// The `name` a manifest declares, or `None` when it does not parse.
fn declared_name(file: &str, text: &str) -> Option<String> {
    let name = if file == "package.json" {
        serde_json::from_str::<serde_json::Value>(text)
            .ok()?
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_owned)
    } else {
        toml::from_str::<toml::Table>(text)
            .ok()?
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_owned)
    };
    Some(name.unwrap_or_default())
}

/// The retired tag names, unless a bare `v<version>` tag shares a commit with a
/// retired tag of that version. Either package could own it, and nothing left
/// in the repository says which; read alone it would credit whichever package
/// remains. Each retired tag's version is the one its name was matched with:
/// reparsing `gone-v1.0.0-v0.9.0` reads `1.0.0-v0.9.0`.
fn refusing_bare_twins(
    groups: &[CommitTags],
    retired: BTreeMap<String, Version>,
) -> Result<BTreeSet<String>, CliError> {
    for group in groups {
        for gone in group.tags() {
            let Some(version) = retired.get(gone) else {
                continue;
            };
            let twin = group.tags().iter().find(|tag| {
                tag.strip_prefix('v')
                    .and_then(|rest| Version::parse(rest).ok())
                    .is_some_and(|bare| bare == *version)
            });
            if let Some(twin) = twin {
                return Err(CliError::unverified(format!(
                    "unverified: tag `{twin}` shares a commit with `{gone}`, a release of a package this workspace no longer has, so oakum cannot tell which package `{twin}` released. If it released that package, delete `{twin}`: `{gone}` keeps the release. If it released another package, tag that commit under the package's name, then delete `{twin}`"
                )));
            }
        }
    }
    Ok(retired.into_keys().collect())
}

#[cfg(test)]
mod tests {
    use super::{absent_names, declared_name, refusing_bare_twins, CommitTags};
    use std::collections::BTreeMap;

    /// `gone-v1.0.0-v0.9.0` names the npm package `gone-v1.0.0` at 0.9.0;
    /// reparsed alone it reads as version `1.0.0-v0.9.0`, which would miss
    /// the same-commit `v0.9.0`.
    #[test]
    fn a_bare_twin_is_found_by_the_version_the_retired_tag_was_matched_with() {
        let groups = [CommitTags::for_test(
            "aaaa",
            &["gone-v1.0.0-v0.9.0", "v0.9.0"],
        )];
        let retired = BTreeMap::from([("gone-v1.0.0-v0.9.0".to_owned(), Version::new(0, 9, 0))]);
        let err = refusing_bare_twins(&groups, retired).expect_err("twin");
        assert!(
            err.detail()
                .contains("tag `v0.9.0` shares a commit with `gone-v1.0.0-v0.9.0`"),
            "{}",
            err.detail()
        );
    }
    use oakum::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
    use semver::Version;

    #[test]
    fn the_newest_release_of_an_absent_name_is_chosen_in_either_order() {
        let workspace = Workspace::new(vec![Package::new(
            PackageId::new(Ecosystem::Cargo, "keep"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )])
        .expect("workspace");
        let older = CommitTags::for_test("aaaa", &["old@0.1.0"]);
        let newer = CommitTags::for_test("bbbb", &["old@0.2.0"]);
        for groups in [[older.clone(), newer.clone()], [newer, older]] {
            let absent = absent_names(&groups, &workspace);
            let old = &absent["old"];
            assert_eq!((old.commit, old.tag), ("bbbb", "old@0.2.0"));
            assert_eq!(old.tags.len(), 2);
        }
    }

    #[test]
    fn declared_name_reads_the_manifest_name_not_a_mention() {
        assert_eq!(
            declared_name("package.json", r#"{"name":"prose","version":"0.1.2"}"#).as_deref(),
            Some("prose")
        );
        assert_eq!(
            declared_name(
                "package.json",
                r#"{"name":"pr-kit","dependencies":{"prose":"0.1.2"}}"#
            )
            .as_deref(),
            Some("pr-kit")
        );
        assert_eq!(
            declared_name(
                "Cargo.toml",
                "[package]\nname = \"old-crate\"\nversion = \"1.0.0\"\n"
            )
            .as_deref(),
            Some("old-crate")
        );
        assert_eq!(
            declared_name("Cargo.toml", "[workspace]\nmembers = [\"old-crate\"]\n").as_deref(),
            Some("")
        );
        assert_eq!(declared_name("package.json", "{ not json"), None);
    }
}
