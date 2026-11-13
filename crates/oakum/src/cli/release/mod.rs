//! `oakum release` writes tags and GitHub releases (ADR-0023). Same local
//! preconditions as `check` (ADR-0020); no rollback (ADR-0011).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use clap::Args;
use semver::Version;
use serde_json::json;

use super::changelog;
use super::ci;
use super::config::{load_config, require_config, tag_managed_ids, LoadedConfig};
use super::git::{Commit, Followed, Git, Op};
use super::github::{self, Look};
use super::handoff;
use super::install_pin;
use super::preconditions::{self, PendingRelease, TagEvaluation};
use super::repository;
use super::tags::{self, Advertised};
use super::template::load_template_body;
use super::CliError;
use super::{deliver_out, say_err, say_out};
use oakum::config::{DEFAULT_MULTI_TAG_FORMAT, DEFAULT_SINGLE_TAG_FORMAT};

mod version_commit;
use version_commit::version_commit_for;

#[derive(Debug, Args)]
pub(super) struct ReleaseArgs {
    #[arg(long, value_name = "REF", help = super::FROM_MERGE_BASE)]
    from: Option<String>,
}

#[derive(Debug)]
struct PlannedTag {
    package: String,
    version: Version,
    /// Repository-relative changelog the release body is read from, at `commit`.
    changelog: String,
    /// From the current workspace, not the one at `commit`.
    shared_changelog: bool,
    /// Either passed `valid_tag_name` (`plan_tags`) or names a tag git
    /// already holds (`resume_candidates`); a new construction site owes one
    /// of the two.
    name: String,
    /// Where the tag points, or will. A resumed tag keeps the commit it was
    /// cut at; a new one names the commit that set its version.
    commit: Commit,
    /// A resumed tag already exists, so a release found for it is a finished
    /// state; for a new tag the same finding is a collision.
    resumed: bool,
}

impl PlannedTag {
    fn owner(&self) -> changelog::Owner<'_> {
        changelog::Owner::new(&self.package, self.shared_changelog)
    }
}

#[derive(Clone, Copy)]
enum Progress {
    Tagged,
    Pushed,
    /// The push failed and the re-read that would settle whether the ref
    /// landed failed too, so no stage can honestly be named.
    PushUnverified,
    /// The same, for a `git tag` and the local re-read after it.
    TagUnverified,
    Released,
}

/// What `release` has for reaching GitHub.
#[derive(Debug)]
enum GitHubAccess {
    /// `token` says whether a token is set beside the missing remote.
    NoRemote {
        token: bool,
    },
    NoToken,
    /// Both are present, but the repository's owner and name could not be
    /// read from the remote.
    Unresolved(CliError),
    Ready(GitHubTarget),
}

#[derive(Clone, Debug)]
struct GitHubTarget {
    remote: String,
    token: String,
    owner: String,
    repo_name: String,
}

#[derive(Debug)]
enum ReleaseDecision {
    NothingToRelease,
    Refused(CliError),
    Proceed(ReleasePlan),
}

#[derive(Debug)]
struct ReleasePlan {
    planned: Vec<PlannedTag>,
    advertised: Advertised,
    github: GitHubTarget,
}

/// Inputs gathered before policy runs. Every look belongs here; `decide_release`
/// has no I/O.
struct ReleaseReadiness {
    had_planned_before_resume: bool,
    resumable: Vec<ExistingTag>,
    access: GitHubAccess,
    preflight: Preflight,
    skip_ci_error: Option<CliError>,
    worktree_dirty: bool,
}

/// What GitHub said about the plan, or why it was not asked.
#[derive(Debug)]
enum Preflight {
    NothingPlanned,
    Refused(CliError),
    /// The tags still owed work, which may be none once GitHub has answered.
    Asked {
        planned: Vec<PlannedTag>,
        advertised: Advertised,
        github: GitHubTarget,
    },
}

/// Where a non-empty plan's GitHub release question goes, or why it cannot be
/// asked. No remote is `unverified` and no token a plain refusal, so the two
/// exit differently.
fn decide_preflight(access: &GitHubAccess) -> Result<&GitHubTarget, CliError> {
    match access {
        GitHubAccess::NoRemote { .. } => Err(CliError::unverified(
            "unverified: this repository has no remotes to push tags to",
        )),
        GitHubAccess::NoToken => Err(CliError::new(
            "`oakum release` needs GITHUB_TOKEN or GH_TOKEN",
        )),
        GitHubAccess::Unresolved(err) => Err(unverified_look(err)),
        GitHubAccess::Ready(github) => Ok(github),
    }
}

/// Release policy as a pure function over gathered readiness.
fn decide_release(readiness: ReleaseReadiness) -> ReleaseDecision {
    if !readiness.had_planned_before_resume {
        if !readiness.resumable.is_empty() {
            if let Err(err) = refuse_without_a_look(&readiness.access) {
                return ReleaseDecision::Refused(err);
            }
        }
        // `Unresolved` falls through: its preflight error is the answer.
        if matches!(
            readiness.access,
            GitHubAccess::NoRemote { .. } | GitHubAccess::NoToken
        ) {
            return ReleaseDecision::NothingToRelease;
        }
    }
    let (planned, advertised, github) = match readiness.preflight {
        Preflight::Refused(err) => return ReleaseDecision::Refused(err),
        Preflight::NothingPlanned => return ReleaseDecision::NothingToRelease,
        Preflight::Asked { planned, .. } if planned.is_empty() => {
            return ReleaseDecision::NothingToRelease
        }
        Preflight::Asked {
            planned,
            advertised,
            github,
        } => (planned, advertised, github),
    };
    if let Some(err) = readiness.skip_ci_error {
        return ReleaseDecision::Refused(err);
    }
    if readiness.worktree_dirty {
        return ReleaseDecision::Refused(CliError::new(
            "working tree is dirty; commit or stash before releasing",
        ));
    }
    ReleaseDecision::Proceed(ReleasePlan {
        planned,
        advertised,
        github,
    })
}

fn apply_decision(
    decision: ReleaseDecision,
    git: &Git,
    holders: &Holders<'_>,
) -> Result<(), CliError> {
    match decision {
        ReleaseDecision::NothingToRelease => {
            say_out("nothing to release");
            Ok(())
        }
        ReleaseDecision::Refused(err) => Err(err),
        ReleaseDecision::Proceed(plan) => {
            let client = github::Client::new(plan.github.token)?;
            let target = Target {
                client: &client,
                owner: &plan.github.owner,
                name: &plan.github.repo_name,
                holders,
            };
            act(
                git,
                &target,
                &plan.github.remote,
                &plan.planned,
                &plan.advertised,
            )
        }
    }
}

