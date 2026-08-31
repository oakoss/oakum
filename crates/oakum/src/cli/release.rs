//! `oakum release` writes tags and GitHub releases (ADR-0023). Same local
//! preconditions as `check` (ADR-0020); no rollback (ADR-0011).

use std::collections::{BTreeMap, BTreeSet};

use clap::Args;
use semver::Version;
use serde_json::json;

use super::add;
use super::ci;
use super::config::{enforce_tool_version, load_config};
use super::git::{Commit, Git, Op};
use super::github::{self, Look};
use super::handoff;
use super::preconditions::{self, PendingRelease, TagEvaluation};
use super::repository;
use super::tags::{self, Advertised};
use super::template::load_template_body;
use super::CliError;

const DEFAULT_SINGLE: &str = "v{{ version }}";
const DEFAULT_MULTI: &str = "{{ package }}/v{{ version }}";

#[derive(Debug, Args)]
pub(super) struct ReleaseArgs {
    /// Git ref to scan from (exclusive). Same default as `check` / `status`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

#[derive(Debug)]
struct PlannedTag {
    package: String,
    version: Version,
    /// Either passed `valid_tag_name` (`plan_tags`) or names a tag git
    /// already holds (`resume_candidates`); a new construction site owes one
    /// of the two.
    name: String,
    /// Where the tag points, or will. A resumed tag keeps the commit it was
    /// cut at; a new one names HEAD.
    commit: Commit,
    /// A resumed tag already exists, so a release found for it is a finished
    /// state; for a new tag the same finding is a collision.
    resumed: bool,
}

#[derive(Clone, Copy)]
enum Progress {
    Tagged,
    Pushed,
    /// The push failed and the re-read that would settle whether the ref
    /// landed failed too, so no stage can honestly be named.
    PushUnverified,
    Released,
}

#[derive(Clone, Copy)]
struct HaveRemote(bool);

#[derive(Clone, Copy)]
struct HaveToken(bool);

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
    owner: String,
    repo_name: String,
    remote: String,
}

/// Inputs gathered before policy runs. Every look belongs here; `decide_release`
/// has no I/O.
struct ReleaseReadiness {
    had_planned_before_resume: bool,
    resumable: Vec<ExistingTag>,
    have_remote: HaveRemote,
    have_token: HaveToken,
    remote: Option<String>,
    token: Option<String>,
    planned: Vec<PlannedTag>,
    advertised: Advertised,
    preflight_error: Option<CliError>,
    skip_ci_error: Option<CliError>,
    worktree_dirty: bool,
    owner: Option<String>,
    repo_name: Option<String>,
}

/// Release policy as a pure function over gathered readiness.
fn decide_release(readiness: ReleaseReadiness) -> ReleaseDecision {
    if !readiness.had_planned_before_resume {
        if !readiness.resumable.is_empty() {
            if let Err(err) = refuse_without_a_look(readiness.have_remote, readiness.have_token) {
                return ReleaseDecision::Refused(err);
            }
        }
        if readiness.remote.is_none() || readiness.token.is_none() {
            return ReleaseDecision::NothingToRelease;
        }
    }
    if let Some(err) = readiness.preflight_error {
        return ReleaseDecision::Refused(err);
    }
    if readiness.planned.is_empty() {
        return ReleaseDecision::NothingToRelease;
    }
    if let Some(err) = readiness.skip_ci_error {
        return ReleaseDecision::Refused(err);
    }
    if readiness.worktree_dirty {
        return ReleaseDecision::Refused(CliError::new(
            "working tree is dirty; commit or stash before releasing",
        ));
    }
    let (Some(owner), Some(repo_name), Some(remote)) =
        (readiness.owner, readiness.repo_name, readiness.remote)
    else {
        return ReleaseDecision::Refused(CliError::new(
            "internal error: release plan missing GitHub coordinates",
        ));
    };
    ReleaseDecision::Proceed(ReleasePlan {
        planned: readiness.planned,
        advertised: readiness.advertised,
        owner,
        repo_name,
        remote,
    })
}

