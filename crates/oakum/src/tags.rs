//! Resolve git tag names to `(package, version)` without talking to git.
//!
//! Pure: the caller supplies the tags that share one commit and the workspace
//! they could belong to. Reachable-from-HEAD collection is `okm-ls1`.
//!
//! ADR-0030: parse the four known shapes (scoped and unscoped `name@version`
//! are two productions of one format), key by package, collapse hyphen/slash
//! duplicates, ignore a bare `v{semver}` when prefixed tags on that commit
//! already name the packages for that version, and refuse leftover ambiguity
//! as unverified rather than inventing `0.1.0`.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use semver::{BuildMetadata, Version};

use crate::plan::{PackageId, Workspace};

/// Tags that look like versions but could not be attributed to exactly one
/// package at exactly one version. Unverified: name them, do not pick a
/// winner, do not compute `0.1.0` (ADR-0030).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    tags: Vec<String>,
}

impl Leftover {
    fn from_tags(tags: BTreeSet<String>) -> Option<Self> {
        if tags.is_empty() {
            None
        } else {
            Some(Self {
                tags: tags.into_iter().collect(),
            })
        }
    }

    #[must_use]
    pub fn tags(&self) -> &[String] {
        &self.tags
    }
}

impl fmt::Display for Leftover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unverified: leftover tag ambiguity: {}",
            self.tags.join(", ")
        )
    }
}

impl core::error::Error for Leftover {}

/// Attribute every tag that shares one commit.
///
/// Tags that do not look like a version (`nightly`, `v1`) are ignored. Two
/// names for the same `(package, version)` collapse to one release.
///
/// `Ok` with an empty map means this commit was parsed and yielded no releases
/// (no tags, or only ignored names). That is not a shallow clone; "we did not
/// look" belongs to collection (`okm-ls1`).
///
/// # Errors
///
/// [`Leftover`] when a tag looks like a version but matches no known
/// production, when a name matches more than one workspace package, when two
/// versions claim the same package, or when a bare `v{semver}` is the only
/// evidence in a multi-package workspace. Any leftover discards attributed
/// tags on the same commit: do not pick a winner.
pub fn resolve_commit_tags(
    tags: &[&str],
    workspace: &Workspace,
) -> Result<BTreeMap<PackageId, Version>, Leftover> {
    let mut by_package: BTreeMap<PackageId, BTreeMap<Version, BTreeSet<String>>> = BTreeMap::new();
    let mut leftover = BTreeSet::new();
    let mut bares = Vec::new();

    for tag in tags {
        match classify(tag, workspace) {
            Class::Prefixed { package, version } => {
                by_package
                    .entry(package)
                    .or_default()
                    .entry(version)
                    .or_default()
                    .insert((*tag).to_owned());
            }
            Class::Bare(version) => bares.push((*tag, version)),
            Class::LooksLikeVersion | Class::Ambiguous => {
                leftover.insert((*tag).to_owned());
            }
            Class::Ignore => {}
        }
    }

    let covered_versions: BTreeSet<Version> = by_package
        .values()
        .filter(|versions| versions.len() == 1)
        .filter_map(|versions| versions.keys().next().cloned())
        .collect();

    match unique_package(workspace) {
        Some(package) => {
            for (tag, version) in bares {
                by_package
                    .entry(package.clone())
                    .or_default()
                    .entry(version)
                    .or_default()
                    .insert(tag.to_owned());
            }
        }
        None => {
            for (tag, version) in bares {
                if !covered_versions.contains(&version) {
                    leftover.insert(tag.to_owned());
                }
            }
        }
    }

    let mut attributed = BTreeMap::new();
    for (package, mut versions) in by_package {
        if versions.len() == 1 {
            if let Some((version, _)) = versions.pop_first() {
                attributed.insert(package, version);
            }
        } else {
            leftover.extend(versions.into_values().flatten());
        }
    }

    match Leftover::from_tags(leftover) {
        None => Ok(attributed),
        Some(err) => Err(err),
    }
}