/// Naming which prerequisite is absent is the difference between a user who
/// sets a token and one who reads `nothing to release` and believes it.
///
/// Both present is necessary, not sufficient: a remote that is not a GitHub
/// URL still fails later in `ci::repository_slug_from`.
fn refuse_without_a_look(access: &GitHubAccess) -> Result<(), CliError> {
    let (remote, token) = match *access {
        GitHubAccess::NoRemote { token } => (false, token),
        GitHubAccess::NoToken => (true, false),
        GitHubAccess::Unresolved(_) | GitHubAccess::Ready(_) => return Ok(()),
    };
    let absent: Vec<&str> = [
        (!remote).then_some("this repository has no remote"),
        (!token).then_some("GITHUB_TOKEN and GH_TOKEN are both unset"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if absent.is_empty() {
        return Ok(());
    }
    Err(CliError::unverified(format!(
        "unverified: a tagged version has no local plan, and {}, so oakum could \
         not ask GitHub whether it is missing its release",
        absent.join(", and ")
    )))
}

/// A current version whose tag already exists locally, at whatever commit it
/// names. `current()` matches only a reachable tag, so one left on abandoned
/// history never appears here.
struct ExistingTag {
    item: PendingRelease,
    name: String,
    commit: Commit,
}

/// The tags the resume nominates for GitHub's release question, derived once
/// so the could-not-look gate and the resume itself cannot disagree about the
/// set. Name and commit are read in one pass so the two cannot drift apart.
fn existing_tags(
    git: &Git,
    release: &ReleaseContext,
    evaluation: &TagEvaluation,
) -> Result<Vec<ExistingTag>, CliError> {
    let reachable = release.loaded.reachable.tags(git)?;
    let listed: BTreeMap<&str, &str> = reachable
        .iter()
        .flat_map(|group| {
            group
                .tags()
                .iter()
                .map(|tag| (tag.as_str(), group.commit()))
        })
        .collect();
    let mut found = Vec::new();
    for item in evaluation.current() {
        // The tagged version carries the tag's build metadata, so a format
        // that adds some renders it twice; the manifest's version does not,
        // and is what a fresh run of that format titles the release.
        let mut candidates = vec![(render_tag(&release.template, &item)?, item.clone())];
        if let Some(package) = release.loaded.workspace.get(item.id()) {
            let manifest = PendingRelease::new(item.id().clone(), package.version().clone());
            let name = render_tag(&release.template, &manifest)?;
            if candidates.iter().all(|(known, _)| *known != name) {
                candidates.push((name, manifest));
            }
        }
        for (name, item) in candidates {
            let commit = match listed.get(name.as_str()) {
                Some(sha) => Some(Commit::from_tag_listing(sha)?),
                None => local_tag_commit(git, &name)?,
            };
            if let Some(commit) = commit {
                found.push(ExistingTag { item, name, commit });
                break;
            }
        }
    }
    Ok(found)
}

pub(super) fn run(args: &ReleaseArgs) -> Result<(), CliError> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    require_config(&config)?;
    let pins = install_pin::read_pin(&repo);
    install_pin::refuse_mismatch(&pins)?;
    let git = Git::at_repository(&repo)?;
    let loaded = preconditions::Loaded::discover(&repo, config)?;
    let evaluation = preconditions::evaluate(&git, &repo, &loaded, pins, args.from.as_deref())?;
    let pending = evaluation.pending();
    // Ahead of `plan_tags`, whose walk reads committed history: a version only
    // the working tree holds would otherwise read as one no commit set.
    let worktree_dirty = worktree_is_dirty(&git)?;
    if !pending.is_empty() && worktree_dirty {
        return Err(CliError::new(
            "working tree is dirty; commit or stash before releasing",
        ));
    }
    let release = ReleaseContext::new(&repo, loaded)?;
    let planned_before_resume = if pending.is_empty() {
        Vec::new()
    } else {
        plan_tags(&git, &release, &pending)?
    };
    let resumable = existing_tags(&git, &release, &evaluation)?;
    let remote = tags::first_remote(&git)?;
    let listing = refuse_unread_tags(&git, &release, &pending, &resumable, remote.as_deref())?;
    let access = match (remote, github::token()) {
        (Some(remote), Some(token)) => match ci::repository_slug_from(&git, &remote) {
            Ok((owner, repo_name)) => GitHubAccess::Ready(GitHubTarget {
                remote,
                token,
                owner,
                repo_name,
            }),
            Err(err) => GitHubAccess::Unresolved(err),
        },
        (None, token) => GitHubAccess::NoRemote {
            token: token.is_some(),
        },
        (Some(_), None) => GitHubAccess::NoToken,
    };
    let had_planned_before_resume = !planned_before_resume.is_empty();
    let mut planned = planned_before_resume;
    planned.extend(resume_candidates(&release, &resumable, &planned)?);
    let preflight = if planned.is_empty() {
        Preflight::NothingPlanned
    } else {
        match decide_preflight(&access) {
            Err(err) => Preflight::Refused(err),
            Ok(github) => {
                let client = github::Client::new(github.token.clone())?;
                match preflight(
                    &git,
                    &client,
                    &github.owner,
                    &github.repo_name,
                    &github.remote,
                    planned,
                    listing,
                ) {
                    Ok((planned, advertised)) => Preflight::Asked {
                        planned,
                        advertised,
                        github: github.clone(),
                    },
                    Err(err) => Preflight::Refused(err),
                }
            }
        }
    };
    let skip_ci_error = match &preflight {
        Preflight::Asked { planned, .. } => refuse_skip_ci(&git, planned).err(),
        _ => None,
    };
    let readiness = ReleaseReadiness {
        had_planned_before_resume,
        resumable,
        access,
        preflight,
        skip_ci_error,
        worktree_dirty,
    };
    apply_decision(decide_release(readiness), &git, &release.holders())
}

/// What one release reads once and every helper shares: the config and
/// validated workspace, the packages a bare `v` tag may belong to, and the
/// tag template.
struct ReleaseContext {
    loaded: preconditions::Loaded,
    bare_candidates: BTreeSet<oakum::plan::PackageId>,
    template: String,
}

impl ReleaseContext {
    fn new(repo: &repository::Repository, loaded: preconditions::Loaded) -> Result<Self, CliError> {
        let bare_candidates = tag_managed_ids(&loaded.workspace, &loaded.config);
        let template = tag_template_for(repo, &loaded.config, bare_candidates.len())?;
        Ok(Self {
            loaded,
            bare_candidates,
            template,
        })
    }

    fn holders(&self) -> Holders<'_> {
        Holders {
            workspace: &self.loaded.workspace,
            bare_candidates: &self.bare_candidates,
        }
    }
}

fn plan_tags(
    git: &Git,
    release: &ReleaseContext,
    items: &[PendingRelease],
) -> Result<Vec<PlannedTag>, CliError> {
    let config = &release.loaded.config;
    let workspace = &release.loaded.workspace;
    let bare_candidates = &release.bare_candidates;
    let template = &release.template;
    let head = git.head()?;
    let mut rendered = Vec::new();
    let mut names = BTreeSet::new();
    for item in items {
        let name = render_tag(template, item)?;
        if !names.insert(name.clone()) {
            return Err(CliError::new(format!(
                "tag-format rendered `{name}` for more than one package"
            )));
        }
        // Ahead of `readable_for_package` below, so a name git rejects
        // outright is reported as an invalid ref rather than as an
        // unreadable tag.
        valid_tag_name(git, &name)?;
        rendered.push((item, name));
    }
    let mut planned = Vec::new();
    for (item, name) in rendered {
        readable_for_package(workspace, item, &name, |id| bare_candidates.contains(id))?;
        let (changelog, shared_changelog) = changelog_for(workspace, config, item)?;
        planned.push(PlannedTag {
            package: item.id().name.clone(),
            version: item.version().clone(),
            changelog,
            shared_changelog,
            name,
            commit: version_commit_for(git, workspace, config, item, &head)?,
            resumed: false,
        });
    }
    Ok(planned)
}

/// Nominates only: whether a candidate is still owed a release is GitHub's
/// answer, and `preflight` is the one place that asks it. A candidate's name
/// needs no `valid_tag_name` pass — it names a tag git already holds.
fn resume_candidates(
    release: &ReleaseContext,
    resumable: &[ExistingTag],
    already: &[PlannedTag],
) -> Result<Vec<PlannedTag>, CliError> {
    let config = &release.loaded.config;
    let workspace = &release.loaded.workspace;
    let bare_candidates = &release.bare_candidates;
    let planned: BTreeSet<&str> = already.iter().map(|tag| tag.name.as_str()).collect();
    let mut extra = Vec::new();
    for candidate in resumable {
        if planned.contains(candidate.name.as_str()) {
            continue;
        }
        readable_for_package(workspace, &candidate.item, &candidate.name, |id| {
            bare_candidates.contains(id)
        })?;
        let (changelog, shared_changelog) = changelog_for(workspace, config, &candidate.item)?;
        extra.push(PlannedTag {
            package: candidate.item.id().name.clone(),
            version: candidate.item.version().clone(),
            changelog,
            shared_changelog,
            name: candidate.name.clone(),
            commit: candidate.commit.clone(),
            resumed: true,
        });
    }
    Ok(extra)
}

/// The shape written when `tag-format` is unset. Bare vs package-prefixed
/// follows the tag-managed set (ADR-0030): unmanaged members do not force a
/// prefixed write shape.
pub(super) const fn default_tag_template(managed_count: usize) -> &'static str {
    if managed_count <= 1 {
        DEFAULT_SINGLE_TAG_FORMAT
    } else {
        DEFAULT_MULTI_TAG_FORMAT
    }
}

fn tag_template_for(
    repo: &repository::Repository,
    config: &LoadedConfig,
    managed_count: usize,
) -> Result<String, CliError> {
    let default = default_tag_template(managed_count);
    match config.tag_format() {
        Some(source) => {
            load_template_body(repo.dir(), repo.path(), source).map_err(CliError::from_boxed)
        }
        None => Ok(default.to_owned()),
    }
}

/// The tag `release` would cut for a package at `version`, for advice that
/// names it; `None` when `tag-format` does not render, or renders a tag that
/// would not read back as that package's version, which no advice should ask
/// anyone to create.
pub(super) fn tag_namer<'a>(
    repo: &repository::Repository,
    config: &LoadedConfig,
    workspace: &'a oakum::plan::Workspace,
) -> impl Fn(&oakum::plan::PackageId, &Version) -> Option<String> + 'a {
    let bare_candidates = tag_managed_ids(workspace, config);
    let template = tag_template_for(repo, config, bare_candidates.len()).ok();
    move |id, version| {
        let name = render_tag_for(template.as_deref()?, &id.name, version).ok()?;
        let item = PendingRelease::new(id.clone(), version.clone());
        readable_for_package(workspace, &item, &name, |id| bare_candidates.contains(id)).ok()?;
        Some(name)
    }
}

fn render_tag(template: &str, item: &PendingRelease) -> Result<String, CliError> {
    render_tag_for(template, &item.id().name, item.version())
}