fn apply_decision(
    decision: ReleaseDecision,
    git: &Git,
    token: Option<String>,
) -> Result<(), CliError> {
    match decision {
        ReleaseDecision::NothingToRelease => {
            println!("nothing to release");
            Ok(())
        }
        ReleaseDecision::Refused(err) => Err(err),
        ReleaseDecision::Proceed(plan) => {
            let token = token
                .ok_or_else(|| CliError::new("`oakum release` needs GITHUB_TOKEN or GH_TOKEN"))?;
            let client = github::Client::new(token)?;
            act(
                git,
                &client,
                &plan.owner,
                &plan.repo_name,
                &plan.remote,
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
fn refuse_without_a_look(remote: HaveRemote, token: HaveToken) -> Result<(), CliError> {
    let absent: Vec<&str> = [
        (!remote.0).then_some("this repository has no remote"),
        (!token.0).then_some("GITHUB_TOKEN and GH_TOKEN are both unset"),
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
    repo: &repository::Repository,
    git: &Git,
    evaluation: &TagEvaluation,
) -> Result<Vec<ExistingTag>, CliError> {
    let template = tag_template(repo)?;
    let mut found = Vec::new();
    for item in evaluation.current() {
        let name = render_tag(&template, &item)?;
        if let Some(commit) = local_tag_commit(git, &name)? {
            found.push(ExistingTag { item, name, commit });
        }
    }
    Ok(found)
}

pub(super) fn run(args: &ReleaseArgs) -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    let config = load_config(&repo).map_err(CliError::from_boxed)?;
    enforce_tool_version(&config).map_err(CliError::from_boxed)?;
    let git = Git::at(repo.path());
    let evaluation = preconditions::evaluate(&git, &repo, args.from.as_deref(), false, false, 3)?;
    let pending = evaluation.pending();
    let planned_before_resume = if pending.is_empty() {
        Vec::new()
    } else {
        plan_tags(&repo, &git, &pending)?
    };
    let resumable = existing_tags(&repo, &git, &evaluation)?;
    let remote = tags::first_remote(&git)?;
    refuse_unread_tags(&repo, &git, &pending, remote.as_deref())?;
    let token = github::token();
    let have_remote = HaveRemote(remote.is_some());
    let have_token = HaveToken(token.is_some());
    let slug = match (remote.as_ref(), token.as_ref()) {
        (Some(remote), Some(_)) => Some(ci::repository_slug_from(&git, remote)),
        _ => None,
    };
    let (owner, repo_name) = match &slug {
        Some(Ok((owner, repo_name))) => (Some(owner.clone()), Some(repo_name.clone())),
        _ => (None, None),
    };
    let had_planned_before_resume = !planned_before_resume.is_empty();
    let worktree_dirty = worktree_is_dirty(&git)?;
    if had_planned_before_resume && worktree_dirty {
        return Err(CliError::new(
            "working tree is dirty; commit or stash before releasing",
        ));
    }
    let mut planned = planned_before_resume;
    planned.extend(resume_candidates(&repo, &resumable, &planned)?);
    let (planned, advertised, preflight_error) = if planned.is_empty() {
        (planned, Advertised::default(), None)
    } else if remote.is_none() {
        (
            planned,
            Advertised::default(),
            Some(CliError::unverified(
                "unverified: this repository has no remotes to push tags to",
            )),
        )
    } else if token.is_none() {
        (
            planned,
            Advertised::default(),
            Some(CliError::new(
                "`oakum release` needs GITHUB_TOKEN or GH_TOKEN",
            )),
        )
    } else if let Some(Err(err)) = slug {
        (planned, Advertised::default(), Some(unverified_look(&err)))
    } else {
        let remote = remote.clone().expect("checked above");
        let token = token.clone().expect("checked above");
        let owner = owner.clone().expect("slug succeeded");
        let repo_name = repo_name.clone().expect("slug succeeded");
        let client = github::Client::new(token)?;
        match preflight(&git, &client, &owner, &repo_name, &remote, planned) {
            Ok((planned, advertised)) => (planned, advertised, None),
            Err(err) => (Vec::new(), Advertised::default(), Some(err)),
        }
    };
    let skip_ci_error = refuse_skip_ci(&git, &planned).err();
    let readiness = ReleaseReadiness {
        had_planned_before_resume,
        resumable,
        have_remote,
        have_token,
        remote,
        token: token.clone(),
        planned,
        advertised,
        preflight_error,
        skip_ci_error,
        worktree_dirty,
        owner,
        repo_name,
    };
    apply_decision(decide_release(readiness), &git, token)
}

fn plan_tags(
    repo: &repository::Repository,
    git: &Git,
    items: &[PendingRelease],
) -> Result<Vec<PlannedTag>, CliError> {
    let template = tag_template(repo)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let head = git.head()?;
    let mut rendered = Vec::new();
    let mut names = BTreeSet::new();
    for item in items {
        let name = render_tag(&template, item)?;
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
        readable_for_package(&workspace, item, &name)?;
        planned.push(PlannedTag {
            package: item.id().name.clone(),
            version: item.version().clone(),
            name,
            commit: head.clone(),
            resumed: false,
        });
    }
    Ok(planned)
}

/// Nominates only: whether a candidate is still owed a release is GitHub's
/// answer, and `preflight` is the one place that asks it. A candidate's name
/// needs no `valid_tag_name` pass — it names a tag git already holds.
fn resume_candidates(
    repo: &repository::Repository,
    resumable: &[ExistingTag],
    already: &[PlannedTag],
) -> Result<Vec<PlannedTag>, CliError> {
    let planned: BTreeSet<&str> = already.iter().map(|tag| tag.name.as_str()).collect();
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let mut extra = Vec::new();
    for candidate in resumable {
        if planned.contains(candidate.name.as_str()) {
            continue;
        }
        readable_for_package(&workspace, &candidate.item, &candidate.name)?;
        extra.push(PlannedTag {
            package: candidate.item.id().name.clone(),
            version: candidate.item.version().clone(),
            name: candidate.name.clone(),
            commit: candidate.commit.clone(),
            resumed: true,
        });
    }
    Ok(extra)
}

fn tag_template(repo: &repository::Repository) -> Result<String, CliError> {
    let config = load_config(repo).map_err(CliError::from_boxed)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    config.validate_workspace_selection(&workspace)?;
    // Bare vs package-prefixed default follows the full workspace size: tag
    // attribution still resolves against every discovered package (ADR-0030).
    let default = if workspace.packages().count() <= 1 {
        DEFAULT_SINGLE
    } else {
        DEFAULT_MULTI
    };
    match config.tag_format() {
        Some(source) => {
            load_template_body(repo.dir(), repo.path(), source).map_err(CliError::from_boxed)
        }
        None => Ok(default.to_owned()),
    }
}

fn render_tag(template: &str, item: &PendingRelease) -> Result<String, CliError> {
    render_tag_for(template, &item.id().name, item.version())
}

fn render_tag_for(template: &str, package: &str, version: &Version) -> Result<String, CliError> {
    let rendered = oakum::template::render(
        "tag-format",
        template,
        json!({
            "package": package,
            "version": version.to_string(),
        }),
    )
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

fn readable_for_package(
    workspace: &oakum::plan::Workspace,
    item: &PendingRelease,
    name: &str,
) -> Result<(), CliError> {
    match oakum::tags::resolve_commit_tags(&[name], workspace) {
        Ok(map) => match map.get(item.id()) {
            Some(version) if version == item.version() => Ok(()),
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
/// A package whose rendered tag is local and reachable is settled, and its
/// `continue` deliberately skips every scan for that package: its release
/// state is read on the resume path, and no other tag — stale, foreign, or
/// unattributable — changes that verdict. Repository-wide tag hygiene is
/// `check`'s question, not this gate's.
fn refuse_unread_tags(
    repo: &repository::Repository,
    git: &Git,
    pending: &[PendingRelease],
    remote: Option<&str>,
) -> Result<(), CliError> {
    let config = load_config(repo).map_err(CliError::from_boxed)?;
    let template = tag_template(repo)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    config.validate_workspace_selection(&workspace)?;
    let listed = tags::all_tag_objects(git)?;
    let reachable = tags::reachable_tag_names(git)?;
    let local_names: BTreeSet<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    let pending_ids: BTreeSet<_> = pending.iter().map(PendingRelease::id).collect();
    let local_resolved =
        resolve_tag_names(listed.iter().map(|(name, _)| name.as_str()), &workspace);
    let mut advertised = None;
    // tag_managed mirrors tags::{drift,untagged_ahead}: unmanaged packages owe no tag.
    for package in workspace
        .packages()
        .filter(|package| config.tag_managed(package))
    {
        if pending_ids.contains(package.id()) {
            continue;
        }
        refuse_unread_for_package(
            &mut UnreadTagScan {
                git,
                template: &template,
                listed: &listed,
                reachable: &reachable,
                local_names: &local_names,
                local_resolved: &local_resolved,
                workspace: &workspace,
                remote,
                advertised: &mut advertised,
            },
            package,
        )?;
    }
    Ok(())
}

struct UnreadTagScan<'a> {
    git: &'a Git,
    template: &'a str,
    listed: &'a [(String, String)],
    reachable: &'a BTreeSet<String>,
    local_names: &'a BTreeSet<&'a str>,
    local_resolved: &'a ResolvedTags,
    workspace: &'a oakum::plan::Workspace,
    remote: Option<&'a str>,
    advertised: &'a mut Option<(tags::Advertised, ResolvedTags)>,
}

fn refuse_unread_for_package(
    ctx: &mut UnreadTagScan<'_>,
    package: &oakum::plan::Package,
) -> Result<(), CliError> {
    let package_name = &package.id().name;
    let version = package.version();
    let rendered = render_tag_for(ctx.template, package_name, version)?;
    if let Some((_, object)) = ctx.listed.iter().find(|(name, _)| *name == rendered) {
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
    if let Some(err) = scan_resolved_tags(
        ctx.local_resolved,
        package.id(),
        version,
        |name| ctx.reachable.contains(name),
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
        let remote_resolved =
            resolve_tag_names(commits.tag_names().map(str::to_owned), ctx.workspace);
        *ctx.advertised = Some((commits, remote_resolved));
    }
    let (advertised_map, remote_resolved) = ctx.advertised.as_ref().expect("consulted above");
    // Settled across the whole listing before the resolution scan, as the
    // local side does: inside the loop, map order would pick which verdict
    // fires, and the earlier-format remedy does not clear this refusal.
    if advertised_map.contains_tag(&rendered) {
        return Err(CliError::unverified(format!(
            "unverified: tag `{rendered}` for {package_name} {version} exists on \
             remote {remote:?} but not locally, so the release scan never read it; \
             run `git fetch {remote} tag {rendered}` and rerun"
        )));
    }
    if let Some(err) = scan_resolved_tags(
        remote_resolved,
        package.id(),
        version,
        |name| ctx.local_names.contains(name),
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
) -> ResolvedTags {
    names
        .into_iter()
        .map(|name| {
            let name = name.into();
            let resolved = oakum::tags::resolve_commit_tags(&[&name], workspace).map_err(|_| ());
            (name, resolved)
        })
        .collect()
}

/// Shared unread-tag ladder: maps-to-this-version refuses, other resolutions
/// pass, and an unattributable name refuses unless `excuse` says otherwise.
fn scan_resolved_tags(
    tags: &ResolvedTags,
    package_id: &oakum::plan::PackageId,
    version: &Version,
    excuse: impl Fn(&str) -> bool,
    on_mismatch: impl Fn(&str) -> CliError,
    on_unattributable: impl Fn(&str) -> CliError,
) -> Option<CliError> {
    for (name, resolved) in tags {
        match resolved {
            Ok(map) if map.get(package_id) == Some(version) => {
                return Some(on_mismatch(name));
            }
            Ok(_) => {}
            Err(()) if excuse(name) => {}
            Err(()) => return Some(on_unattributable(name)),
        }
    }
    None
}

fn valid_tag_name(git: &Git, name: &str) -> Result<(), CliError> {
    if name.starts_with('-') {
        return Err(CliError::new(format!(
            "tag-format rendered an invalid git ref: {name:?}"
        )));
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
/// the git remote; the advertised snapshot is then read once and handed to
/// `act`, whose per-tag decisions are made against it; `push_outcome` still
/// re-reads on its failure path to settle what a died push did.
fn preflight(
    git: &Git,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: Vec<PlannedTag>,
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
    let advertised = tags::remote_tag_commits(git, remote)?;
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
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: &[PlannedTag],
    advertised: &Advertised,
) -> Result<(), CliError> {
    let mut completed = Vec::new();
    // Opened before anything is written, so a failed look cannot strand a
    // pushed tag outside the partial-failure report.
    let mut handoff = handoff::Handoff::open(
        client,
        owner,
        name,
        git,
        planned.iter().map(|tag| &tag.commit),
    )?;
    for (index, tag) in planned.iter().enumerate() {
        let released = |completed: &[String], err: &CliError| {
            partial_failure(
                completed,
                Some(Progress::Released),
                &tag.name,
                &planned[index + 1..],
                err,
            )
        };
        match release_one(git, client, owner, name, remote, tag, advertised) {
            Ok(did_push) => {
                match handoff.confirm_push(&tag.name, &tag.commit, did_push) {
                    Ok(None) => {}
                    Ok(Some(run)) => {
                        if !run.html_url.is_empty() {
                            println!("{}", run.html_url);
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
                completed.push(tag.name.clone());
                println!("{} {} {}", tag.package, tag.version, tag.name);
            }
            Err((progress, err)) => {
                return Err(partial_failure(
                    &completed,
                    progress,
                    &tag.name,
                    &planned[index..],
                    &err,
                ));
            }
        }
    }
    Ok(())
}

fn release_one(
    git: &Git,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    tag: &PlannedTag,
    advertised: &Advertised,
) -> Result<bool, (Option<Progress>, CliError)> {
    let mut progress = None;
    let remote_has_it = advertised.points_at(&tag.name, &tag.commit);
    if local_tag_commit(git, &tag.name)
        .map_err(|err| (progress, err))?
        .is_none()
        && !remote_has_it
    {
        // Only a planned tag reaches this: a resumed one already exists, which
        // is what `existing_tags` selected it for. So the commit here is HEAD.
        git.run(Op::AnnotatedTag {
            name: &tag.name,
            commit: &tag.commit,
        })
        .map_err(|err| (progress, err))?;
        progress = Some(Progress::Tagged);
    }
    let did_push = !remote_has_it;
    if did_push {
        git.run(Op::PushTag {
            remote,
            tag: &tag.name,
        })
        .map_err(|err| push_outcome(git, remote, tag, progress, err))?;
    }
    progress = Some(Progress::Pushed);
    let title = format!("{} {}", tag.package, tag.version);
    let body = format!("{title}\n");
    let created = client
        .create_release(owner, name, &tag.name, &title, &body)
        .map_err(|err| (progress, CliError::from(err)))?;
    println!("{}", created.html_url);
    Ok(did_push)
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
    err: CliError,
) -> (Option<Progress>, CliError) {
    match tags::remote_tag_commits(git, remote) {
        Ok(advertised) if advertised.points_at(&tag.name, &tag.commit) => {
            (Some(Progress::Pushed), err)
        }
        Ok(_) => (progress, err),
        Err(reread) => {
            // The embedded error sheds its outcome token: one verdict, said
            // once, at the front.
            let reread = reread.detail();
            (
                Some(Progress::PushUnverified),
                CliError::unverified(format!(
                    "unverified: {err}; the re-read that would settle whether \
                     the tag landed failed too ({reread})"
                )),
            )
        }
    }
}

fn partial_failure(
    completed: &[String],
    progress: Option<Progress>,
    current: &str,
    remaining: &[PlannedTag],
    err: &CliError,
) -> CliError {
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
    detail.push_str(&err.to_string());
    CliError::new(detail)
}

fn worktree_is_dirty(git: &Git) -> Result<bool, CliError> {
    Ok(!git.text(Op::WorktreeStatus)?.is_empty())
}

/// One gate over the tags oakum is about to push, each read at its own
/// commit: GitHub reads the skip annotation from the commit a push delivers,
/// a pending tag delivers HEAD, and a resumed one delivers the commit it was
/// cut at. Stated over the push set so no path can under-cover it.
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
                 release workflow would never start. Land a new commit without \
                 the annotation (an empty one works), re-cut the tag at a \
                 clean commit, or start the downstream workflow by hand",
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
        "unverified: {err}, so oakum could not ask GitHub whether these tags \
         are released"
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
        decide_release, refuse_skip_ci, refuse_without_a_look, skip_ci, unverified_look,
        valid_tag_name, worktree_is_dirty, Advertised, Commit, ExistingTag, Git, HaveRemote,
        HaveToken, PendingRelease, PlannedTag, ReleaseDecision, ReleaseReadiness, Version,
    };
    use crate::cli::git::Reply;
    use crate::cli::CliError;

    /// The helper is pure, so every combination is a unit assertion rather
    /// than a spawned binary.
    #[test]
    fn decide_nothing_when_no_work_and_no_credentials() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: false,
            resumable: Vec::new(),
            have_remote: HaveRemote(false),
            have_token: HaveToken(false),
            remote: None,
            token: None,
            planned: Vec::new(),
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: None,
            worktree_dirty: false,
            owner: None,
            repo_name: None,
        });
        assert!(matches!(decision, ReleaseDecision::NothingToRelease));
    }

    #[test]
    fn decide_refuses_a_dirty_worktree_before_proceeding() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: true,
            resumable: Vec::new(),
            have_remote: HaveRemote(true),
            have_token: HaveToken(true),
            remote: Some(String::from("origin")),
            token: Some(String::from("token")),
            planned: vec![planned_tag("v0.1.1")],
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: None,
            worktree_dirty: true,
            owner: Some(String::from("oakoss")),
            repo_name: Some(String::from("oakum")),
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("dirty"), "{err}");
    }

    #[test]
    fn decide_refuses_on_preflight_error() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: true,
            resumable: Vec::new(),
            have_remote: HaveRemote(true),
            have_token: HaveToken(true),
            remote: Some(String::from("origin")),
            token: Some(String::from("token")),
            planned: vec![planned_tag("v0.1.1")],
            advertised: Advertised::default(),
            preflight_error: Some(CliError::new(
                "`oakum release` needs GITHUB_TOKEN or GH_TOKEN",
            )),
            skip_ci_error: None,
            worktree_dirty: false,
            owner: Some(String::from("oakoss")),
            repo_name: Some(String::from("oakum")),
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("GITHUB_TOKEN"), "{err}");
    }

    #[test]
    fn decide_proceeds_when_readiness_is_complete() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: true,
            resumable: Vec::new(),
            have_remote: HaveRemote(true),
            have_token: HaveToken(true),
            remote: Some(String::from("origin")),
            token: Some(String::from("token")),
            planned: vec![planned_tag("v0.1.1")],
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: None,
            worktree_dirty: false,
            owner: Some(String::from("oakoss")),
            repo_name: Some(String::from("oakum")),
        });
        assert!(matches!(decision, ReleaseDecision::Proceed(_)));
    }

    #[test]
    fn decide_nothing_when_credentials_exist_but_there_is_no_work() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: false,
            resumable: Vec::new(),
            have_remote: HaveRemote(true),
            have_token: HaveToken(true),
            remote: Some(String::from("origin")),
            token: Some(String::from("token")),
            planned: Vec::new(),
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: None,
            worktree_dirty: false,
            owner: Some(String::from("oakoss")),
            repo_name: Some(String::from("oakum")),
        });
        assert!(matches!(decision, ReleaseDecision::NothingToRelease));
    }

    #[test]
    fn decide_refuses_a_resume_look_without_credentials() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: false,
            resumable: vec![resumable_tag("v0.1.0")],
            have_remote: HaveRemote(true),
            have_token: HaveToken(false),
            remote: Some(String::from("origin")),
            token: None,
            planned: Vec::new(),
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: None,
            worktree_dirty: false,
            owner: None,
            repo_name: None,
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
            had_planned_before_resume: true,
            resumable: Vec::new(),
            have_remote: HaveRemote(true),
            have_token: HaveToken(true),
            remote: Some(String::from("origin")),
            token: Some(String::from("token")),
            planned: vec![planned_tag("v0.1.1")],
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: Some(CliError::new(
                "refusing to release: HEAD commit is marked skip-ci",
            )),
            worktree_dirty: false,
            owner: Some(String::from("oakoss")),
            repo_name: Some(String::from("oakum")),
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("skip-ci"), "{err}");
    }

    #[test]
    fn decide_refuses_when_github_coordinates_are_missing() {
        let decision = decide_release(ReleaseReadiness {
            had_planned_before_resume: true,
            resumable: Vec::new(),
            have_remote: HaveRemote(true),
            have_token: HaveToken(true),
            remote: None,
            token: Some(String::from("token")),
            planned: vec![planned_tag("v0.1.1")],
            advertised: Advertised::default(),
            preflight_error: None,
            skip_ci_error: None,
            worktree_dirty: false,
            owner: None,
            repo_name: None,
        });
        let ReleaseDecision::Refused(err) = decision else {
            panic!("expected refusal, got {decision:?}");
        };
        assert!(err.to_string().contains("GitHub coordinates"), "{err}");
    }

    fn planned_tag(name: &str) -> PlannedTag {
        let commit = Git::answering([("rev-parse HEAD", Reply::said("cafe"))])
            .head()
            .expect("minted");
        PlannedTag {
            package: String::from("demo"),
            version: Version::new(0, 1, 1),
            name: String::from(name),
            commit,
            resumed: false,
        }
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
        refuse_without_a_look(HaveRemote(true), HaveToken(true)).expect("both present");

        let no_remote = refuse_without_a_look(HaveRemote(false), HaveToken(true))
            .expect_err("a missing remote refuses");
        assert!(no_remote
            .to_string()
            .contains("this repository has no remote"));
        assert!(!no_remote.to_string().contains("GITHUB_TOKEN"));

        let no_token = refuse_without_a_look(HaveRemote(true), HaveToken(false))
            .expect_err("a missing token refuses");
        assert!(no_token.to_string().contains("GITHUB_TOKEN and GH_TOKEN"));
        assert!(!no_token.to_string().contains("has no remote"));

        let neither = refuse_without_a_look(HaveRemote(false), HaveToken(false))
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
        assert!(matches!(err, CliError::Unverified { .. }), "{err:?}");
        assert!(err.to_string().starts_with("unverified:"), "{err}");
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
}