/// Highest attributed version per package across many commits.
///
/// Leftover on any commit is unverified for the whole look: do not keep a
/// partial max. A package with no attributed tag is omitted (`okm-coc`);
/// that is not drift.
///
/// # Errors
///
/// [`Leftover`] if any commit's tags cannot be attributed.
pub fn current_versions(
    commits: &[&[&str]],
    workspace: &Workspace,
) -> Result<BTreeMap<PackageId, Version>, Leftover> {
    let mut leftover = BTreeSet::new();
    let mut current = BTreeMap::new();
    for tags in commits {
        match resolve_commit_tags(tags, workspace) {
            Ok(attributed) if leftover.is_empty() => {
                for (id, version) in attributed {
                    current
                        .entry(id)
                        .and_modify(|have: &mut Version| {
                            if without_build(&version) > without_build(have) {
                                *have = version.clone();
                            }
                        })
                        .or_insert(version);
                }
            }
            Ok(_) => {}
            Err(err) => leftover.extend(err.tags().iter().cloned()),
        }
    }
    match Leftover::from_tags(leftover) {
        None => Ok(current),
        Some(err) => Err(err),
    }
}

/// A package whose manifest is above the highest reachable tag (ADR-0014).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drift {
    id: PackageId,
    manifest: Version,
    tagged: Version,
}

impl Drift {
    fn new(id: PackageId, manifest: Version, tagged: Version) -> Option<Self> {
        (without_build(&manifest) > without_build(&tagged)).then_some(Self {
            id,
            manifest,
            tagged,
        })
    }

    #[must_use]
    pub fn id(&self) -> &PackageId {
        &self.id
    }

    #[must_use]
    pub fn manifest(&self) -> &Version {
        &self.manifest
    }

    #[must_use]
    pub fn tagged(&self) -> &Version {
        &self.tagged
    }
}

/// Packages whose working-tree version is strictly above the tagged version
/// (ADR-0014). Untagged packages are bootstrap (`okm-coc`), not drift, except
/// a manifest above `0.1.0`, which is [`untagged_ahead`]. A manifest behind
/// the tag is not drift.
#[must_use]
pub fn drift(workspace: &Workspace, tagged: &BTreeMap<PackageId, Version>) -> Vec<Drift> {
    workspace
        .packages()
        .filter(|package| package.publishable())
        .filter_map(|package| {
            let tagged = tagged.get(package.id())?;
            Drift::new(
                package.id().clone(),
                package.version().clone(),
                tagged.clone(),
            )
        })
        .collect()
}

/// Publishable packages with no tag whose manifest is above `0.1.0`
/// (ADR-0014). Placeholders `0.0.0` and `0.1.0` are bootstrap.
#[must_use]
pub fn untagged_ahead(
    workspace: &Workspace,
    tagged: &BTreeMap<PackageId, Version>,
) -> Vec<(PackageId, Version)> {
    workspace
        .packages()
        .filter(|package| package.publishable())
        .filter(|package| !tagged.contains_key(package.id()))
        .filter(|package| without_build(package.version()) > Version::new(0, 1, 0))
        .map(|package| (package.id().clone(), package.version().clone()))
        .collect()
}

/// Build metadata cannot advance a release line.
fn without_build(version: &Version) -> Version {
    let mut version = version.clone();
    version.build = BuildMetadata::EMPTY;
    version
}

enum Class {
    Prefixed {
        package: PackageId,
        version: Version,
    },
    Bare(Version),
    LooksLikeVersion,
    Ambiguous,
    Ignore,
}

enum PrefixedMatch {
    Unique {
        package: PackageId,
        version: Version,
    },
    Ambiguous,
}

fn classify(tag: &str, workspace: &Workspace) -> Class {
    if let Some(found) = match_prefixed(tag, workspace) {
        return match found {
            PrefixedMatch::Unique { package, version } => Class::Prefixed { package, version },
            PrefixedMatch::Ambiguous => Class::Ambiguous,
        };
    }
    if let Some(version) = bare_version(tag) {
        return Class::Bare(version);
    }
    if looks_like_version(tag) {
        Class::LooksLikeVersion
    } else {
        Class::Ignore
    }
}

/// Longest matching package name wins (`linesmith-core` over `linesmith`).
fn match_prefixed(tag: &str, workspace: &Workspace) -> Option<PrefixedMatch> {
    let mut best: Option<(&str, Version)> = None;
    for package in workspace.packages() {
        let name = package.id().name.as_str();
        let Some(version) = prefixed_version(name, tag) else {
            continue;
        };
        if best.as_ref().is_none_or(|(n, _)| name.len() > n.len()) {
            best = Some((name, version));
        }
    }
    let (name, version) = best?;
    Some(match unique_named(workspace, name) {
        Some(package) => PrefixedMatch::Unique { package, version },
        None => PrefixedMatch::Ambiguous,
    })
}