/// The variables `tag-format` renders with. Extracted so the schema guard in
/// this file's tests reads the context the renderer actually passes: against a
/// second hand-written literal it compared one list to another, and a field
/// added here left the guard green (measured).
fn tag_context(package: &str, version: &Version) -> serde_json::Value {
    json!({
        "package": package,
        "version": version.to_string(),
    })
}

fn render_tag_for(template: &str, package: &str, version: &Version) -> Result<String, CliError> {
    let rendered = oakum::template::render("tag-format", template, tag_context(package, version))
        .map_err(|err| CliError::new(err.to_string()))?;
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return Err(CliError::new("tag-format rendered an empty string"));
    }
    if rendered.chars().any(char::is_whitespace) {
        return Err(CliError::new(format!(
            "tag-format rendered whitespace: {rendered:?}"
        )));
    }
    Ok(rendered.to_owned())
}

/// The changelog the release body is read from, and whether it is shared.
fn changelog_for(
    workspace: &oakum::plan::Workspace,
    config: &LoadedConfig,
    item: &PendingRelease,
) -> Result<(String, bool), CliError> {
    let package = workspace.get(item.id()).ok_or_else(|| {
        CliError::new(format!(
            "{} is planned for release but not in the workspace",
            item.id().name
        ))
    })?;
    Ok((
        changelog::changelog_repo_path(package),
        changelog::Owner::of(workspace, config, package).shared(),
    ))
}

/// A tag `act` still owes a release for, with the body it will send. Built
/// after preflight and before any git write, so a resumed tag that already
/// has its release never has its changelog read, and a changelog that cannot
/// be read stops the run with nothing tagged.
struct OwedRelease<'a> {
    tag: &'a PlannedTag,
    body: String,
    /// Why `body` is the title instead of a changelog section. Printed at the
    /// release, not at planning, so a run that stops earlier never emits it.
    notice: Option<String>,
}

fn load_release_bodies<'a>(
    git: &Git,
    planned: &'a [PlannedTag],
) -> Result<Vec<OwedRelease<'a>>, CliError> {
    planned
        .iter()
        .map(|tag| {
            let (body, notice) = release_body(git, tag)?.resolve(tag);
            Ok(OwedRelease { tag, body, notice })
        })
        .collect()
}

/// Where a release body comes from: the changelog section for the version
/// (ADR-0032 anticipates the paste), or one of the reasons it is the title
/// instead. The heading is dropped because the release page shows the version.
enum BodySource {
    Section(String),
    NoChangelog,
    /// A link git would not follow to a file in that commit.
    Unfollowed {
        why: &'static str,
    },
    EmptySection,
    NoHeading,
    /// Why [`changelog::section_or_refusal`] cannot tell.
    Undecided(String),
}

impl BodySource {
    /// The body to send and, for a fallback, the notice that explains it.
    fn resolve(self, tag: &PlannedTag) -> (String, Option<String>) {
        let (path, at) = (tag.changelog.as_str(), tag.commit.as_str());
        let remedy = match &self {
            Self::NoHeading => " (run `oakum version` before releasing, or add the section)",
            _ => "",
        };
        let why = match self {
            Self::Section(section) => return (format!("{section}\n"), None),
            Self::NoChangelog => format!("no {path} at {at}"),
            Self::Unfollowed { why } => format!("{path} at {at} is {why}"),
            Self::EmptySection => format!("{path} has an empty {} section", tag.version),
            Self::NoHeading => format!(
                "{path} has no `## {}` heading",
                tag.owner().heading(&tag.version)
            ),
            Self::Undecided(why) => why,
        };
        (
            format!("{} {}\n", tag.package, tag.version),
            Some(format!("release body: {why}; using the title{remedy}")),
        )
    }
}

/// A changelog read that failed, as the refusal names it: one that never
/// answered stays unverified.
fn changelog_read_failed(
    path: &str,
    commit: &Commit,
    purpose: &str,
    stopped: &str,
    err: &CliError,
) -> CliError {
    let message = format!(
        "could not read {} at {} {purpose}: {}; {stopped}",
        path.escape_debug(),
        commit.as_str(),
        err.detail()
    );
    if err.is_unanswered() {
        err.recast(message)
    } else {
        CliError::new(message)
    }
}

fn release_body(git: &Git, tag: &PlannedTag) -> Result<BodySource, CliError> {
    let path = tag.changelog.as_str();
    let commit = &tag.commit;
    // A resumed tag already exists; only its release is missing.
    let stopped = if tag.resumed {
        "no release was created"
    } else {
        "no tag was created"
    };
    let read_failed =
        |err: &CliError| changelog_read_failed(path, commit, "for the release body", stopped, err);
    let bytes = match git
        .followed_blob(commit, path)
        .map_err(|err| read_failed(&err))?
    {
        Followed::File(bytes) => bytes,
        Followed::Absent => return Ok(BodySource::NoChangelog),
        Followed::Dangling => {
            return Ok(BodySource::Unfollowed {
                why: "a symlink to a path not in that commit",
            })
        }
        Followed::Unfollowed(why) => return Ok(BodySource::Unfollowed { why }),
    };
    let text = String::from_utf8(bytes)
        .map_err(|_| read_failed(&CliError::new("its text is not valid UTF-8")))?;
    if let Err(why) =
        changelog::section_or_refusal(Path::new(path), &text, tag.owner(), &tag.version)
    {
        return Ok(BodySource::Undecided(why.detail()));
    }
    Ok(
        match changelog::section_for_version(&text, tag.owner(), &tag.version) {
            Some(section) if !section.is_empty() => BodySource::Section(section),
            Some(_) => BodySource::EmptySection,
            None => BodySource::NoHeading,
        },
    )
}

fn readable_for_package(
    workspace: &oakum::plan::Workspace,
    item: &PendingRelease,
    name: &str,
    bare_candidate: impl Fn(&oakum::plan::PackageId) -> bool,
) -> Result<(), CliError> {
    match oakum::tags::resolve_commit_tags(&[name], workspace, bare_candidate) {
        Ok(map) => match map.get(item.id()) {
            Some(version)
                if oakum::tags::without_build(version)
                    == oakum::tags::without_build(item.version()) =>
            {
                Ok(())
            }
            Some(version) => Err(CliError::new(format!(
                "tag-format rendered `{name}` as {} {version}, not {} {}",
                item.id().name,
                item.id().name,
                item.version()
            ))),
            None => Err(CliError::new(format!(
                "tag-format rendered `{name}`, which is not a readable tag for {}",
                item.id().name
            ))),
        },
        Err(_) => Err(CliError::new(format!(
            "tag-format rendered `{name}`, which later check would treat as leftover"
        ))),
    }
}

/// The tags the resume never asks about (okm-e9e.21), asked per tag-managed
/// package: when a package's current version has no reachable local tag at
/// the rendered name, every shape that could be hiding that version's state
/// — a tag on unreachable history, a corrupt tag object, a name that drifted
/// from the tag-format, a tag only the remote holds (under the rendered
/// name, an earlier format's name, or one that cannot be attributed at all)
/// — refuses as `unverified` rather than letting `nothing to release` claim
/// a look that never happened.
///
/// Before any package, `all_tag_objects` refuses a clone that may not hold
/// every tag (shallow, tag-suppressed, or fetching only some branches).
///
/// A package whose rendered tag is local and reachable is settled, and
/// [`refuse_unread_for_package`] returns before every scan for it: its release
/// state is read on the resume path, and no other tag — stale, foreign, or
/// unattributable — changes that verdict. A pending package is scanned only
/// for another tag naming its version. Repository-wide tag hygiene is
/// `check`'s question, not this gate's.
fn refuse_unread_tags(
    git: &Git,
    release: &ReleaseContext,
    pending: &[PendingRelease],
    resumable: &[ExistingTag],
    remote: Option<&str>,
) -> Result<Option<Advertised>, CliError> {
    let resumed: BTreeSet<&str> = resumable.iter().map(|tag| tag.name.as_str()).collect();
    let workspace = &release.loaded.workspace;
    let bare_candidates = &release.bare_candidates;
    let template = &release.template;
    let listed = tags::all_tag_objects(git)?;
    let reachable = release.loaded.reachable.names(git)?;
    let local_names: BTreeSet<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    let pending_ids: BTreeSet<_> = pending.iter().map(PendingRelease::id).collect();
    let local_resolved = resolve_tag_names(
        listed.iter().map(|(name, _)| name.as_str()),
        workspace,
        |id| bare_candidates.contains(id),
    );
    let mut advertised = None;
    // tag_managed mirrors tags::{drift,untagged_pending}: unmanaged packages owe no tag.
    for package in workspace
        .packages()
        .filter(|package| bare_candidates.contains(package.id()))
    {
        refuse_unread_for_package(
            &mut UnreadTagScan {
                git,
                template,
                listed: &listed,
                reachable: &reachable,
                local_names: &local_names,
                local_resolved: &local_resolved,
                resumed: &resumed,
                workspace,
                bare_candidates,
                remote,
                advertised: &mut advertised,
            },
            package,
            pending_ids.contains(package.id()),
        )?;
    }
    Ok(advertised.map(|(commits, _)| commits))
}

