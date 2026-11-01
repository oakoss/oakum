//! Resolve git tag names to `(package, version)` without talking to git.
//!
//! Pure: the caller supplies the tags that share one commit and the workspace
//! they could belong to. Reachable-from-HEAD collection is `okm-ls1`.
//!
//! ADR-0030: parse the four known shapes (scoped and unscoped `name@version`
//! are two productions of one format), key by package, collapse hyphen/slash
//! duplicates, ignore a bare `v{semver}` when prefixed tags on that commit
//! already name the packages for that version, and refuse leftover ambiguity
//! as unverified rather than inventing `0.1.0`. Bare assignment counts only
//! packages the caller marks as tag-managed candidates.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use semver::{BuildMetadata, Version};

use crate::changeset::{resolve_package_name, UnknownReason};
use crate::plan::{Ecosystem, Package, PackageId, Workspace};

/// Tags that look like versions but could not be attributed to exactly one
/// package at exactly one version. Unverified: name them, do not pick a
/// winner, do not compute `0.1.0` (ADR-0030).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    tags: Vec<String>,
    why: BTreeSet<Unattributed>,
}

/// Why a tag was left over; each cause has its own fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Unattributed {
    TwoVersions,
    BareAmongMany,
    UnknownShape,
    UnknownPackage,
    SharedName,
    OneVersionTwoCommits,
}

impl Unattributed {
    const fn fix(self) -> &'static str {
        match self {
            Self::TwoVersions => {
                "one commit carries tags for two versions of one package: keep the tag for the version that commit's manifest names and delete the other (`git tag -d <tag>`, and `git push origin :refs/tags/<tag>` if it was pushed)"
            }
            Self::BareAmongMany => {
                "a bare `v<version>` tag names no package when several are tagged: tag `<package>/v<version>` instead"
            }
            Self::UnknownShape => {
                "a tag looks like a version in a shape oakum does not read: rename it to `<package>@<version>`, `<package>/v<version>`, `<package>-v<version>`, or `v<version>` when one package is tagged"
            }
            Self::UnknownPackage => {
                "a tag names a package this workspace does not have, and no manifest at its commit declares one by that name: if it released a package under another name, tag that release with the package's current name; if it released nothing, delete it"
            }
            Self::SharedName => {
                "a tag names a package that more than one ecosystem has (a Cargo crate and an npm package of one name), so oakum cannot tell which it releases: rename the package the tag did not release"
            }
            Self::OneVersionTwoCommits => {
                "one version of a package is tagged on two commits, so oakum cannot tell which one released it: delete the tag on the commit that did not (`git tag -d <tag>`, and `git push origin :refs/tags/<tag>` if it was pushed)"
            }
        }
    }
}

impl Leftover {
    fn from_parts(tags: BTreeSet<String>, why: BTreeSet<Unattributed>) -> Option<Self> {
        if tags.is_empty() {
            None
        } else {
            Some(Self {
                tags: tags.into_iter().collect(),
                why,
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
        )?;
        // A line of its own keeps `check --json`'s `summary`, the first line;
        // unindented because the renderers indent continuation lines.
        for why in &self.why {
            write!(f, "\n{}", why.fix())?;
        }
        Ok(())
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
/// evidence among two or more tag-managed candidates. Any leftover discards
/// attributed tags on the same commit: do not pick a winner.
///
/// `bare_candidate` selects which packages may own a bare tag. Product paths
/// pass [`crate::config::Config::tag_managed`]. Prefixed matching still uses
/// the full workspace.
pub fn resolve_commit_tags(
    tags: &[&str],
    workspace: &Workspace,
    bare_candidate: impl Fn(&PackageId) -> bool,
) -> Result<BTreeMap<PackageId, Version>, Leftover> {
    let mut by_package: BTreeMap<PackageId, BTreeMap<Version, BTreeSet<String>>> = BTreeMap::new();
    let mut leftover = BTreeSet::new();
    let mut why = BTreeSet::new();
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
            Class::LooksLikeVersion => {
                leftover.insert((*tag).to_owned());
                why.insert(if in_a_read_shape(tag) {
                    Unattributed::UnknownPackage
                } else {
                    Unattributed::UnknownShape
                });
            }
            Class::Ambiguous => {
                leftover.insert((*tag).to_owned());
                why.insert(Unattributed::SharedName);
            }
            Class::Ignore => {}
        }
    }

    // Covered for bare-ignore only among bare candidates: an unmanaged
    // package's prefixed tag must not make a multi-candidate bare look settled.
    let covered_versions: BTreeSet<Version> = by_package
        .iter()
        .filter(|(id, _)| bare_candidate(id))
        .map(|(_, versions)| versions)
        .filter(|versions| versions.len() == 1)
        .filter_map(|versions| versions.keys().next().cloned())
        .collect();

    match bare_owner(workspace, &bare_candidate) {
        BareOwner::One(package) => {
            for (tag, version) in bares {
                by_package
                    .entry(package.clone())
                    .or_default()
                    .entry(version)
                    .or_default()
                    .insert(tag.to_owned());
            }
        }
        BareOwner::Many => {
            for (tag, version) in bares {
                if !covered_versions.contains(&version) {
                    leftover.insert(tag.to_owned());
                    why.insert(Unattributed::BareAmongMany);
                    // A prefix cannot tell same-named candidates apart either.
                    if candidate_names_collide(workspace, &bare_candidate) {
                        why.insert(Unattributed::SharedName);
                    }
                }
            }
        }
        BareOwner::None => {
            // Ignore rather than leftover when the managed set is empty
            // (exclude-everything / private-only probes).
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
            why.insert(Unattributed::TwoVersions);
        }
    }

    match Leftover::from_parts(leftover, why) {
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
    bare_candidate: impl Fn(&PackageId) -> bool,
) -> Result<BTreeMap<PackageId, Version>, Leftover> {
    let tagged: Vec<((), &[&str])> = commits.iter().map(|tags| ((), *tags)).collect();
    Ok(highest(&tagged, workspace, bare_candidate)?
        .into_iter()
        .map(|(id, (version, _))| (id, version))
        .collect())
}

/// A package's highest attributed tag and the commit it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatestTag {
    version: Version,
    commit: String,
}

impl LatestTag {
    #[must_use]
    pub const fn version(&self) -> &Version {
        &self.version
    }

    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }
}

/// [`current_versions`], keeping the commit each highest tag is on.
///
/// # Errors
///
/// [`Leftover`] if any commit's tags cannot be attributed, or when one
/// package's highest version is tagged on two commits: the commit is where its
/// next release's commits start, so it is not picked.
pub fn latest_tags(
    commits: &[(&str, &[&str])],
    workspace: &Workspace,
    bare_candidate: impl Fn(&PackageId) -> bool,
) -> Result<BTreeMap<PackageId, LatestTag>, Leftover> {
    let mut latest = BTreeMap::new();
    let mut leftover = BTreeSet::new();
    for (id, (version, on)) in highest(commits, workspace, bare_candidate)? {
        let mut on = on.into_iter();
        match (on.next(), on.next()) {
            (Some(commit), None) => {
                latest.insert(
                    id,
                    LatestTag {
                        version,
                        commit: String::from(commit),
                    },
                );
            }
            _ => {
                leftover.insert(format!("{}@{version}", id.name));
            }
        }
    }
    match Leftover::from_parts(
        leftover,
        BTreeSet::from([Unattributed::OneVersionTwoCommits]),
    ) {
        None => Ok(latest),
        Some(err) => Err(err),
    }
}

/// Each package's highest attributed version and every commit tagging it.
fn highest<C: Clone + Ord>(
    commits: &[(C, &[&str])],
    workspace: &Workspace,
    bare_candidate: impl Fn(&PackageId) -> bool,
) -> Result<BTreeMap<PackageId, (Version, BTreeSet<C>)>, Leftover> {
    let mut leftover = BTreeSet::new();
    let mut why = BTreeSet::new();
    let mut current: BTreeMap<PackageId, (Version, BTreeSet<C>)> = BTreeMap::new();
    for (commit, tags) in commits {
        match resolve_commit_tags(tags, workspace, &bare_candidate) {
            Ok(attributed) if leftover.is_empty() => {
                for (id, version) in attributed {
                    let have = current
                        .entry(id)
                        .or_insert_with(|| (version.clone(), BTreeSet::new()));
                    match without_build(&version).cmp(&without_build(&have.0)) {
                        core::cmp::Ordering::Greater => {
                            *have = (version, BTreeSet::from([commit.clone()]));
                        }
                        core::cmp::Ordering::Equal => {
                            have.1.insert(commit.clone());
                        }
                        core::cmp::Ordering::Less => {}
                    }
                }
            }
            Ok(_) => {}
            Err(err) => {
                leftover.extend(err.tags);
                why.extend(err.why);
            }
        }
    }
    match Leftover::from_parts(leftover, why) {
        None => Ok(current),
        Some(err) => Err(err),
    }
}

/// Where a package's commit-derived intent starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentBase {
    /// After the commit its highest tag is on.
    Since(String),
    /// Never released: all of `HEAD`'s history.
    Root,
    /// Released by `version`, untagged: rereading would plan the release twice.
    AwaitingTag,
}