fn prefixed_version(name: &str, tag: &str) -> Option<Version> {
    let rest = tag.strip_prefix(name)?;
    rest.strip_prefix('@')
        .or_else(|| rest.strip_prefix("/v"))
        .or_else(|| rest.strip_prefix("-v"))
        .and_then(|s| Version::parse(s).ok())
}

fn unique_named(workspace: &Workspace, name: &str) -> Option<PackageId> {
    let mut matches = workspace
        .packages()
        .filter(|package| package.id().name == name)
        .map(|package| package.id().clone());
    let first = matches.next()?;
    match matches.next() {
        Some(_) => None,
        None => Some(first),
    }
}

/// `None` means two or more packages; [`Workspace::new`] refuses an empty set.
fn unique_package(workspace: &Workspace) -> Option<PackageId> {
    let mut packages = workspace.packages().map(|package| package.id().clone());
    let first = packages.next()?;
    match packages.next() {
        Some(_) => None,
        None => Some(first),
    }
}

fn bare_version(tag: &str) -> Option<Version> {
    tag.strip_prefix('v').and_then(|s| Version::parse(s).ok())
}

fn looks_like_version(tag: &str) -> bool {
    if Version::parse(tag).is_ok() || bare_version(tag).is_some() {
        return true;
    }
    // Known productions first so a prerelease hyphen (`other-v1.2.3-beta.1`)
    // is not reduced to the last ident (`beta.1`) and ignored.
    if suffix_is_version(tag, "@") || suffix_is_version(tag, "/v") || suffix_is_version(tag, "-v") {
        return true;
    }
    if let Some((_, rest)) = tag.rsplit_once('/') {
        if Version::parse(rest).is_ok() {
            return true;
        }
    }
    for (i, ch) in tag.char_indices() {
        if ch == '-' {
            let rest = &tag[i + 1..];
            if Version::parse(rest).is_ok() || bare_version(rest).is_some() {
                return true;
            }
        }
    }
    false
}