struct UnreadTagScan<'a> {
    git: &'a Git,
    template: &'a str,
    listed: &'a [(String, String)],
    reachable: &'a BTreeSet<String>,
    local_names: &'a BTreeSet<&'a str>,
    local_resolved: &'a ResolvedTags,
    /// The tags the resume asks GitHub about, which are read.
    resumed: &'a BTreeSet<&'a str>,
    workspace: &'a oakum::plan::Workspace,
    bare_candidates: &'a BTreeSet<oakum::plan::PackageId>,
    remote: Option<&'a str>,
    advertised: &'a mut Option<(tags::Advertised, ResolvedTags)>,
}

/// A `pending` package's rendered tag is the one the run makes or resumes, so
/// only another tag naming its version refuses: one the release would tag a
/// second time.
fn refuse_unread_for_package(
    ctx: &mut UnreadTagScan<'_>,
    package: &oakum::plan::Package,
    pending: bool,
) -> Result<(), CliError> {
    let package_name = &package.id().name;
    let version = package.version();
    let rendered = render_tag_for(ctx.template, package_name, version)?;
    let found = ctx.listed.iter().find(|(name, _)| *name == rendered);
    if let Some((_, object)) = found.filter(|_| !pending) {
        if ctx.reachable.contains(&rendered) {
            return Ok(());
        }
        return Err(match local_tag_commit(ctx.git, &rendered)? {
            Some(commit) => CliError::unverified(format!(
                "unverified: tag `{rendered}` for {package_name} {version} points at \
                 `{}` on history unreachable from HEAD, so the release scan never \
                 read it; merge that history or move the tag",
                commit.as_str()
            )),
            None => CliError::unverified(format!(
                "unverified: tag `{rendered}` for {package_name} {version} names \
                 object `{object}`, which cannot be read; the repository is corrupt \
                 where the release state would be"
            )),
        });
    }
    let resumed = ctx.resumed;
    let read = |name: &str| name == rendered || resumed.contains(name);
    if let Some(err) = scan_resolved_tags(
        ctx.local_resolved,
        (package.id(), version),
        read,
        |name| pending || ctx.reachable.contains(name),
        |name| {
            CliError::unverified(format!(
                "unverified: tag-format renders `{rendered}` for {package_name} \
                 {version}, but the tag that exists for that version is \
                 `{name}`; the resume asks only about `{rendered}`, so \
                 reconcile the tag-format with the existing tags"
            ))
        },
        |name| {
            CliError::unverified(format!(
                "unverified: tag `{name}` looks like a version but could not \
                 be attributed to a package, and it sits outside reachable \
                 history, so its release state was never read; reconcile or \
                 delete the tag"
            ))
        },
    ) {
        return Err(err);
    }
    let Some(remote) = ctx.remote else {
        return Ok(());
    };
    if ctx.advertised.is_none() {
        let commits = tags::remote_tag_commits(ctx.git, remote)?;
        let remote_resolved = resolve_tag_names(
            commits.tag_names().map(str::to_owned),
            ctx.workspace,
            |id| ctx.bare_candidates.contains(id),
        );
        *ctx.advertised = Some((commits, remote_resolved));
    }
    let (advertised_map, remote_resolved) = ctx.advertised.as_ref().expect("consulted above");
    // Settled across the whole listing before the resolution scan, as the
    // local side does: inside the loop, map order would pick which verdict
    // fires, and the earlier-format remedy does not clear this refusal.
    if !pending && advertised_map.contains_tag(&rendered) {
        return Err(CliError::unverified(format!(
            "unverified: tag `{rendered}` for {package_name} {version} exists on \
             remote {remote:?} but not locally, so the release scan never read it; \
             run `git fetch {remote} tag {rendered}` and rerun"
        )));
    }
    if let Some(err) = scan_resolved_tags(
        remote_resolved,
        (package.id(), version),
        read,
        |name| pending || ctx.local_names.contains(name),
        |name| {
            CliError::unverified(format!(
                "unverified: remote {remote:?} has tag `{name}`, which names \
                 {package_name} {version}, but the tag-format renders \
                 `{rendered}`; the release scan never read it — fetch it \
                 (`git fetch {remote} tag {name}`) or reconcile the tag-format"
            ))
        },
        |name| {
            CliError::unverified(format!(
                "unverified: remote {remote:?} has tag `{name}`, which looks \
                 like a version but could not be attributed to a package; the \
                 release scan never read it — fetch it (`git fetch {remote} \
                 tag {name}`) or reconcile the tag names"
            ))
        },
    ) {
        return Err(err);
    }
    Ok(())
}

type ResolvedTags = Vec<(
    String,
    Result<BTreeMap<oakum::plan::PackageId, Version>, ()>,
)>;

/// Resolved once so the per-package scans share the maps.
fn resolve_tag_names(
    names: impl IntoIterator<Item = impl Into<String>>,
    workspace: &oakum::plan::Workspace,
    bare_candidate: impl Fn(&oakum::plan::PackageId) -> bool,
) -> ResolvedTags {
    names
        .into_iter()
        .map(|name| {
            let name = name.into();
            let resolved = oakum::tags::resolve_commit_tags(&[&name], workspace, &bare_candidate)
                .map_err(|_| ());
            (name, resolved)
        })
        .collect()
}

/// Shared unread-tag ladder over every tag the run does not `read`:
/// maps-to-this-version refuses, build metadata aside (ADR-0030), other
/// resolutions pass, and an unattributable name refuses unless `excuse` says
/// otherwise.
fn scan_resolved_tags(
    tags: &ResolvedTags,
    (package_id, version): (&oakum::plan::PackageId, &Version),
    read: impl Fn(&str) -> bool,
    excuse: impl Fn(&str) -> bool,
    on_mismatch: impl Fn(&str) -> CliError,
    on_unattributable: impl Fn(&str) -> CliError,
) -> Option<CliError> {
    let version = Some(oakum::tags::without_build(version));
    for (name, resolved) in tags.iter().filter(|(name, _)| !read(name)) {
        match resolved {
            Ok(map) if map.get(package_id).map(oakum::tags::without_build) == version => {
                return Some(on_mismatch(name));
            }
            Ok(_) => {}
            Err(()) if excuse(name) => {}
            Err(()) => return Some(on_unattributable(name)),
        }
    }
    None
}

/// A strict subset of what git accepts under `refs/tags/`, decided without a
/// child; a name outside it is still git's to judge.
fn plainly_valid_tag_name(name: &str) -> bool {
    name.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._/@+-".contains(&byte))
        && !name.contains("..")
        && name.split('/').all(|component| {
            !component.is_empty()
                && !component.starts_with('.')
                && !component.ends_with('.')
                && !component.to_ascii_lowercase().ends_with(".lock")
        })
}

fn valid_tag_name(git: &Git, name: &str) -> Result<(), CliError> {
    if name.starts_with('-') {
        return Err(CliError::new(format!(
            "tag-format rendered an invalid git ref: {name:?}"
        )));
    }
    if plainly_valid_tag_name(name) {
        return Ok(());
    }
    // `check-ref-format` validates syntax without reading a repository, but it
    // is still a git child: it goes through the repository's handle so a caller
    // cannot be validated against whatever the process cwd happens to be.
    if git.predicate(Op::ValidRefName { reference: name })? {
        return Ok(());
    }
    Err(CliError::new(format!(
        "tag-format rendered an invalid git ref: {name:?}"
    )))
}

/// The tags still owed work, each asked about on GitHub exactly once: a
/// resumed tag whose release already exists is a finished state and drops
/// out of the set, while the same finding for a new tag is a collision and
/// refuses. The lookups come first so a fully-released resume never contacts
/// the git remote; the advertised snapshot (the unread-tag scan's `listing`
/// if any) is read once and handed to `act`; `push_outcome` still re-reads
/// on its failure path to settle what a died push did.
fn preflight(
    git: &Git,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: Vec<PlannedTag>,
    listing: Option<Advertised>,
) -> Result<(Vec<PlannedTag>, Advertised), CliError> {
    let mut owed = Vec::new();
    for tag in planned {
        match client.release_for_tag(owner, name, &tag.name)? {
            Look::Empty => owed.push(tag),
            Look::Found(_) if tag.resumed => {}
            Look::Found(release) => {
                return Err(CliError::new(format!(
                    "GitHub already has a release for `{}` ({})",
                    tag.name, release.html_url
                )));
            }
        }
    }
    if owed.is_empty() {
        return Ok((owed, Advertised::default()));
    }
    let advertised = match listing {
        Some(advertised) => advertised,
        None => tags::remote_tag_commits(git, remote)?,
    };
    for tag in &owed {
        if !advertised.points_at(&tag.name, &tag.commit) {
            if let Some(existing) = advertised.get(&tag.name) {
                return Err(CliError::new(format!(
                    "remote {remote:?} already has tag `{}` at `{}`, not `{}`",
                    tag.name,
                    existing.as_str(),
                    tag.commit.as_str()
                )));
            }
        }
        if let Some(existing) = local_tag_commit(git, &tag.name)? {
            if existing != tag.commit {
                return Err(CliError::new(format!(
                    "local tag `{}` points at `{}`, not `{}`",
                    tag.name,
                    existing.as_str(),
                    tag.commit.as_str()
                )));
            }
        }
    }
    Ok((owed, advertised))
}