/// A manifest above its highest tag, or untagged, with a changelog section for
/// its version was written by `version` (ADR-0014, *Untagged versions*). One
/// without the section was set by hand, and reads from the tag as usual.
///
/// # Errors
///
/// Whatever `has_section` returns, asked only when the answer decides.
pub fn intent_base<E>(
    package: &Package,
    latest: Option<&LatestTag>,
    has_section: impl FnOnce() -> Result<bool, E>,
) -> Result<IntentBase, E> {
    let unreleased = latest
        .is_none_or(|latest| without_build(package.version()) > without_build(&latest.version));
    if unreleased && has_section()? {
        return Ok(IntentBase::AwaitingTag);
    }
    Ok(latest.map_or(IntentBase::Root, |latest| {
        IntentBase::Since(latest.commit.clone())
    }))
}

/// What a manifest declares at one commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declared {
    Version(Version),
    /// No manifest, or one that names no version.
    Absent,
    /// A manifest that does not parse: a look that did not happen, not a
    /// different version.
    Unreadable,
}

/// What a manifest's `text` declares. `root` reads the workspace manifest an
/// inheriting Cargo member takes its version from, `Ok(None)` when it is
/// absent; it is called only for such a member.
///
/// # Errors
///
/// What `root` returns.
pub fn declared<E>(
    ecosystem: Ecosystem,
    text: &str,
    root: impl FnOnce() -> Result<Option<String>, E>,
) -> Result<Declared, E> {
    use crate::manifest::{
        cargo_declared_version, cargo_workspace_version, package_json_version, DeclaredVersion,
    };
    // An inheriting member whose workspace names no version is not a manifest
    // cargo accepts, so it is unreadable rather than absent.
    let inherited = |text: &str| cargo_workspace_version(text).ok().flatten();
    let version = match ecosystem {
        Ecosystem::Npm => match package_json_version(text) {
            Ok(Some(version)) => Some(version),
            Ok(None) => return Ok(Declared::Absent),
            Err(_) => None,
        },
        Ecosystem::Cargo => match cargo_declared_version(text) {
            Ok(DeclaredVersion::Literal(version)) => Some(version),
            Ok(DeclaredVersion::Workspace) => root()?.as_deref().and_then(inherited),
            Ok(DeclaredVersion::Absent) => return Ok(Declared::Absent),
            Err(_) => None,
        },
    };
    Ok(version
        .and_then(|version| Version::parse(&version).ok())
        .map_or(Declared::Unreadable, Declared::Version))
}

/// Where a pending release's tag goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionCommit<C> {
    /// The commit that bumped the manifest to the pending version, so commits
    /// landing before `release` runs stay after the tag.
    SetAt(C),
    /// The newest writer declares another version.
    NotDeclared,
    /// This writer's manifest does not parse, so the walk cannot go past it.
    Unreadable(C),
}