fn suffix_is_version(tag: &str, marker: &str) -> bool {
    tag.rsplit_once(marker)
        .is_some_and(|(_, rest)| Version::parse(rest).is_ok() || bare_version(rest).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Ecosystem, Package, ResolvesDependenciesAt};
    use alloc::string::ToString;

    fn cargo_workspace(names: &[&str]) -> Workspace {
        workspace(Ecosystem::Cargo, names)
    }

    fn npm_workspace(names: &[&str]) -> Workspace {
        workspace(Ecosystem::Npm, names)
    }

    fn workspace(ecosystem: Ecosystem, names: &[&str]) -> Workspace {
        let packages: Vec<Package> = names
            .iter()
            .map(|name| {
                Package::new(
                    PackageId::new(ecosystem, *name),
                    Version::new(0, 1, 0),
                    ResolvesDependenciesAt::Install,
                    true,
                    Vec::new(),
                )
            })
            .collect();
        Workspace::new(packages).expect("workspace")
    }

    fn ver(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    fn id(ecosystem: Ecosystem, name: &str) -> PackageId {
        PackageId::new(ecosystem, name)
    }

    #[test]
    fn scoped_changeset_is_not_a_knope_split_on_slash() {
        let ws = npm_workspace(&["@jbabin91/mui-theme", "tt-package-demo-2"]);
        let got = resolve_commit_tags(&["@jbabin91/mui-theme@1.4.3"], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "@jbabin91/mui-theme")),
            Some(&ver("1.4.3"))
        );
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn unscoped_changeset_names_the_package() {
        let ws = npm_workspace(&["pr-kit", "review-cycle"]);
        let got = resolve_commit_tags(&["pr-kit@0.1.0"], &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(got.get(&id(Ecosystem::Npm, "pr-kit")), Some(&ver("0.1.0")));
    }

    #[test]
    fn scoped_and_unscoped_changeset_tags_on_one_commit() {
        let ws = npm_workspace(&["@jbabin91/mui-theme", "tt-package-demo-2"]);
        let got = resolve_commit_tags(
            &["@jbabin91/mui-theme@1.4.3", "tt-package-demo-2@1.0.0"],
            &ws,
        )
        .expect("ok");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "@jbabin91/mui-theme")),
            Some(&ver("1.4.3"))
        );
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "tt-package-demo-2")),
            Some(&ver("1.0.0"))
        );
    }

    #[test]
    fn knope_slash_and_release_plz_hyphen_collapse() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let tags = [
            "v0.2.0",
            "linesmith/v0.2.0",
            "linesmith-core/v0.2.0",
            "linesmith-core-v0.2.0",
        ];
        let got = resolve_commit_tags(&tags, &ws).expect("7219fa6");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.2.0"))
        );
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith-core")),
            Some(&ver("0.2.0"))
        );
        assert!(!got.contains_key(&id(Ecosystem::Cargo, "linesmith-plugin")));
    }

    #[test]
    fn four_hyphen_and_slash_tags_are_three_packages() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let tags = [
            "linesmith-v0.1.3",
            "linesmith-core-v0.1.3",
            "linesmith-plugin-v0.1.3",
            "linesmith-plugin/v0.1.3",
        ];
        let got = resolve_commit_tags(&tags, &ws).expect("3bbde7f");
        assert_eq!(got.len(), 3);
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.1.3"))
        );
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith-core")),
            Some(&ver("0.1.3"))
        );
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith-plugin")),
            Some(&ver("0.1.3"))
        );
    }

    #[test]
    fn bare_tag_alone_in_a_multi_package_workspace_is_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve_commit_tags(&["v0.1.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
        assert!(err.to_string().contains("v0.1.0"), "{err}");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn bare_tag_assigns_when_the_workspace_has_one_package() {
        let ws = npm_workspace(&["tsc-files"]);
        let got = resolve_commit_tags(&["v0.2.3"], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "tsc-files")),
            Some(&ver("0.2.3"))
        );
    }

    #[test]
    fn empty_tags_are_ok() {
        let ws = cargo_workspace(&["linesmith"]);
        let got = resolve_commit_tags(&[], &ws).expect("ok");
        assert!(got.is_empty());
    }

    #[test]
    fn nightly_and_v1_are_ignored() {
        let ws = cargo_workspace(&["linesmith"]);
        let got = resolve_commit_tags(&["nightly", "v1"], &ws).expect("ok");
        assert!(got.is_empty());
    }

    #[test]
    fn fifth_shape_is_leftover_not_ignored() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["linesmith/0.4.1"], &ws).expect_err("fifth");
        assert_eq!(err.tags(), &["linesmith/0.4.1".to_string()]);
    }

    #[test]
    fn unprefixed_semver_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["1.0.0"], &ws).expect_err("fifth");
        assert_eq!(err.tags(), &["1.0.0".to_string()]);
    }

    #[test]
    fn unknown_package_shaped_tag_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["other-v1.0.0"], &ws).expect_err("unknown");
        assert_eq!(err.tags(), &["other-v1.0.0".to_string()]);
    }

    #[test]
    fn same_name_in_two_ecosystems_is_leftover() {
        let packages = vec![
            Package::new(
                PackageId::new(Ecosystem::Cargo, "shared"),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                PackageId::new(Ecosystem::Npm, "shared"),
                Version::new(1, 0, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ];
        let ws = Workspace::new(packages).expect("workspace");
        let err = resolve_commit_tags(&["shared@1.2.3"], &ws).expect_err("polyglot");
        assert_eq!(err.tags(), &["shared@1.2.3".to_string()]);
    }

    #[test]
    fn two_versions_for_one_package_on_one_commit_are_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["linesmith/v0.2.0", "linesmith/v0.3.0"], &ws)
            .expect_err("conflict");
        assert_eq!(
            err.tags(),
            &[
                "linesmith/v0.2.0".to_string(),
                "linesmith/v0.3.0".to_string()
            ]
        );
    }

    #[test]
    fn longer_package_name_wins_hyphen_prefix() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let got = resolve_commit_tags(&["linesmith-core-v0.4.1"], &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith-core")),
            Some(&ver("0.4.1"))
        );
    }

    #[test]
    fn ignored_tags_do_not_hide_a_real_release() {
        let ws = cargo_workspace(&["linesmith"]);
        let got = resolve_commit_tags(&["nightly", "linesmith/v0.4.1"], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.4.1"))
        );
    }

    #[test]
    fn prerelease_hyphen_on_an_unknown_package_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["other-v1.0.0-beta.1"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["other-v1.0.0-beta.1".to_string()]);
    }

    #[test]
    fn uncovered_bare_next_to_a_prefixed_tag_is_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve_commit_tags(&["linesmith/v0.2.0", "v0.1.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn leftover_on_a_mixed_commit_discards_the_attributed_package() {
        let ws = cargo_workspace(&["linesmith"]);
        let err =
            resolve_commit_tags(&["linesmith/v0.2.0", "other-v1.0.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["other-v1.0.0".to_string()]);
    }

    #[test]
    fn ignored_tag_does_not_drop_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve_commit_tags(&["nightly", "v0.1.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn hyphen_and_slash_at_different_versions_are_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["linesmith/v0.2.0", "linesmith-v0.3.0"], &ws)
            .expect_err("conflict");
        assert_eq!(
            err.tags(),
            &[
                "linesmith-v0.3.0".to_string(),
                "linesmith/v0.2.0".to_string()
            ]
        );
    }

    #[test]
    fn two_bare_versions_in_a_one_package_workspace_are_leftover() {
        let ws = npm_workspace(&["tsc-files"]);
        let err = resolve_commit_tags(&["v0.2.3", "v0.8.4"], &ws).expect_err("conflict");
        assert_eq!(err.tags(), &["v0.2.3".to_string(), "v0.8.4".to_string()]);
    }

    #[test]
    fn prefixed_and_bare_at_different_versions_are_leftover() {
        let ws = npm_workspace(&["tsc-files"]);
        let err = resolve_commit_tags(&["tsc-files@0.2.3", "v0.8.4"], &ws).expect_err("conflict");
        assert_eq!(
            err.tags(),
            &["tsc-files@0.2.3".to_string(), "v0.8.4".to_string()]
        );
    }

    #[test]
    fn prefixed_and_bare_at_the_same_version_collapse() {
        let ws = npm_workspace(&["tsc-files"]);
        let got = resolve_commit_tags(&["tsc-files@0.2.3", "v0.2.3"], &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "tsc-files")),
            Some(&ver("0.2.3"))
        );
    }

    #[test]
    fn unknown_at_and_slash_v_prereleases_are_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["other@1.0.0-beta.1", "other/v1.0.0-beta.1"], &ws)
            .expect_err("leftover");
        assert_eq!(
            err.tags(),
            &[
                "other/v1.0.0-beta.1".to_string(),
                "other@1.0.0-beta.1".to_string()
            ]
        );
    }

    #[test]
    fn bare_tag_in_a_polyglot_workspace_is_leftover() {
        let packages = vec![
            Package::new(
                PackageId::new(Ecosystem::Cargo, "shared"),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                PackageId::new(Ecosystem::Npm, "shared"),
                Version::new(1, 0, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ];
        let ws = Workspace::new(packages).expect("workspace");
        let err = resolve_commit_tags(&["v1.2.3"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v1.2.3".to_string()]);
    }

    #[test]
    fn hyphen_without_v_on_an_unknown_package_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve_commit_tags(&["other-1.0.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["other-1.0.0".to_string()]);
    }

    #[test]
    fn two_uncovered_bares_in_a_multi_package_workspace_are_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve_commit_tags(&["v0.1.0", "v0.1.1"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string(), "v0.1.1".to_string()]);
    }

    fn cargo_at(name: &str, version: &str) -> Workspace {
        let packages = vec![Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            ver(version),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )];
        Workspace::new(packages).expect("workspace")
    }

    #[test]
    fn current_versions_takes_the_max_across_commits() {
        let ws = cargo_workspace(&["linesmith"]);
        let got =
            current_versions(&[&["linesmith/v0.1.0"], &["linesmith/v0.2.0"]], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.2.0"))
        );
        let got =
            current_versions(&[&["linesmith/v0.2.0"], &["linesmith/v0.1.0"]], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.2.0"))
        );
    }

    #[test]
    fn current_versions_keeps_packages_from_earlier_commits() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let got = current_versions(&[&["linesmith/v0.1.0"], &["linesmith-core/v0.2.0"]], &ws)
            .expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.1.0"))
        );
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith-core")),
            Some(&ver("0.2.0"))
        );
    }

    #[test]
    fn leftover_on_any_commit_is_unverified_for_the_look() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let err =
            current_versions(&[&["linesmith/v0.2.0"], &["v0.1.0"]], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn drift_when_manifest_is_above_the_tag() {
        let ws = cargo_at("linesmith", "0.2.0");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let got = drift(&ws, &tagged);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), &id(Ecosystem::Cargo, "linesmith"));
        assert_eq!(got[0].manifest(), &ver("0.2.0"));
        assert_eq!(got[0].tagged(), &ver("0.1.0"));
    }

    #[test]
    fn drift_reports_only_the_package_that_is_ahead() {
        let packages = vec![
            Package::new(
                PackageId::new(Ecosystem::Cargo, "ahead"),
                ver("0.2.0"),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                PackageId::new(Ecosystem::Cargo, "ok"),
                ver("0.1.0"),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ];
        let ws = Workspace::new(packages).expect("workspace");
        let tagged = BTreeMap::from([
            (id(Ecosystem::Cargo, "ahead"), ver("0.1.0")),
            (id(Ecosystem::Cargo, "ok"), ver("0.1.0")),
        ]);
        let got = drift(&ws, &tagged);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), &id(Ecosystem::Cargo, "ahead"));
        assert_eq!(got[0].manifest(), &ver("0.2.0"));
        assert_eq!(got[0].tagged(), &ver("0.1.0"));
    }

    #[test]
    fn no_drift_when_manifest_matches_or_is_behind() {
        let ws = cargo_at("linesmith", "0.1.0");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        assert!(drift(&ws, &tagged).is_empty());
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.2.0"))]);
        assert!(drift(&ws, &tagged).is_empty());
    }

    #[test]
    fn no_tag_is_not_drift() {
        let ws = cargo_at("linesmith", "0.2.0");
        assert!(drift(&ws, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn untagged_above_0_1_0_is_ahead() {
        let ws = cargo_at("linesmith", "0.2.0");
        let got = untagged_ahead(&ws, &BTreeMap::new());
        assert_eq!(got, vec![(id(Ecosystem::Cargo, "linesmith"), ver("0.2.0"))]);
        let ws = cargo_at("linesmith", "0.1.1");
        let got = untagged_ahead(&ws, &BTreeMap::new());
        assert_eq!(got, vec![(id(Ecosystem::Cargo, "linesmith"), ver("0.1.1"))]);
        let ws = cargo_at("linesmith", "0.1.0");
        assert!(untagged_ahead(&ws, &BTreeMap::new()).is_empty());
        let ws = cargo_at("linesmith", "0.0.0");
        assert!(untagged_ahead(&ws, &BTreeMap::new()).is_empty());
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let ws = cargo_at("linesmith", "0.2.0");
        assert!(untagged_ahead(&ws, &tagged).is_empty());
        let packages = vec![
            Package::new(
                PackageId::new(Ecosystem::Cargo, "demo"),
                ver("0.1.0"),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                PackageId::new(Ecosystem::Cargo, "other"),
                ver("0.2.0"),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ];
        let ws = Workspace::new(packages).expect("workspace");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "demo"), ver("0.1.0"))]);
        assert_eq!(
            untagged_ahead(&ws, &tagged),
            vec![(id(Ecosystem::Cargo, "other"), ver("0.2.0"))]
        );
    }

    #[test]
    fn unpublishable_package_is_not_drift() {
        let packages = vec![Package::new(
            PackageId::new(Ecosystem::Cargo, "priv"),
            ver("0.2.0"),
            ResolvesDependenciesAt::Install,
            false,
            Vec::new(),
        )];
        let ws = Workspace::new(packages).expect("workspace");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "priv"), ver("0.1.0"))]);
        assert!(drift(&ws, &tagged).is_empty());
        assert!(untagged_ahead(&ws, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn build_metadata_does_not_count_as_ahead() {
        let ws = cargo_at("linesmith", "0.1.0+local");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        assert!(drift(&ws, &tagged).is_empty());
    }
}