fn act(
    git: &Git,
    target: &Target<'_>,
    remote: &str,
    planned: &[PlannedTag],
    advertised: &Advertised,
) -> Result<(), CliError> {
    let owed = load_release_bodies(git, planned)?;
    let mut completed = Vec::new();
    // Opened before anything is written, so a failed look cannot strand a
    // pushed tag outside the partial-failure report.
    let mut handoff = handoff::Handoff::open(
        target.client,
        target.owner,
        target.name,
        git,
        planned.iter().map(|tag| &tag.commit),
    )?;
    for (index, release) in owed.iter().enumerate() {
        let tag = release.tag;
        let released = |completed: &[String], err: &CliError| {
            partial_failure(completed, Some(Progress::Released), planned, index, err)
        };
        match release_one(git, target, remote, release, advertised) {
            Ok(did_push) => {
                match handoff.confirm_push(&tag.name, &tag.commit, did_push) {
                    Ok(None) => {}
                    Ok(Some(run)) => {
                        if !run.html_url.is_empty() {
                            if let Err(err) = deliver_out(&run.html_url) {
                                return Err(released(
                                    &completed,
                                    &CliError::undelivered(
                                        format!(
                                            "downstream run {} found, but its URL",
                                            run.html_url
                                        ),
                                        &err,
                                    ),
                                ));
                            }
                        }
                        let commit_recurs = planned[index + 1..]
                            .iter()
                            .any(|later| later.commit == tag.commit);
                        if commit_recurs {
                            if let Err(err) = handoff.absorb_before_next(&tag.commit) {
                                return Err(released(&completed, &err));
                            }
                        }
                    }
                    Err(err) => {
                        return Err(released(&completed, &err));
                    }
                }
                let line = format!("{} {} {}", tag.package, tag.version, tag.name);
                if let Err(err) = deliver_out(&line) {
                    return Err(released(
                        &completed,
                        &CliError::undelivered(
                            format!("{} released, but the line saying so", tag.name),
                            &err,
                        ),
                    ));
                }
                completed.push(tag.name.clone());
            }
            Err((progress, err)) => {
                return Err(partial_failure(&completed, progress, planned, index, &err));
            }
        }
    }
    Ok(())
}

/// The GitHub repository `release_one` creates releases in.
struct Target<'a> {
    client: &'a github::Client,
    owner: &'a str,
    name: &'a str,
    holders: &'a Holders<'a>,
}

fn release_one(
    git: &Git,
    target: &Target<'_>,
    remote: &str,
    release: &OwedRelease<'_>,
    advertised: &Advertised,
) -> Result<bool, (Option<Progress>, CliError)> {
    let Target {
        client,
        owner,
        name,
        holders: _,
    } = *target;
    let tag = release.tag;
    let mut progress = None;
    let remote_has_it = advertised.points_at(&tag.name, &tag.commit);
    if local_tag_commit(git, &tag.name)
        .map_err(|err| (progress, err))?
        .is_none()
        && !remote_has_it
    {
        // Only a planned tag reaches this: a resumed one already exists, which
        // is what `existing_tags` selected it for. So the commit here is the
        // one that set the version.
        git.run(Op::AnnotatedTag {
            name: &tag.name,
            commit: &tag.commit,
        })
        .map_err(|err| {
            if err.is_unanswered() {
                tag_outcome(git, tag, &err)
            } else {
                (progress, err)
            }
        })?;
        progress = Some(Progress::Tagged);
    }
    let did_push = !remote_has_it;
    if did_push {
        git.run(Op::PushTag {
            remote,
            tag: &tag.name,
        })
        .map_err(|err| push_outcome(git, remote, tag, progress, &err))?;
    }
    progress = Some(Progress::Pushed);
    let title = format!("{} {}", tag.package, tag.version);
    if let Some(notice) = &release.notice {
        say_err(notice);
    }
    let body = release.body.as_str();
    let (latest, note) = latest_for(target, tag);
    let created = client
        .create_release(owner, name, &tag.name, &title, body, latest)
        .map_err(|err| (progress, CliError::from(err)))?;
    progress = Some(Progress::Released);
    if let Some(note) = note {
        say_err(&note);
    }
    deliver_out(&created.html_url).map_err(|err| {
        (
            progress,
            CliError::undelivered(
                format!("release {} created, but its URL", created.html_url),
                &err,
            ),
        )
    })?;
    Ok(did_push)
}

/// Whether the release takes Latest, and the line to say once it exists when
/// Latest stays on another package's release or could not be read. Read
/// before each create: an earlier release in this run may hold it. Unread,
/// the release is still created, without taking Latest: the tag is already
/// pushed and owes its release.
fn latest_for(target: &Target<'_>, tag: &PlannedTag) -> (bool, Option<String>) {
    if !tag.version.pre.is_empty() {
        return (false, None);
    }
    let moves_it = format!("`gh release edit {} --latest` moves it", tag.name);
    match target.client.latest_release_holder(target.owner, target.name) {
        Ok(Look::Found(held)) => {
            let holder = target.holders.of(&held);
            let latest = takes_latest(&tag.package, &tag.version, &holder);
            let ours = matches!(&holder, Holder::Package { name, .. } if *name == tag.package);
            let note = (!latest && !ours)
                .then(|| format!("{}: Latest stays on {held}; {moves_it}", tag.name));
            (latest, note)
        }
        Ok(Look::Empty) => (takes_latest(&tag.package, &tag.version, &Holder::None), None),
        Err(err) => (
            false,
            Some(format!(
                "{}: could not read which release is Latest ({err}); created without taking Latest; {moves_it}",
                tag.name
            )),
        ),
    }
}

/// The release GitHub shows as Latest, read as this workspace's tags are.
#[derive(Debug, PartialEq, Eq)]
enum Holder {
    None,
    Package { name: String, version: Version },
    Other,
}

/// What attributes a holder's tag to a package: the workspace and the
/// packages a bare `v` tag may belong to, as every other tag read uses.
struct Holders<'a> {
    workspace: &'a oakum::plan::Workspace,
    bare_candidates: &'a BTreeSet<oakum::plan::PackageId>,
}

impl Holders<'_> {
    fn of(&self, tag: &str) -> Holder {
        let resolved = oakum::tags::resolve_commit_tags(&[tag], self.workspace, |id| {
            self.bare_candidates.contains(id)
        });
        match resolved
            .map(BTreeMap::into_iter)
            .map(|mut found| (found.next(), found.next()))
        {
            Ok((Some((id, version)), None)) => Holder::Package {
                name: id.name,
                version,
            },
            _ => Holder::Other,
        }
    }
}

/// GitHub keeps one Latest release per repository, and oakum cannot rank one
/// package's version against another's, so a release takes Latest only from
/// none, or from an earlier version of the same package, and never as a
/// prerelease: a library release must not take it from the binary whose
/// installer URL reads it.
fn takes_latest(package: &str, version: &Version, holder: &Holder) -> bool {
    if !version.pre.is_empty() {
        return false;
    }
    match holder {
        Holder::None => true,
        Holder::Package {
            name,
            version: held,
        } => name == package && held.cmp_precedence(version).is_lt(),
        Holder::Other => false,
    }
}

/// A failed push may still have landed the ref — measured, a `git push`
/// killed after the remote accepted it — and the stage decides the user's
/// recovery under ADR-0011, so the remote is re-read rather than reporting
/// the stage from before the push.
fn push_outcome(
    git: &Git,
    remote: &str,
    tag: &PlannedTag,
    progress: Option<Progress>,
    err: &CliError,
) -> (Option<Progress>, CliError) {
    // A push that never answered is unverified until the re-read settles
    // whether it landed; once settled, the stop is a plain failure.
    match tags::remote_tag_commits(git, remote) {
        Ok(advertised) if advertised.points_at(&tag.name, &tag.commit) => {
            (Some(Progress::Pushed), CliError::new(err.detail()))
        }
        Ok(_) => (progress, CliError::new(err.detail())),
        Err(reread) => (
            Some(Progress::PushUnverified),
            reread_failed(err, "the tag landed", &reread),
        ),
    }
}