/// `writers`: commits that changed the manifest along `HEAD`'s first parents,
/// newest first, with what each declares.
///
/// # Errors
///
/// The first error `writers` yields; later writers are not read.
pub fn version_commit<C, E>(
    writers: impl IntoIterator<Item = Result<(C, Declared), E>>,
    pending: &Version,
) -> Result<VersionCommit<C>, E> {
    let mut newest_run = None;
    for writer in writers {
        let (commit, declared) = writer?;
        match declared {
            Declared::Version(version) if without_build(&version) == without_build(pending) => {
                newest_run = Some(commit);
            }
            // No version, or no manifest, is a version other than the pending
            // one (Cargo reads a missing version as 0.0.0), so the newer
            // writer set it.
            Declared::Version(_) | Declared::Absent => {
                return Ok(newest_run.map_or(VersionCommit::NotDeclared, VersionCommit::SetAt));
            }
            Declared::Unreadable => return Ok(VersionCommit::Unreadable(commit)),
        }
    }
    // Before its oldest writer the manifest did not exist at this path.
    Ok(newest_run.map_or(VersionCommit::NotDeclared, VersionCommit::SetAt))
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
/// (ADR-0014). Untagged packages are never drift; the ones owed a tag are
/// [`untagged_pending`]. A manifest behind the tag is not drift.
///
/// `managed` selects which packages are in scope (typically publishable, or
/// `OakumConfig::tag_managed`).
#[must_use]
pub fn drift(
    workspace: &Workspace,
    tagged: &BTreeMap<PackageId, Version>,
    managed: impl Fn(&Package) -> bool,
) -> Vec<Drift> {
    workspace
        .packages()
        .filter(|package| managed(package))
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

/// A package whose manifest is below its highest reachable tag: the next
/// version computed from it would repeat or undercut a released one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Behind {
    id: PackageId,
    manifest: Version,
    tagged: Version,
}

impl Behind {
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

/// Managed packages whose working-tree version is strictly below the tagged
/// version, build metadata ignored.
#[must_use]
pub fn behind(
    workspace: &Workspace,
    tagged: &BTreeMap<PackageId, Version>,
    managed: impl Fn(&Package) -> bool,
) -> Vec<Behind> {
    workspace
        .packages()
        .filter(|package| managed(package))
        .filter_map(|package| {
            let tagged = tagged.get(package.id())?;
            (without_build(package.version()) < without_build(tagged)).then(|| Behind {
                id: package.id().clone(),
                manifest: package.version().clone(),
                tagged: tagged.clone(),
            })
        })
        .collect()
}

/// Managed packages whose manifest matches the highest reachable tag.
#[must_use]
pub fn tagged_current(
    workspace: &Workspace,
    tagged: &BTreeMap<PackageId, Version>,
    managed: impl Fn(&Package) -> bool,
) -> Vec<(PackageId, Version)> {
    workspace
        .packages()
        .filter(|package| managed(package))
        .filter_map(|package| {
            let tagged = tagged.get(package.id())?;
            (without_build(package.version()) == without_build(tagged))
                .then(|| (package.id().clone(), tagged.clone()))
        })
        .collect()
}

/// A managed package with no reachable tag that is owed one: never a `0.0.0`
/// or `0.1.0` placeholder without a changelog section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Untagged {
    id: PackageId,
    version: Version,
    has_section: bool,
}

impl Untagged {
    fn new(package: &Package, has_section: bool) -> Option<Self> {
        (has_section || !is_placeholder(package.version())).then(|| Self {
            id: package.id().clone(),
            version: package.version().clone(),
            has_section,
        })
    }

    #[must_use]
    pub fn id(&self) -> &PackageId {
        &self.id
    }

    #[must_use]
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Whether the changelog has a section for the version, like the one
    /// `version` writes. Also `false` when the changelog was unreadable and
    /// could only have changed the wording.
    #[must_use]
    pub fn has_section(&self) -> bool {
        self.has_section
    }
}

/// Managed packages with no tag that are owed one (ADR-0014, *Untagged
/// versions*): every version except a placeholder `0.0.0` or `0.1.0` whose
/// changelog has no section for it. `has_section` reads that changelog.
///
/// Returns the pending packages and, apart, the read errors for placeholders
/// whose changelog decides whether they owe a tag at all; one package the look
/// could not settle does not hide the others.
pub fn untagged_pending<E>(
    workspace: &Workspace,
    tagged: &BTreeMap<PackageId, Version>,
    managed: impl Fn(&Package) -> bool,
    has_section: impl Fn(&Package) -> Result<bool, E>,
) -> (Vec<Untagged>, Vec<E>) {
    let mut pending = Vec::new();
    let mut unread = Vec::new();
    for package in workspace
        .packages()
        .filter(|package| managed(package))
        .filter(|package| !tagged.contains_key(package.id()))
    {
        let section = match has_section(package) {
            Ok(section) => section,
            Err(err) if is_placeholder(package.version()) => {
                unread.push(err);
                continue;
            }
            Err(_) => false,
        };
        pending.extend(Untagged::new(package, section));
    }
    (pending, unread)
}

/// `0.0.0` or `0.1.0`, build metadata aside: the numbers a fresh manifest
/// carries, which owe a tag only once a changelog section names them.
#[must_use]
pub fn is_placeholder(version: &Version) -> bool {
    let version = without_build(version);
    version == Version::new(0, 0, 0) || version == Version::new(0, 1, 0)
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
    match resolve_package_name(name, workspace) {
        Ok(package) => Some(PrefixedMatch::Unique { package, version }),
        Err(UnknownReason::Ambiguous) => Some(PrefixedMatch::Ambiguous),
        Err(UnknownReason::Missing) => None,
    }
}

fn prefixed_version(name: &str, tag: &str) -> Option<Version> {
    let rest = tag.strip_prefix(name)?;
    rest.strip_prefix('@')
        .or_else(|| rest.strip_prefix("/v"))
        .or_else(|| rest.strip_prefix("-v"))
        .and_then(|s| Version::parse(s).ok())
}

enum BareOwner {
    None,
    One(PackageId),
    Many,
}

fn bare_owner(workspace: &Workspace, bare_candidate: &impl Fn(&PackageId) -> bool) -> BareOwner {
    let mut packages = workspace
        .packages()
        .map(|package| package.id().clone())
        .filter(|id| bare_candidate(id));
    let Some(first) = packages.next() else {
        return BareOwner::None;
    };
    match packages.next() {
        Some(_) => BareOwner::Many,
        None => BareOwner::One(first),
    }
}

fn bare_version(tag: &str) -> Option<Version> {
    tag.strip_prefix('v').and_then(|s| Version::parse(s).ok())
}

/// Whether a tag claims to state a version at all, in any shape — including
/// shapes oakum does not attribute to a package.
///
/// `false` is what `release` treats as a tag belonging to something else
/// entirely: `v1`, `latest`, `nightly`, `release-2024-01-01`. Those are moving
/// pointers and date stamps, not releases, and a reader that refuses a history
/// over one is refusing over a tag `release` itself skips ([ADR-0030]).
///
/// [ADR-0030]: ../../../docs/decisions/0030-derive-read-tag-shapes.md
#[must_use]
pub fn looks_like_version(tag: &str) -> bool {
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

/// `<name>@<version>`, `<name>/v<version>` or `<name>-v<version>` for some name.
/// A name holds no `@` past its scope, so `demo@v1.2.3-v2.0.0` is no read shape.
fn in_a_read_shape(tag: &str) -> bool {
    read_shape(tag).is_some()
}

fn read_shape(tag: &str) -> Option<(&str, Version)> {
    ["@", "/v", "-v"].iter().find_map(|marker| {
        let (name, rest) = tag.rsplit_once(marker)?;
        if name.is_empty() || name.chars().skip(1).any(|c| c == '@') {
            return None;
        }
        Version::parse(rest).ok().map(|version| (name, version))
    })
}

/// The package name and version a tag states when it is in a read shape but
/// names no package in `workspace`, as after a removal or a rename.
#[must_use]
pub fn absent_package_tag(tag: &str, workspace: &Workspace) -> Option<(String, Version)> {
    match classify(tag, workspace) {
        Class::LooksLikeVersion => {
            read_shape(tag).map(|(name, version)| (name.to_owned(), version))
        }
        _ => None,
    }
}

/// The workspace package a prefixed tag names, whatever its version.
#[must_use]
pub fn prefixed_package(tag: &str, workspace: &Workspace) -> Option<PackageId> {
    match classify(tag, workspace) {
        Class::Prefixed { package, .. } => Some(package),
        _ => None,
    }
}

fn candidate_names_collide(
    workspace: &Workspace,
    bare_candidate: &impl Fn(&PackageId) -> bool,
) -> bool {
    let mut seen = BTreeSet::new();
    workspace
        .packages()
        .map(Package::id)
        .filter(|id| bare_candidate(id))
        .any(|id| !seen.insert(id.name.as_str()))
}

fn suffix_is_version(tag: &str, marker: &str) -> bool {
    tag.rsplit_once(marker)
        .is_some_and(|(_, rest)| Version::parse(rest).is_ok() || bare_version(rest).is_some())
}

/// Version only; does not attribute a package.
///
/// ADR-0030: `name@version`, `name/v{semver}`, `name-v{semver}`, `v{semver}`.
#[must_use]
pub fn version_from_tag(tag: &str) -> Option<Version> {
    if let Ok(version) = Version::parse(tag) {
        return Some(version);
    }
    if let Some(version) = bare_version(tag) {
        return Some(version);
    }
    for marker in ["@", "/v"] {
        if let Some((_, rest)) = tag.rsplit_once(marker) {
            if let Some(version) = version_suffix(rest) {
                return Some(version);
            }
        }
    }
    // Leftmost `-v` so `pkg-v1.0.0-v1` keeps prerelease `v1` instead of parsing `1`.
    for (i, _) in tag.match_indices("-v") {
        if let Some(version) = version_suffix(&tag[i + 2..]) {
            return Some(version);
        }
    }
    tag.rsplit_once('/')
        .and_then(|(_, rest)| Version::parse(rest).ok())
}

fn version_suffix(rest: &str) -> Option<Version> {
    Version::parse(rest).ok().or_else(|| bare_version(rest))
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
        workspace_publishability(
            ecosystem,
            &names.iter().map(|n| (*n, true)).collect::<Vec<_>>(),
        )
    }

    fn workspace_publishability(ecosystem: Ecosystem, packages: &[(&str, bool)]) -> Workspace {
        let packages: Vec<Package> = packages
            .iter()
            .map(|(name, publishable)| {
                Package::new(
                    PackageId::new(ecosystem, *name),
                    Version::new(0, 1, 0),
                    ResolvesDependenciesAt::Install,
                    *publishable,
                    Vec::new(),
                )
            })
            .collect();
        Workspace::new(packages).expect("workspace")
    }

    fn at(version: &str) -> Package {
        Package::new(
            PackageId::new(Ecosystem::Cargo, "demo"),
            Version::parse(version).expect("version"),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )
    }

    fn tagged(version: &str) -> LatestTag {
        LatestTag {
            version: Version::parse(version).expect("version"),
            commit: String::from("tagsha"),
        }
    }

    fn base(package: &Package, latest: Option<&LatestTag>, section: bool) -> IntentBase {
        intent_base::<()>(package, latest, || Ok(section)).expect("base")
    }

    #[test]
    fn a_tagged_package_reads_from_its_tag() {
        let latest = tagged("0.4.1");
        let since = IntentBase::Since(String::from("tagsha"));
        assert_eq!(base(&at("0.4.1"), Some(&latest), true), since);
        assert_eq!(base(&at("0.4.1+build"), Some(&latest), true), since);
        // Set by hand above the tag: no section, so the tag still decides.
        assert_eq!(base(&at("0.5.0"), Some(&latest), false), since);
    }

    #[test]
    fn a_version_written_but_not_tagged_awaits_its_tag() {
        assert_eq!(
            base(&at("0.4.2"), Some(&tagged("0.4.1")), true),
            IntentBase::AwaitingTag
        );
        assert_eq!(base(&at("0.1.0"), None, true), IntentBase::AwaitingTag);
    }

    #[test]
    fn a_never_released_package_reads_all_history() {
        assert_eq!(base(&at("0.1.0"), None, false), IntentBase::Root);
    }

    #[test]
    fn the_changelog_is_read_only_when_it_decides() {
        let read = intent_base::<&str>(&at("0.4.1"), Some(&tagged("0.4.1")), || Err("read"));
        assert_eq!(read, Ok(IntentBase::Since(String::from("tagsha"))));
        let read = intent_base::<&str>(&at("0.4.2"), Some(&tagged("0.4.1")), || Err("read"));
        assert_eq!(read, Err("read"));
    }

    /// `"!"` is a manifest that does not parse; `""` is one that is absent.
    fn stated(version: &str) -> Declared {
        match version {
            "" => Declared::Absent,
            "!" => Declared::Unreadable,
            version => Declared::Version(Version::parse(version).expect("version")),
        }
    }

    fn walk(list: &[(&'static str, &str)]) -> VersionCommit<&'static str> {
        let writers = list
            .iter()
            .map(|(commit, version)| Ok::<_, ()>((*commit, stated(version))));
        version_commit(writers, &Version::new(0, 4, 1)).expect("walk")
    }

    fn cargo(text: &str, root: Option<&str>) -> Declared {
        declared(Ecosystem::Cargo, text, || {
            Ok::<_, ()>(root.map(String::from))
        })
        .expect("root")
    }

    #[test]
    fn a_cargo_manifest_declares_its_literal_or_inherited_version() {
        assert_eq!(
            cargo("[package]\nversion = \"0.4.1\"\n", None),
            stated("0.4.1")
        );
        let member = "[package]\nversion.workspace = true\n";
        assert_eq!(
            cargo(member, Some("[workspace.package]\nversion = \"0.4.1\"\n")),
            stated("0.4.1")
        );
        assert_eq!(cargo("[package]\nname = \"x\"\n", None), stated(""));
    }

    /// Cargo refuses each of these, so none is a version the walk can pass.
    #[test]
    fn a_cargo_manifest_cargo_would_refuse_is_unreadable() {
        let member = "[package]\nversion.workspace = true\n";
        for (text, root) in [
            ("[[ not toml", None),
            ("[package]\nversion = \"not semver\"\n", None),
            (member, None),
            (member, Some("[workspace]\n")),
            (member, Some("[[ not toml")),
        ] {
            assert_eq!(cargo(text, root), stated("!"), "{text} / {root:?}");
        }
    }

    #[test]
    fn a_package_json_without_a_version_is_absent_not_unreadable() {
        let npm = |text: &str| {
            declared(Ecosystem::Npm, text, || -> Result<_, ()> {
                panic!("npm has no root")
            })
            .expect("npm")
        };
        assert_eq!(npm("{\"version\": \"0.4.1\"}"), stated("0.4.1"));
        assert_eq!(npm("{\"name\": \"x\"}"), stated(""));
        assert_eq!(npm("{ not json"), stated("!"));
        assert_eq!(npm("{\"version\": 1}"), stated("!"));
    }

    #[test]
    fn the_root_is_read_only_for_an_inheriting_member() {
        let mut asked = false;
        let _ = declared(Ecosystem::Cargo, "[package]\nversion = \"0.4.1\"\n", || {
            asked = true;
            Ok::<_, ()>(None)
        });
        assert!(!asked);
        assert_eq!(
            declared(
                Ecosystem::Cargo,
                "[package]\nversion.workspace = true\n",
                || { Err::<Option<String>, _>("unread") }
            ),
            Err("unread")
        );
    }

    #[test]
    fn the_version_commit_is_where_the_pending_version_began() {
        // A dependency edit after the release still declares 0.4.1.
        assert_eq!(
            walk(&[("deps", "0.4.1"), ("release", "0.4.1+b"), ("old", "0.4.0")]),
            VersionCommit::SetAt("release")
        );
    }

    /// A package created at its version, or moved: the version was set where
    /// the manifest appeared at this path.
    #[test]
    fn a_walk_that_runs_out_of_writers_tags_where_the_manifest_appeared() {
        assert_eq!(
            walk(&[("edit", "0.4.1"), ("created", "0.4.1")]),
            VersionCommit::SetAt("created")
        );
    }

    /// A version key that went away, or a manifest deleted and recreated, is a
    /// version other than the pending one.
    #[test]
    fn an_absent_version_before_the_run_is_where_it_was_set() {
        assert_eq!(
            walk(&[("edit", "0.4.1"), ("restored", "0.4.1"), ("deleted", "")]),
            VersionCommit::SetAt("restored")
        );
    }

    #[test]
    fn a_newest_writer_on_another_version_is_not_declared() {
        assert_eq!(
            walk(&[("next", "0.4.2"), ("release", "0.4.1")]),
            VersionCommit::NotDeclared
        );
        assert_eq!(walk(&[]), VersionCommit::NotDeclared);
    }

    /// A manifest that does not parse might be where the version was set.
    #[test]
    fn an_unreadable_manifest_stops_the_walk() {
        assert_eq!(
            walk(&[("repair", "0.4.1"), ("broken", "!"), ("release", "0.4.1")]),
            VersionCommit::Unreadable("broken")
        );
    }

    #[test]
    fn the_walk_stops_at_the_first_version_that_differs() {
        let mut read = 0;
        let lazy = [("release", "0.4.1"), ("old", "0.4.0"), ("older", "0.3.0")]
            .into_iter()
            .map(|(commit, version)| {
                read += 1;
                Ok::<_, ()>((commit, stated(version)))
            });
        assert_eq!(
            version_commit(lazy, &Version::new(0, 4, 1)),
            Ok(VersionCommit::SetAt("release"))
        );
        assert_eq!(read, 2);
    }

    #[test]
    fn latest_tags_keeps_the_highest_tag_commit() {
        let ws = cargo_workspace(&["demo"]);
        let new: (&str, &[&str]) = ("new", &["demo@0.2.0"]);
        let old: (&str, &[&str]) = ("old", &["demo@0.1.0"]);
        for commits in [[new, old], [old, new]] {
            let latest = latest_tags(&commits, &ws, |_| true).expect("latest");
            assert_eq!(
                latest.get(&PackageId::new(Ecosystem::Cargo, "demo")),
                Some(&LatestTag {
                    version: Version::new(0, 2, 0),
                    commit: String::from("new"),
                })
            );
        }
    }

    /// Which commit released it decides where the next release's commits
    /// start, so a tie is not broken by listing order.
    #[test]
    fn one_version_on_two_commits_is_leftover() {
        let ws = cargo_workspace(&["demo"]);
        let commits: [(&str, &[&str]); 2] =
            [("aaa", &["demo@0.2.0+x"]), ("bbb", &["demo@0.2.0+y"])];
        let err = latest_tags(&commits, &ws, |_| true).expect_err("tie");
        assert_eq!(err.tags(), ["demo@0.2.0+x"]);
        assert!(err.to_string().contains("tagged on two commits"), "{err}");
        let versions: [&[&str]; 2] = [&["demo@0.2.0+x"], &["demo@0.2.0+y"]];
        current_versions(&versions, &ws, |_| true).expect("versions need no commit");
    }

    /// Unit tests that do not model selection treat every member as a candidate.
    fn resolve(
        tags: &[&str],
        workspace: &Workspace,
    ) -> Result<BTreeMap<PackageId, Version>, Leftover> {
        resolve_commit_tags(tags, workspace, |_| true)
    }

    fn current(
        commits: &[&[&str]],
        workspace: &Workspace,
    ) -> Result<BTreeMap<PackageId, Version>, Leftover> {
        current_versions(commits, workspace, |_| true)
    }

    fn ver(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    fn id(ecosystem: Ecosystem, name: &str) -> PackageId {
        PackageId::new(ecosystem, name)
    }

    #[test]
    fn version_from_tag_reads_adr_0030_shapes() {
        assert_eq!(version_from_tag("v1.2.3").as_ref(), Some(&ver("1.2.3")));
        assert_eq!(
            version_from_tag("demo@0.10.0").as_ref(),
            Some(&ver("0.10.0"))
        );
        assert_eq!(
            version_from_tag("linesmith-core-v0.2.0").as_ref(),
            Some(&ver("0.2.0"))
        );
        assert_eq!(
            version_from_tag("linesmith/v0.2.0").as_ref(),
            Some(&ver("0.2.0"))
        );
        assert_eq!(version_from_tag("nightly"), None);
        assert!(version_from_tag("demo@0.10.0").unwrap() > version_from_tag("demo@0.9.0").unwrap());
        assert_eq!(
            version_from_tag("pkg-v1.0.0-v1").as_ref(),
            Some(&ver("1.0.0-v1"))
        );
    }

    #[test]
    fn scoped_changeset_is_not_a_knope_split_on_slash() {
        let ws = npm_workspace(&["@jbabin91/mui-theme", "tt-package-demo-2"]);
        let got = resolve(&["@jbabin91/mui-theme@1.4.3"], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "@jbabin91/mui-theme")),
            Some(&ver("1.4.3"))
        );
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn unscoped_changeset_names_the_package() {
        let ws = npm_workspace(&["pr-kit", "review-cycle"]);
        let got = resolve(&["pr-kit@0.1.0"], &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(got.get(&id(Ecosystem::Npm, "pr-kit")), Some(&ver("0.1.0")));
    }

    #[test]
    fn scoped_and_unscoped_changeset_tags_on_one_commit() {
        let ws = npm_workspace(&["@jbabin91/mui-theme", "tt-package-demo-2"]);
        let got = resolve(
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
        let got = resolve(&tags, &ws).expect("7219fa6");
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
        let got = resolve(&tags, &ws).expect("3bbde7f");
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
        let err = resolve(&["v0.1.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
        assert!(err.to_string().contains("v0.1.0"), "{err}");
        assert!(err.to_string().contains("unverified"), "{err}");
        assert_fixes(&err, &[Unattributed::BareAmongMany]);
    }

    /// The message names each cause's fix, and only those.
    fn assert_fixes(err: &Leftover, expected: &[Unattributed]) {
        assert_eq!(err.why, expected.iter().copied().collect(), "{err}");
        let text = err.to_string();
        for why in expected {
            assert!(text.contains(why.fix()), "{text}");
        }
    }

    #[test]
    fn a_leftover_across_commits_names_every_cause() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let commits: [&[&str]; 2] = [
            &["v0.1.0", "linesmith/0.4.1"],
            &["linesmith/v0.2.0", "linesmith/v0.3.0"],
        ];
        let err = current_versions(&commits, &ws, |_| true).expect_err("leftover");
        assert_fixes(
            &err,
            &[
                Unattributed::TwoVersions,
                Unattributed::BareAmongMany,
                Unattributed::UnknownShape,
            ],
        );
    }

    #[test]
    fn bare_tag_assigns_when_the_workspace_has_one_package() {
        let ws = npm_workspace(&["tsc-files"]);
        let got = resolve(&["v0.2.3"], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "tsc-files")),
            Some(&ver("0.2.3"))
        );
    }

    #[test]
    fn bare_tag_is_ignored_when_no_tag_candidate_remains() {
        let ws = workspace_publishability(
            Ecosystem::Cargo,
            &[("probe", false), ("other-probe", false)],
        );
        let got = resolve_commit_tags(&["v0.1.0"], &ws, |_| false).expect("ok");
        assert!(got.is_empty());
    }

    #[test]
    fn bare_tag_assigns_when_only_one_member_is_a_tag_candidate() {
        let ws =
            workspace_publishability(Ecosystem::Cargo, &[("oakum", true), ("plan-no-std", false)]);
        let candidates: BTreeSet<_> = ws
            .packages()
            .filter(|package| package.publishable())
            .map(|package| package.id().clone())
            .collect();
        let got = resolve_commit_tags(&["v0.1.0"], &ws, |id| candidates.contains(id)).expect("ok");
        assert_eq!(got.get(&id(Ecosystem::Cargo, "oakum")), Some(&ver("0.1.0")));
        assert!(!got.contains_key(&id(Ecosystem::Cargo, "plan-no-std")));
    }

    #[test]
    fn bare_tag_assigns_when_a_publishable_sibling_is_excluded_from_candidates() {
        let ws = cargo_workspace(&["alpha", "beta"]);
        let candidates: BTreeSet<_> = [id(Ecosystem::Cargo, "alpha")].into_iter().collect();
        let got =
            resolve_commit_tags(&["v0.1.0"], &ws, |pkg| candidates.contains(pkg)).expect("ok");
        assert_eq!(got.get(&id(Ecosystem::Cargo, "alpha")), Some(&ver("0.1.0")));
        assert!(!got.contains_key(&id(Ecosystem::Cargo, "beta")));
    }

    #[test]
    fn bare_tag_is_leftover_when_two_tag_candidates_remain() {
        let ws = workspace_publishability(
            Ecosystem::Cargo,
            &[("oakum", true), ("sibling", true), ("probe", false)],
        );
        let candidates: BTreeSet<_> = ws
            .packages()
            .filter(|package| package.publishable())
            .map(|package| package.id().clone())
            .collect();
        let err = resolve_commit_tags(&["v0.1.0"], &ws, |id| candidates.contains(id))
            .expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn unmanaged_prefixed_tag_does_not_cover_a_multi_candidate_bare() {
        let ws = workspace_publishability(
            Ecosystem::Cargo,
            &[("oakum", true), ("sibling", true), ("probe", false)],
        );
        let candidates: BTreeSet<_> = ws
            .packages()
            .filter(|package| package.publishable())
            .map(|package| package.id().clone())
            .collect();
        let err = resolve_commit_tags(&["probe/v0.1.0", "v0.1.0"], &ws, |id| {
            candidates.contains(id)
        })
        .expect_err("leftover");
        assert!(err.tags().contains(&"v0.1.0".to_string()), "{err:?}");
    }

    #[test]
    fn absent_package_tag_names_a_package_the_workspace_lacks() {
        let ws = npm_workspace(&["pr-kit", "@oakoss/ui"]);
        for (tag, name) in [
            ("prose@0.1.2", "prose"),
            ("@oakoss/old@1.0.0", "@oakoss/old"),
            ("prose/v0.1.2", "prose"),
            ("old-crate-v2.0.0", "old-crate"),
        ] {
            let (got, _) = absent_package_tag(tag, &ws).unwrap_or_else(|| panic!("{tag}"));
            assert_eq!(got, name, "{tag}");
        }
        for tag in [
            "pr-kit@0.2.0",
            "@oakoss/ui@1.0.0",
            "v1.0.0",
            "nightly",
            "prose/0.1.2",
        ] {
            assert_eq!(absent_package_tag(tag, &ws), None, "{tag}");
        }
    }

    #[test]
    fn prefixed_package_ignores_bare_and_absent_names() {
        let ws = npm_workspace(&["pr-kit"]);
        assert_eq!(
            prefixed_package("pr-kit@0.2.0", &ws),
            Some(PackageId::new(Ecosystem::Npm, "pr-kit"))
        );
        assert_eq!(prefixed_package("v0.2.0", &ws), None);
        assert_eq!(prefixed_package("prose@0.1.2", &ws), None);
    }

    #[test]
    fn empty_tags_are_ok() {
        let ws = cargo_workspace(&["linesmith"]);
        let got = resolve(&[], &ws).expect("ok");
        assert!(got.is_empty());
    }

    #[test]
    fn nightly_and_v1_are_ignored() {
        let ws = cargo_workspace(&["linesmith"]);
        let got = resolve(&["nightly", "v1"], &ws).expect("ok");
        assert!(got.is_empty());
    }

    #[test]
    fn fifth_shape_is_leftover_not_ignored() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["linesmith/0.4.1"], &ws).expect_err("fifth");
        assert_eq!(err.tags(), &["linesmith/0.4.1".to_string()]);
        assert_fixes(&err, &[Unattributed::UnknownShape]);
    }

    #[test]
    fn unprefixed_semver_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["1.0.0"], &ws).expect_err("fifth");
        assert_eq!(err.tags(), &["1.0.0".to_string()]);
        assert_fixes(&err, &[Unattributed::UnknownShape]);
        let err = resolve(&["linesmith@v1.2.3-v2.0.0"], &ws).expect_err("compound");
        assert_fixes(&err, &[Unattributed::UnknownShape]);
    }

    #[test]
    fn unknown_package_shaped_tag_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["other-v1.0.0"], &ws).expect_err("unknown");
        assert_eq!(err.tags(), &["other-v1.0.0".to_string()]);
        assert_fixes(&err, &[Unattributed::UnknownPackage]);
        for tag in ["other@1.0.0", "@scope/other@1.0.0", "other/v1.0.0"] {
            let err = resolve(&[tag], &ws).expect_err(tag);
            assert_fixes(&err, &[Unattributed::UnknownPackage]);
        }
    }

    #[test]
    fn one_commit_names_each_of_its_causes() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let err = resolve(&["v0.1.0", "linesmith/0.4.1"], &ws).expect_err("leftover");
        assert_fixes(
            &err,
            &[Unattributed::BareAmongMany, Unattributed::UnknownShape],
        );
    }

    /// Prefixing alone would not settle it: the prefixed tag is ambiguous too.
    #[test]
    fn a_bare_tag_among_same_named_packages_also_names_the_shared_name() {
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
        let err = resolve(&["v1.2.3"], &ws).expect_err("bare");
        assert_fixes(
            &err,
            &[Unattributed::BareAmongMany, Unattributed::SharedName],
        );
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
        let err = resolve(&["shared@1.2.3"], &ws).expect_err("polyglot");
        assert_eq!(err.tags(), &["shared@1.2.3".to_string()]);
        assert_fixes(&err, &[Unattributed::SharedName]);
    }

    #[test]
    fn two_versions_for_one_package_on_one_commit_are_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["linesmith/v0.2.0", "linesmith/v0.3.0"], &ws).expect_err("conflict");
        assert_eq!(
            err.tags(),
            &[
                "linesmith/v0.2.0".to_string(),
                "linesmith/v0.3.0".to_string()
            ]
        );
        assert_fixes(&err, &[Unattributed::TwoVersions]);
    }

    #[test]
    fn longer_package_name_wins_hyphen_prefix() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let got = resolve(&["linesmith-core-v0.4.1"], &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith-core")),
            Some(&ver("0.4.1"))
        );
    }

    #[test]
    fn ignored_tags_do_not_hide_a_real_release() {
        let ws = cargo_workspace(&["linesmith"]);
        let got = resolve(&["nightly", "linesmith/v0.4.1"], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.4.1"))
        );
    }

    #[test]
    fn prerelease_hyphen_on_an_unknown_package_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["other-v1.0.0-beta.1"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["other-v1.0.0-beta.1".to_string()]);
        assert_fixes(&err, &[Unattributed::UnknownPackage]);
    }

    #[test]
    fn uncovered_bare_next_to_a_prefixed_tag_is_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve(&["linesmith/v0.2.0", "v0.1.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn leftover_on_a_mixed_commit_discards_the_attributed_package() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["linesmith/v0.2.0", "other-v1.0.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["other-v1.0.0".to_string()]);
    }

    #[test]
    fn ignored_tag_does_not_drop_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve(&["nightly", "v0.1.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn hyphen_and_slash_at_different_versions_are_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["linesmith/v0.2.0", "linesmith-v0.3.0"], &ws).expect_err("conflict");
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
        let err = resolve(&["v0.2.3", "v0.8.4"], &ws).expect_err("conflict");
        assert_eq!(err.tags(), &["v0.2.3".to_string(), "v0.8.4".to_string()]);
    }

    #[test]
    fn prefixed_and_bare_at_different_versions_are_leftover() {
        let ws = npm_workspace(&["tsc-files"]);
        let err = resolve(&["tsc-files@0.2.3", "v0.8.4"], &ws).expect_err("conflict");
        assert_eq!(
            err.tags(),
            &["tsc-files@0.2.3".to_string(), "v0.8.4".to_string()]
        );
    }

    #[test]
    fn prefixed_and_bare_at_the_same_version_collapse() {
        let ws = npm_workspace(&["tsc-files"]);
        let got = resolve(&["tsc-files@0.2.3", "v0.2.3"], &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(
            got.get(&id(Ecosystem::Npm, "tsc-files")),
            Some(&ver("0.2.3"))
        );
    }

    #[test]
    fn unknown_at_and_slash_v_prereleases_are_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err =
            resolve(&["other@1.0.0-beta.1", "other/v1.0.0-beta.1"], &ws).expect_err("leftover");
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
        let err = resolve(&["v1.2.3"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v1.2.3".to_string()]);
    }

    #[test]
    fn hyphen_without_v_on_an_unknown_package_is_leftover() {
        let ws = cargo_workspace(&["linesmith"]);
        let err = resolve(&["other-1.0.0"], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["other-1.0.0".to_string()]);
    }

    #[test]
    fn two_uncovered_bares_in_a_multi_package_workspace_are_leftover() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core", "linesmith-plugin"]);
        let err = resolve(&["v0.1.0", "v0.1.1"], &ws).expect_err("leftover");
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
        let got = current(&[&["linesmith/v0.1.0"], &["linesmith/v0.2.0"]], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.2.0"))
        );
        let got = current(&[&["linesmith/v0.2.0"], &["linesmith/v0.1.0"]], &ws).expect("ok");
        assert_eq!(
            got.get(&id(Ecosystem::Cargo, "linesmith")),
            Some(&ver("0.2.0"))
        );
    }

    #[test]
    fn current_versions_keeps_packages_from_earlier_commits() {
        let ws = cargo_workspace(&["linesmith", "linesmith-core"]);
        let got = current(&[&["linesmith/v0.1.0"], &["linesmith-core/v0.2.0"]], &ws).expect("ok");
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
        let err = current(&[&["linesmith/v0.2.0"], &["v0.1.0"]], &ws).expect_err("leftover");
        assert_eq!(err.tags(), &["v0.1.0".to_string()]);
    }

    #[test]
    fn tagged_current_when_manifest_matches_the_tag() {
        let ws = cargo_at("linesmith", "0.1.0");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let got = tagged_current(&ws, &tagged, Package::publishable);
        assert_eq!(got, vec![(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
    }

    #[test]
    fn tagged_current_skips_drift() {
        let ws = cargo_at("linesmith", "0.2.0");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let got = tagged_current(&ws, &tagged, Package::publishable);
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn tagged_current_uses_the_tagged_version_not_build_metadata() {
        let ws = cargo_at("linesmith", "0.1.0+local");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        assert_eq!(
            tagged_current(&ws, &tagged, Package::publishable),
            vec![(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]
        );
    }

    #[test]
    fn drift_when_manifest_is_above_the_tag() {
        let ws = cargo_at("linesmith", "0.2.0");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let got = drift(&ws, &tagged, Package::publishable);
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
        let got = drift(&ws, &tagged, Package::publishable);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), &id(Ecosystem::Cargo, "ahead"));
        assert_eq!(got[0].manifest(), &ver("0.2.0"));
        assert_eq!(got[0].tagged(), &ver("0.1.0"));
    }

    #[test]
    fn no_drift_when_manifest_matches_or_is_behind() {
        let ws = cargo_at("linesmith", "0.1.0");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let got = drift(&ws, &tagged, Package::publishable);
        assert!(got.is_empty(), "{got:?}");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.2.0"))]);
        let got = drift(&ws, &tagged, Package::publishable);
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn behind_only_when_the_manifest_is_below_its_tag() {
        let ws = cargo_at("linesmith", "0.1.0+local");
        let at = |tag: &str| BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver(tag))]);
        let got = behind(&ws, &at("0.2.0"), Package::publishable);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].manifest(), &ver("0.1.0+local"));
        assert_eq!(got[0].tagged(), &ver("0.2.0"));
        for tag in ["0.1.0", "0.1.0+build", "0.0.9"] {
            assert_eq!(behind(&ws, &at(tag), Package::publishable), [], "{tag}");
        }
        assert_eq!(behind(&ws, &BTreeMap::new(), Package::publishable), []);
        let plain = cargo_at("linesmith", "0.1.0");
        assert_eq!(behind(&plain, &at("0.1.0+build"), Package::publishable), []);
    }

    #[test]
    fn no_tag_is_not_drift() {
        let ws = cargo_at("linesmith", "0.2.0");
        let got = drift(&ws, &BTreeMap::new(), Package::publishable);
        assert!(got.is_empty(), "{got:?}");
    }

    /// `(name, version, has_section)` for each pending package, with a
    /// changelog section for the packages named in `sections`.
    fn pending(
        ws: &Workspace,
        tagged: &BTreeMap<PackageId, Version>,
        sections: &[&str],
    ) -> Vec<(String, String, bool)> {
        untagged_pending(ws, tagged, Package::publishable, |package| {
            Ok::<_, core::convert::Infallible>(sections.contains(&package.id().name.as_str()))
        })
        .0
        .into_iter()
        .map(|item| {
            (
                item.id().name.clone(),
                item.version().to_string(),
                item.has_section(),
            )
        })
        .collect()
    }

    fn owed(name: &str, version: &str, has_section: bool) -> Vec<(String, String, bool)> {
        vec![(String::from(name), String::from(version), has_section)]
    }

    #[test]
    fn placeholders_without_a_section_owe_no_tag() {
        let none = BTreeMap::new();
        for version in ["0.0.0", "0.1.0", "0.1.0+local"] {
            let ws = cargo_at("linesmith", version);
            assert!(pending(&ws, &none, &[]).is_empty(), "{version}");
        }
    }

    #[test]
    fn a_version_with_a_section_is_owed_a_tag_whatever_its_number() {
        let none = BTreeMap::new();
        for version in ["0.0.0", "0.0.1", "0.1.0", "0.2.0"] {
            let ws = cargo_at("linesmith", version);
            assert_eq!(
                pending(&ws, &none, &["linesmith"]),
                owed("linesmith", version, true),
                "{version}"
            );
        }
    }

    #[test]
    fn a_hand_set_version_is_owed_a_tag_below_0_1_0_too() {
        let none = BTreeMap::new();
        for version in ["0.0.1", "0.0.5", "0.1.1", "0.2.0", "0.1.0-rc.1"] {
            let ws = cargo_at("linesmith", version);
            assert_eq!(
                pending(&ws, &none, &[]),
                owed("linesmith", version, false),
                "{version}"
            );
        }
    }

    #[test]
    fn a_tagged_package_is_never_untagged_pending() {
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let ws = cargo_at("linesmith", "0.2.0");
        let got = pending(&ws, &tagged, &["linesmith"]);
        assert!(got.is_empty(), "{got:?}");
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
        assert_eq!(pending(&ws, &tagged, &[]), owed("other", "0.2.0", false));
    }

    #[test]
    fn an_unreadable_changelog_is_unread_only_where_it_decides() {
        let packages: Vec<_> = ["0.1.0", "0.0.0", "0.2.0", "0.0.5"]
            .iter()
            .map(|version| {
                Package::new(
                    PackageId::new(Ecosystem::Cargo, format!("at-{version}")),
                    ver(version),
                    ResolvesDependenciesAt::Install,
                    true,
                    Vec::new(),
                )
            })
            .collect();
        let ws = Workspace::new(packages).expect("workspace");
        let (pending, unread) =
            untagged_pending(&ws, &BTreeMap::new(), Package::publishable, |package| {
                Err(package.id().name.clone())
            });
        let pending: Vec<_> = pending
            .iter()
            .map(|item| (item.id().name.as_str(), item.has_section()))
            .collect();
        assert_eq!(pending, vec![("at-0.0.5", false), ("at-0.2.0", false)]);
        assert_eq!(unread, vec!["at-0.0.0", "at-0.1.0"]);
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
        let got = drift(&ws, &tagged, Package::publishable);
        assert!(got.is_empty(), "{got:?}");
        let got = pending(&ws, &BTreeMap::new(), &[]);
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn build_metadata_does_not_count_as_ahead() {
        let ws = cargo_at("linesmith", "0.1.0+local");
        let tagged = BTreeMap::from([(id(Ecosystem::Cargo, "linesmith"), ver("0.1.0"))]);
        let got = drift(&ws, &tagged, Package::publishable);
        assert!(got.is_empty(), "{got:?}");
    }
}