/// The tag counterpart of [`push_outcome`], for a `git tag` that never
/// answered: it may have written the tag, and the local re-read settles which
/// stage to name. An answered failure wrote nothing and keeps its exit.
fn tag_outcome(git: &Git, tag: &PlannedTag, err: &CliError) -> (Option<Progress>, CliError) {
    match local_tag_commit(git, &tag.name) {
        Ok(Some(commit)) if commit == tag.commit => {
            (Some(Progress::Tagged), CliError::new(err.detail()))
        }
        Ok(_) => (None, CliError::new(err.detail())),
        Err(reread) => (
            Some(Progress::TagUnverified),
            reread_failed(err, "the tag was written", &reread),
        ),
    }
}

/// Both errors shed their outcome token: one verdict, said once, at the front.
fn reread_failed(err: &CliError, settles: &str, reread: &CliError) -> CliError {
    CliError::unverified(format!(
        "{}; the re-read that would settle whether {settles} failed too ({})",
        err.detail(),
        reread.detail()
    ))
}

fn partial_failure(
    completed: &[String],
    progress: Option<Progress>,
    planned: &[PlannedTag],
    index: usize,
    err: &CliError,
) -> CliError {
    let current = &planned[index].name;
    // A tag whose release exists is not outstanding; listing it under
    // `remaining` too would invite a second release.
    let remaining = if matches!(progress, Some(Progress::Released)) {
        &planned[index + 1..]
    } else {
        &planned[index..]
    };
    let mut detail = String::from("release stopped; tags are not deleted.\n");
    if completed.is_empty() && progress.is_none() {
        detail.push_str("completed: none\n");
    } else {
        detail.push_str("completed:\n");
        for name in completed {
            detail.push_str("  ");
            detail.push_str(name);
            detail.push('\n');
        }
        if let Some(progress) = progress {
            let stage = match progress {
                Progress::Tagged => "tagged",
                Progress::Pushed => "pushed",
                Progress::PushUnverified => "tagged; whether the push landed is unverified",
                Progress::TagUnverified => "whether the tag was written is unverified",
                Progress::Released => "released",
            };
            detail.push_str("  ");
            detail.push_str(current);
            detail.push_str(" (");
            detail.push_str(stage);
            detail.push_str(")\n");
        }
    }
    if remaining.is_empty() {
        detail.push_str("remaining: none\n");
    } else {
        detail.push_str("remaining:\n");
        for tag in remaining {
            detail.push_str("  ");
            detail.push_str(&tag.name);
            detail.push('\n');
        }
    }
    detail.push_str(&err.detail());
    err.recast(detail)
}

fn worktree_is_dirty(git: &Git) -> Result<bool, CliError> {
    Ok(!git.text(Op::WorktreeStatus)?.is_empty())
}

/// One gate over the tags oakum is about to push, each read at its own
/// commit: GitHub reads the skip annotation from the commit a push delivers,
/// a pending tag delivers the commit that set its version, and a resumed one
/// delivers the commit it was cut at. Stated over the push set so no path can
/// under-cover it.
fn refuse_skip_ci(git: &Git, planned: &[PlannedTag]) -> Result<(), CliError> {
    let mut read: BTreeSet<&str> = BTreeSet::new();
    for tag in planned {
        if !read.insert(tag.commit.as_str()) {
            continue;
        }
        let commit = git.commit_text(tag.commit.as_str())?;
        if skip_ci(&commit.message, &commit.skip_checks) {
            return Err(CliError::new(format!(
                "tag `{}` would be pushed at a commit whose message carries a \
                 skip-ci annotation, which GitHub honours for tag pushes: the \
                 release workflow would never start. The tag goes on the commit \
                 that set the version, so a later commit does not move it: tag \
                 a clean commit by hand, or start the downstream workflow by hand",
                tag.name
            )));
        }
    }
    Ok(())
}

/// Wrapped at this call site, not in `repository_slug_from`: its other
/// callers are comment writes, where `unverified` would misdescribe a failure
/// that is not a missed look.
fn unverified_look(err: &CliError) -> CliError {
    CliError::unverified(format!(
        "{}, so oakum could not ask GitHub whether these tags are released",
        err.detail()
    ))
}

/// `trailers` is git's own parse of the `skip-checks` trailers (one value per
/// line, ridden along by [`Op::CommitMessage`]), so the trailer question is
/// answered by the parser GitHub's documentation describes rather than an
/// approximation of it. Any position among the trailers refuses — the doc
/// says `skip-checks` should be last — trading a spurious local refusal for
/// never pushing a tag whose workflow was suppressed.
fn skip_ci(message: &str, trailers: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "[skip ci]",
        "[ci skip]",
        "[no ci]",
        "[skip actions]",
        "[actions skip]",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || trailers
            .lines()
            .any(|value| value.trim().eq_ignore_ascii_case("true"))
}

fn local_tag_commit(git: &Git, name: &str) -> Result<Option<Commit>, CliError> {
    git.tag_commit(name)
}

#[cfg(test)]
mod tests {
    use super::{
        decide_preflight, decide_release, refuse_skip_ci, refuse_without_a_look, skip_ci,
        takes_latest, unverified_look, valid_tag_name, worktree_is_dirty, Advertised, BodySource,
        Commit, ExistingTag, Git, GitHubAccess, GitHubTarget, Holder, PendingRelease, PlannedTag,
        Preflight, Progress, ReleaseDecision, ReleaseReadiness, Version,
    };
    use super::{push_outcome, tag_outcome};
    use crate::cli::git::{Reply, Scripted};
    use crate::cli::CliError;

    #[test]
    fn a_release_takes_latest_only_from_an_earlier_version_of_its_own_package() {
        let v = |text: &str| Version::parse(text).expect("version");
        let held = |name: &str, version: &str| Holder::Package {
            name: name.to_owned(),
            version: v(version),
        };
        let cases = [
            (
                "linesmith-plugin",
                "0.1.3",
                held("linesmith", "0.4.1"),
                false,
            ),
            ("linesmith", "0.5.0", held("linesmith", "0.4.1"), true),
            ("linesmith", "0.4.0", held("linesmith", "0.4.1"), false),
            ("linesmith", "0.4.1", held("linesmith", "0.4.1"), false),
            ("linesmith", "0.5.0", held("linesmith-core", "0.9.0"), false),
            ("linesmith-core", "0.5.0", held("linesmith", "0.4.1"), false),
            ("linesmith", "0.5.0", Holder::Other, false),
            ("linesmith", "0.5.0", Holder::None, true),
            ("linesmith", "0.5.0-rc.1", held("linesmith", "0.4.1"), false),
            ("linesmith", "0.5.0-rc.1", Holder::None, false),
            ("linesmith", "0.5.0", held("linesmith", "0.5.0-rc.1"), true),
            ("linesmith", "1.2.3+z", held("linesmith", "1.2.3+a"), false),
        ];
        for (package, version, holder, expected) in cases {
            assert_eq!(
                takes_latest(package, &v(version), &holder),
                expected,
                "{package} {version} with Latest at {holder:?}"
            );
        }
    }

    /// The reporter wrote `{{ name }}`, and found `package` only by reading
    /// this file. The schema now names both variables; this is what keeps that
    /// list honest when the context changes.
    #[test]
    fn the_schema_names_every_variable_tag_format_renders_with() {
        crate::cli::schema_names_every_variable(
            "tag-format",
            super::tag_context("demo", &Version::new(1, 2, 3)),
        );
    }

    /// The helper is pure, so every combination is a unit assertion rather
    /// than a spawned binary.
    #[test]
    fn decide_nothing_when_no_work_and_no_credentials() {
        let decision = decide_release(readiness(
            false,
            GitHubAccess::NoRemote { token: false },
            Preflight::NothingPlanned,
        ));
        assert!(matches!(decision, ReleaseDecision::NothingToRelease));
    }

    #[test]
    fn decide_refuses_a_dirty_worktree_before_proceeding() {
        let decision = decide_release(ReleaseReadiness {
            worktree_dirty: true,
            ..readiness(true, ready(), asked(vec![planned_tag("v0.1.1")]))
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("dirty"), "{err}");
    }

    #[test]
    fn decide_refuses_on_preflight_error() {
        let decision = decide_release(readiness(
            true,
            ready(),
            Preflight::Refused(CliError::new(
                "`oakum release` needs GITHUB_TOKEN or GH_TOKEN",
            )),
        ));
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("GITHUB_TOKEN"), "{err}");
    }

    #[test]
    fn decide_proceeds_when_readiness_is_complete() {
        let decision = decide_release(readiness(true, ready(), asked(vec![planned_tag("v0.1.1")])));
        assert!(matches!(decision, ReleaseDecision::Proceed(_)));
    }

    #[test]
    fn decide_nothing_when_credentials_exist_but_there_is_no_work() {
        let decision = decide_release(readiness(false, ready(), Preflight::NothingPlanned));
        assert!(matches!(decision, ReleaseDecision::NothingToRelease));
    }

    #[test]
    fn decide_nothing_when_github_has_answered_every_tag() {
        let decision = decide_release(readiness(false, ready(), asked(Vec::new())));
        assert!(matches!(decision, ReleaseDecision::NothingToRelease));
    }

    /// A resume-only run with both present: an unreadable slug is the
    /// preflight's refusal, not a quiet "nothing to release".
    #[test]
    fn decide_reports_an_unresolved_slug_rather_than_nothing() {
        let err = CliError::new("not a GitHub URL");
        let decision = decide_release(ReleaseReadiness {
            resumable: vec![resumable_tag("v0.1.0")],
            ..readiness(
                false,
                GitHubAccess::Unresolved(err.clone()),
                Preflight::Refused(unverified_look(&err)),
            )
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.is_unverified(), "{err}");
    }

    #[test]
    fn decide_refuses_a_resume_look_without_credentials() {
        let decision = decide_release(ReleaseReadiness {
            resumable: vec![resumable_tag("v0.1.0")],
            ..readiness(false, GitHubAccess::NoToken, Preflight::NothingPlanned)
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("GITHUB_TOKEN"), "{err}");
        assert!(err.to_string().starts_with("unverified:"), "{err}");
    }

    #[test]
    fn decide_refuses_on_skip_ci_error() {
        let decision = decide_release(ReleaseReadiness {
            skip_ci_error: Some(CliError::new(
                "refusing to release: HEAD commit is marked skip-ci",
            )),
            ..readiness(true, ready(), asked(vec![planned_tag("v0.1.1")]))
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("skip-ci"), "{err}");
    }

    /// A missing remote cannot be confirmed (exit 2); a missing token is a
    /// plain refusal (exit 1).
    #[test]
    fn the_preflight_names_what_github_access_lacks() {
        for token in [false, true] {
            let no_remote =
                decide_preflight(&GitHubAccess::NoRemote { token }).expect_err("no remote");
            assert_eq!(no_remote.exit_code(), 2, "{no_remote}");
            assert!(no_remote.to_string().contains("no remotes"), "{no_remote}");
        }
        let no_token = decide_preflight(&GitHubAccess::NoToken).expect_err("no token");
        assert_eq!(no_token.exit_code(), 1, "{no_token}");
        assert!(no_token.to_string().contains("GITHUB_TOKEN"), "{no_token}");
        let unresolved = decide_preflight(&GitHubAccess::Unresolved(CliError::new("not GitHub")))
            .expect_err("unresolved");
        assert_eq!(unresolved.exit_code(), 2, "{unresolved}");
        assert_eq!(decide_preflight(&ready()).expect("ready").owner, "oakoss");
    }

    fn target() -> GitHubTarget {
        GitHubTarget {
            remote: String::from("origin"),
            token: String::from("token"),
            owner: String::from("oakoss"),
            repo_name: String::from("oakum"),
        }
    }

    fn ready() -> GitHubAccess {
        GitHubAccess::Ready(target())
    }

    fn asked(planned: Vec<PlannedTag>) -> Preflight {
        Preflight::Asked {
            planned,
            advertised: Advertised::default(),
            github: target(),
        }
    }

    fn readiness(
        had_planned_before_resume: bool,
        access: GitHubAccess,
        preflight: Preflight,
    ) -> ReleaseReadiness {
        ReleaseReadiness {
            had_planned_before_resume,
            resumable: Vec::new(),
            access,
            preflight,
            skip_ci_error: None,
            worktree_dirty: false,
        }
    }

    fn planned_tag(name: &str) -> PlannedTag {
        let commit = Git::answering([("rev-parse HEAD", Reply::said("cafe"))])
            .head()
            .expect("minted");
        PlannedTag {
            package: String::from("demo"),
            version: Version::new(0, 1, 1),
            changelog: String::from("CHANGELOG.md"),
            shared_changelog: false,
            name: String::from(name),
            commit,
            resumed: false,
        }
    }

    /// The refusal keeps its unverified class through the read-failure
    /// wrapper, and the path prints on one line.
    #[test]
    fn a_changelog_path_holding_a_newline_is_unverified() {
        let tag = PlannedTag {
            changelog: String::from("pkg\nage/CHANGELOG.md"),
            ..planned_tag("v0.1.1")
        };
        let Err(err) = super::release_body(&Git::answering([]), &tag) else {
            panic!("a newline in the path was read");
        };
        assert_eq!(err.exit_code(), 2, "{err}");
        assert!(err.to_string().contains("pkg\\nage/CHANGELOG.md"), "{err}");
        assert!(!err.to_string().contains("pkg\nage"), "{err}");
    }

    fn resumable_tag(name: &str) -> ExistingTag {
        let commit = Git::answering([("rev-parse HEAD", Reply::said("cafe"))])
            .head()
            .expect("minted");
        ExistingTag {
            item: PendingRelease::new(
                oakum::plan::PackageId::new(oakum::plan::Ecosystem::Cargo, "demo"),
                Version::new(0, 1, 0),
            ),
            name: String::from(name),
            commit,
        }
    }

    #[test]
    fn a_look_needs_both_a_remote_and_a_token() {
        refuse_without_a_look(&ready()).expect("both present");
        refuse_without_a_look(&GitHubAccess::Unresolved(CliError::new("not GitHub")))
            .expect("both present, so the preflight names the slug failure");

        let no_remote = refuse_without_a_look(&GitHubAccess::NoRemote { token: true })
            .expect_err("a missing remote refuses");
        assert!(no_remote
            .to_string()
            .contains("this repository has no remote"));
        assert!(!no_remote.to_string().contains("GITHUB_TOKEN"));

        let no_token =
            refuse_without_a_look(&GitHubAccess::NoToken).expect_err("a missing token refuses");
        assert!(no_token.to_string().contains("GITHUB_TOKEN and GH_TOKEN"));
        assert!(!no_token.to_string().contains("has no remote"));

        let neither = refuse_without_a_look(&GitHubAccess::NoRemote { token: false })
            .expect_err("neither present refuses")
            .to_string();
        assert!(
            neither.contains("this repository has no remote"),
            "{neither}"
        );
        assert!(neither.contains("GITHUB_TOKEN and GH_TOKEN"), "{neither}");
    }

    /// The class is the contract, not the prefix: a slug failure on the
    /// release path is a look that could not be made.
    #[test]
    fn a_slug_failure_on_the_release_path_is_unverified() {
        let err = unverified_look(&CliError::new("git `origin` `x` is not a github.com URL"));
        assert!(err.is_unverified(), "{err:?}");
        assert!(err.to_string().contains("is not a github.com URL"), "{err}");
    }

    const STATUS: &str = "status --porcelain";

    /// Driven at the caller rather than at the git layer. What the `Answer`
    /// split defends here is a `status` that reported a problem instead of
    /// answering; a `status` that returns cleanly having read nothing is
    /// indistinguishable from a clean worktree, and no rule recovers that.
    #[test]
    fn a_dirty_worktree_stops_the_release_and_a_clean_one_does_not() {
        let clean = Git::answering([(STATUS, Reply::said(""))]);
        assert!(!worktree_is_dirty(&clean).expect("a clean worktree"));
        assert_eq!(clean.asked().len(), 1, "{:?}", clean.asked());

        let dirty = Git::answering([(STATUS, Reply::said(" M crates/oakum/src/lib.rs"))]);
        assert!(worktree_is_dirty(&dirty).expect("a dirty worktree"));
    }

    #[test]
    fn a_skip_ci_commit_in_the_push_set_stops_the_release() {
        // Minted through a scripted read: `Commit` has no constructor outside
        // `cli::git`, so a test cannot invent one either.
        let cafe = Git::answering([("rev-parse HEAD", Reply::said("cafe"))])
            .head()
            .expect("minted");
        let planned = |commit: &Commit| PlannedTag {
            package: String::from("demo"),
            version: Version::new(0, 1, 1),
            changelog: String::from("CHANGELOG.md"),
            shared_changelog: false,
            name: String::from("v0.1.1"),
            commit: commit.clone(),
            resumed: false,
        };
        let marked = Git::answering([("log -1", Reply::said("fix: typo [skip ci]\u{0}\n"))]);
        let err = refuse_skip_ci(&marked, &[planned(&cafe)]).expect_err("a skip-ci commit stops");
        assert!(err.to_string().contains("skip-ci"), "{err}");
        assert!(err.to_string().contains("v0.1.1"), "{err}");

        let plain = Git::answering([("log -1", Reply::said("fix: typo\u{0}\n"))]);
        refuse_skip_ci(&plain, &[planned(&cafe)]).expect("an unmarked commit releases");
        assert_eq!(plain.asked().len(), 1, "{:?}", plain.asked());

        // One read per distinct commit, however many tags share it.
        let shared = Git::answering([("log -1", Reply::said("fix: typo\u{0}\n"))]);
        refuse_skip_ci(&shared, &[planned(&cafe), planned(&cafe)]).expect("shared commit");
        assert_eq!(shared.asked().len(), 1, "{:?}", shared.asked());

        let trailer = Git::answering([("log -1", Reply::said("chore: release\u{0}true\n"))]);
        let err =
            refuse_skip_ci(&trailer, &[planned(&cafe)]).expect_err("a skip-checks trailer stops");
        assert!(err.to_string().contains("skip-ci"), "{err}");
    }

    /// The five bracketed strings come from github/docs skip-workflow-runs.md,
    /// which is the whole list GitHub honours — a tag push is an `on: push`
    /// event, so it is covered too. The trailer half arrives pre-parsed by
    /// git (one value per line), so only the value decision lives here.
    #[test]
    fn skip_ci_matches_github_annotations_and_trailers() {
        assert!(skip_ci("fix: typo [skip ci]", ""));
        assert!(skip_ci("[CI SKIP] docs", ""));
        assert!(skip_ci("chore: [no ci]", ""));
        assert!(skip_ci("[skip actions]\nmore", ""));
        assert!(skip_ci("[actions skip]", ""));
        assert!(!skip_ci("fix: do not skip", ""));
        assert!(!skip_ci("skip ci without brackets", ""));
        // A prose mention never reaches the trailer half: git parses the
        // trailers, and prose is not one.
        assert!(!skip_ci("docs: explain the skip-checks:true trailer", ""));

        assert!(skip_ci("fix: typo", "true"));
        // git returns the value verbatim and GitHub's key match is
        // case-insensitive, so `Skip-Checks: True` arrives as `True`.
        assert!(skip_ci("fix: typo", "True"));
        assert!(skip_ci("fix: typo", " true "));
        assert!(skip_ci("fix: typo", "false\ntrue"));
        assert!(!skip_ci("fix: typo", "false"));
        assert!(!skip_ci("fix: typo", "truely"));
        assert!(!skip_ci("fix: typo", ""));
    }

    #[test]
    fn valid_tag_name_rejects_git_ref_hazards() {
        // A real child on purpose: the rules being asserted are git's, and a
        // scripted answer would only restate what this test was written to
        // check. `check-ref-format` reads no repository, so the path it runs in
        // makes no difference to the verdict.
        let git = Git::at(".");
        assert!(valid_tag_name(&git, "v0.1.1").is_ok());
        assert!(valid_tag_name(&git, "pkg/v0.1.1").is_ok());
        assert!(valid_tag_name(&git, "foo..bar").is_err());
        assert!(valid_tag_name(&git, "foo.lock").is_err());
        assert!(valid_tag_name(&git, "foo@{bar}").is_err());
        assert!(valid_tag_name(&git, "-v0.1.1").is_err());
        assert!(valid_tag_name(&git, "--delete").is_err());
    }

    /// Every name the allowlist accepts, git accepts too: the short names over
    /// the allowlist's punctuation, exhaustively, and the hazards by hand.
    #[test]
    fn the_tag_name_allowlist_never_accepts_what_git_refuses() {
        use crate::cli::git::Op;

        let git = Git::at(".");
        let alphabet = ["a", "A", "0", "_", ".", "/", "@", "-", "+"];
        let usual = [
            "v0.1.1",
            "pkg/v0.1.1",
            "@scope/pkg@1.0.0",
            "pkg-v1.0.0+build.1",
        ];
        let mut names: Vec<String> = Vec::new();
        for first in alphabet {
            names.push(first.to_owned());
            for second in alphabet {
                names.push(format!("{first}{second}"));
                for third in alphabet {
                    names.push(format!("{first}{second}{third}"));
                }
            }
        }
        names.extend(
            [
                "v1.0.lock",
                "a.lock/b",
                "a/b.lock",
                "x/.y",
                "v1.0.",
                "a/",
                "/a",
                "a//b",
                "a..b",
                ".a",
                "@",
                "a@{b",
                "pkg@1.0.0",
                "@scope/pkg@1.2.3",
            ]
            .map(String::from),
        );
        names.extend(usual.map(String::from));
        let mut accepted = 0;
        for name in &names {
            if super::plainly_valid_tag_name(name) {
                accepted += 1;
                assert!(
                    git.predicate(Op::ValidRefName { reference: name })
                        .expect("check-ref-format"),
                    "the allowlist accepted {name:?}, which git refuses"
                );
            }
        }
        assert!(accepted > 0, "the allowlist accepted none of {names:?}");
        for name in usual {
            assert!(super::plainly_valid_tag_name(name), "{name}");
        }
    }

    #[test]
    fn body_source_renders_each_fallback_once() {
        let tag = planned_tag("v0.1.1");
        assert_eq!(
            BodySource::Section(String::from("- a fix")).resolve(&tag),
            (String::from("- a fix\n"), None)
        );
        for (source, expected) in [
            (
                BodySource::NoChangelog,
                "release body: no CHANGELOG.md at cafe; using the title",
            ),
            (
                BodySource::Unfollowed {
                    why: "a symlink loop",
                },
                "release body: CHANGELOG.md at cafe is a symlink loop; using the title",
            ),
            (
                BodySource::EmptySection,
                "release body: CHANGELOG.md has an empty 0.1.1 section; using the title",
            ),
            (
                BodySource::NoHeading,
                "release body: CHANGELOG.md has no `## 0.1.1` heading; using the title (run `oakum version` before releasing, or add the section)",
            ),
        ] {
            let (body, notice) = source.resolve(&tag);
            assert_eq!(body, "demo 0.1.1\n");
            assert_eq!(notice.as_deref(), Some(expected));
        }
    }

    /// The error a child that never answered produces, as the spawn classifies
    /// a deadline-killed one; stands in for the failed `git tag` or `git push`.
    fn unanswered() -> CliError {
        let Err(err) = Git::scripted([("rev-parse HEAD", Scripted::NoAnswer)]).head() else {
            panic!("a child that never answered produced a commit");
        };
        assert!(err.is_unanswered(), "{err}");
        assert_eq!(err.exit_code(), 2, "{err}");
        err
    }

    const TAG_REREAD: &str = "rev-parse --verify refs/tags";
    const PUSH_REREAD: &str = "ls-remote --tags";

    /// The local re-read after a `git tag` that never answered names the stage
    /// it finds; only a re-read that also goes unanswered stays unverified.
    #[test]
    fn a_tag_that_never_answered_is_settled_by_the_local_re_read() {
        let tag = planned_tag("v0.1.1");
        let err = unanswered();
        let cases = [
            (Scripted::Says(Reply::said("cafe")), "landed", 1),
            (Scripted::Says(Reply::absent()), "absent", 1),
            (Scripted::Says(Reply::said("beef")), "elsewhere", 1),
            (Scripted::NoAnswer, "unread", 2),
        ];
        for (reread, case, exit) in cases {
            let git = Git::scripted([(TAG_REREAD, reread)]);
            let (progress, out) = tag_outcome(&git, &tag, &err);
            match case {
                "landed" => assert!(matches!(progress, Some(Progress::Tagged)), "{case}"),
                "unread" => {
                    assert!(matches!(progress, Some(Progress::TagUnverified)), "{case}");
                    assert!(out.to_string().contains("the tag was written"), "{out}");
                }
                _ => assert!(progress.is_none(), "{case}"),
            }
            assert_eq!(out.exit_code(), exit, "{case}: {out}");
        }
    }

    /// The remote re-read after a `git push` that never answered: the tag at
    /// its commit is pushed, anything else keeps the stage from before the push.
    #[test]
    fn a_push_that_never_answered_is_settled_by_the_remote_re_read() {
        let tag = planned_tag("v0.1.1");
        let err = unanswered();
        let cases = [
            (
                Scripted::Says(Reply::said("cafe\trefs/tags/v0.1.1^{}\n")),
                "landed",
                1,
            ),
            (Scripted::Says(Reply::said("")), "absent", 1),
            (
                Scripted::Says(Reply::said("beef\trefs/tags/v0.1.1^{}\n")),
                "elsewhere",
                1,
            ),
            (Scripted::NoAnswer, "unread", 2),
        ];
        for (reread, case, exit) in cases {
            let git = Git::scripted([
                ("remote", Scripted::Says(Reply::said("origin"))),
                (
                    "remote -v",
                    Scripted::Says(Reply::said(
                        "origin\thttps://github.com/oakoss/demo.git (fetch)\n\
                         origin\thttps://github.com/oakoss/demo.git (push)\n",
                    )),
                ),
                (PUSH_REREAD, reread),
            ]);
            let (progress, out) = push_outcome(&git, "origin", &tag, Some(Progress::Tagged), &err);
            match case {
                "landed" => assert!(matches!(progress, Some(Progress::Pushed)), "{case}"),
                "unread" => {
                    assert!(matches!(progress, Some(Progress::PushUnverified)), "{case}");
                    assert!(out.to_string().contains("the tag landed"), "{out}");
                }
                _ => assert!(matches!(progress, Some(Progress::Tagged)), "{case}"),
            }
            assert_eq!(out.exit_code(), exit, "{case}: {out}");
        }
    }
}
