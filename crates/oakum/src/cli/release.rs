//! `oakum release` writes tags and GitHub releases (ADR-0023). Same local
//! preconditions as `check` (ADR-0020); no rollback (ADR-0011).

use std::collections::{BTreeMap, BTreeSet};

use clap::Args;
use semver::Version;
use serde_json::json;

use super::add;
use super::ci;
use super::config::{enforce_tool_version, load_config};
use super::git::{Git, Op};
use super::github::{self, Look};
use super::handoff::{self, Downstream};
use super::preconditions::{self, PendingRelease, TagEvaluation};
use super::repository;
use super::tags;
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

/// A commit id, kept distinct from a ref name so the two cannot be transposed
/// at a call site: `git check-ref-format` accepts a bare sha, so validating the
/// name catches nothing.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Commit(String);

impl Commit {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

struct PlannedTag {
    package: String,
    version: Version,
    name: String,
    /// Where the tag points, or will. A resumed tag keeps the commit it was
    /// cut at; a new one names HEAD.
    commit: Commit,
}

impl PlannedTag {
    fn new(
        git: &Git,
        package: String,
        version: Version,
        name: String,
        commit: Commit,
    ) -> Result<Self, CliError> {
        valid_tag_name(git, &name)?;
        Ok(Self {
            package,
            version,
            name,
            commit,
        })
    }
}

#[derive(Clone, Copy)]
enum Progress {
    Tagged,
    Pushed,
    Released,
}

#[derive(Clone, Copy)]
struct HaveRemote(bool);

#[derive(Clone, Copy)]
struct HaveToken(bool);

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

/// The tags `resume_tags` consults GitHub about, derived once so the
/// could-not-look gate and the resume itself cannot disagree about the set.
/// Name and commit are read in one pass so the two cannot drift apart.
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
    let evaluation = preconditions::evaluate(&repo, args.from.as_deref(), false, false, 3)?;
    let pending = evaluation.pending();
    if !pending.is_empty() {
        refuse_dirty_worktree(&git)?;
        refuse_skip_ci(&git)?;
    }
    let mut planned = plan_tags(&repo, &git, &pending)?;
    let resumable = existing_tags(&repo, &git, &evaluation)?;
    let remote = tags::first_remote(&git)?;
    let token = github::token();
    let have_remote = HaveRemote(remote.is_some());
    let have_token = HaveToken(token.is_some());
    // An existing tag for a current version is the only thing `resume_tags`
    // asks GitHub about, so it is also the only thing a missing token or remote
    // hides. With none there the answer was fully local and a verdict is a look
    // that happened.
    if planned.is_empty() {
        if !resumable.is_empty() {
            refuse_without_a_look(have_remote, have_token)?;
        }
        if remote.is_none() || token.is_none() {
            println!("nothing to release");
            return Ok(());
        }
    }
    let remote = remote.ok_or_else(|| {
        CliError::unverified("unverified: this repository has no remotes to push tags to")
    })?;
    let token =
        token.ok_or_else(|| CliError::new("`oakum release` needs GITHUB_TOKEN or GH_TOKEN"))?;
    let client = github::Client::new(token)?;
    let (owner, name) =
        ci::repository_slug_from(repo.path(), &remote).map_err(|err| unverified_look(&err))?;
    planned.extend(resume_tags(
        &repo, &git, &client, &owner, &name, &resumable, &planned,
    )?);
    if planned.is_empty() {
        println!("nothing to release");
        return Ok(());
    }
    refuse_dirty_worktree(&git)?;
    preflight(&git, &client, &owner, &name, &remote, &planned)?;
    let downstream = handoff::discover(repo.dir())?;
    match &downstream {
        Downstream::None => {
            eprintln!("no downstream workflow listens for tags");
        }
        Downstream::DispatchOnly { paths } => {
            return Err(CliError::new(format!(
                "downstream {} is workflow_dispatch; a tag push will not start it",
                paths.join(", ")
            )));
        }
        Downstream::PushTags { .. } => {}
    }
    act(&git, &client, &owner, &name, &remote, &planned, &downstream)
}

fn plan_tags(
    repo: &repository::Repository,
    git: &Git,
    items: &[PendingRelease],
) -> Result<Vec<PlannedTag>, CliError> {
    let template = tag_template(repo)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let head = Commit(git.text(Op::Head)?);
    let mut rendered = Vec::new();
    let mut names = BTreeSet::new();
    for item in items {
        let name = render_tag(&template, item)?;
        if !names.insert(name.clone()) {
            return Err(CliError::new(format!(
                "tag-format rendered `{name}` for more than one package"
            )));
        }
        // Ahead of `readable_for_package` below, and so ahead of
        // `PlannedTag::new`'s own check, so a name git rejects outright is
        // reported as an invalid ref rather than as an unreadable tag.
        valid_tag_name(git, &name)?;
        rendered.push((item, name));
    }
    let mut planned = Vec::new();
    for (item, name) in rendered {
        readable_for_package(&workspace, item, &name)?;
        planned.push(PlannedTag::new(
            git,
            item.id().name.clone(),
            item.version().clone(),
            name,
            head.clone(),
        )?);
    }
    Ok(planned)
}

fn resume_tags(
    repo: &repository::Repository,
    git: &Git,
    client: &github::Client,
    owner: &str,
    name: &str,
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
        let tag = PlannedTag::new(
            git,
            candidate.item.id().name.clone(),
            candidate.item.version().clone(),
            candidate.name.clone(),
            candidate.commit.clone(),
        )?;
        match client.release_for_tag(owner, name, &tag.name)? {
            // A released tag at a skip-ci commit is a finished state; only a
            // tag still needing its push can stall, so only it is gated.
            Look::Empty => {
                refuse_skip_ci_tagged(git, &tag.name, &tag.commit)?;
                extra.push(tag);
            }
            Look::Found(_) => {}
        }
    }
    Ok(extra)
}

fn tag_template(repo: &repository::Repository) -> Result<String, CliError> {
    let config = load_config(repo).map_err(CliError::from_boxed)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let publishable = workspace
        .packages()
        .filter(|package| package.publishable())
        .count();
    let default = if publishable <= 1 {
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
    let rendered = oakum::template::render(
        "tag-format",
        template,
        json!({
            "package": item.id().name,
            "version": item.version().to_string(),
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

fn preflight(
    git: &Git,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: &[PlannedTag],
) -> Result<(), CliError> {
    let advertised = tags::remote_tag_commits(git, remote)?;
    for tag in planned {
        if let Some(existing) = advertised.get(&tag.name) {
            if existing != tag.commit.as_str() {
                return Err(CliError::new(format!(
                    "remote {remote:?} already has tag `{}` at `{existing}`, not `{}`",
                    tag.name,
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
        match client.release_for_tag(owner, name, &tag.name)? {
            Look::Empty => {}
            Look::Found(release) => {
                return Err(CliError::new(format!(
                    "GitHub already has a release for `{}` ({})",
                    tag.name, release.html_url
                )));
            }
        }
    }
    Ok(())
}

fn act(
    git: &Git,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: &[PlannedTag],
    downstream: &Downstream,
) -> Result<(), CliError> {
    let mut completed = Vec::new();
    let head = git.text(Op::Head)?;
    // Every commit is snapshotted before anything is written, so a failed look
    // cannot strand a pushed tag outside the partial-failure report.
    let mut seen: BTreeMap<Commit, handoff::SeenRuns> = BTreeMap::new();
    if matches!(downstream, Downstream::PushTags { .. }) {
        for tag in planned {
            if !seen.contains_key(&tag.commit) {
                let before = handoff::snapshot(client, owner, name, &tag.commit)?;
                seen.insert(tag.commit.clone(), before);
            }
        }
    }
    for (index, tag) in planned.iter().enumerate() {
        match release_one(git, client, owner, name, remote, tag) {
            Ok(did_push) => {
                if let Downstream::PushTags { paths } = downstream {
                    let Some(seen) = seen.get_mut(&tag.commit) else {
                        return Err(partial_failure(
                            &completed,
                            Some(Progress::Released),
                            &tag.name,
                            &planned[index + 1..],
                            &CliError::unverified(format!(
                                "unverified: no pre-push snapshot for `{}`, so the \
                                 handoff for `{}` cannot be confirmed",
                                tag.commit.as_str(),
                                tag.name
                            )),
                        ));
                    };
                    match handoff::confirm(client, owner, name, &tag.commit, paths, seen, did_push)
                    {
                        Ok(run) => {
                            if !run.html_url.is_empty() {
                                println!("{}", run.html_url);
                            }
                            if planned[index + 1..]
                                .iter()
                                .any(|later| later.commit == tag.commit)
                            {
                                if let Err(err) =
                                    handoff::absorb(client, owner, name, &tag.commit, seen)
                                {
                                    return Err(partial_failure(
                                        &completed,
                                        Some(Progress::Released),
                                        &tag.name,
                                        &planned[index + 1..],
                                        &err,
                                    ));
                                }
                            }
                        }
                        Err(err) => {
                            return Err(partial_failure(
                                &completed,
                                Some(Progress::Released),
                                &tag.name,
                                &planned[index + 1..],
                                &listener_caveat(err, tag, &head),
                            ));
                        }
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
) -> Result<bool, (Option<Progress>, CliError)> {
    let mut progress = None;
    let advertised = tags::remote_tag_commits(git, remote).map_err(|err| (progress, err))?;
    let remote_has_it = advertised
        .get(&tag.name)
        .is_some_and(|sha| sha == tag.commit.as_str());
    if local_tag_commit(git, &tag.name)
        .map_err(|err| (progress, err))?
        .is_none()
        && !remote_has_it
    {
        // Only a planned tag reaches this: a resumed one already exists, which
        // is what `existing_tags` selected it for. So the commit here is HEAD.
        git.run(Op::AnnotatedTag {
            name: &tag.name,
            commit: tag.commit.as_str(),
        })
        .map_err(|err| (progress, err))?;
        progress = Some(Progress::Tagged);
    }
    let did_push = advertised
        .get(&tag.name)
        .is_none_or(|sha| sha != tag.commit.as_str());
    if did_push {
        git.run(Op::PushTag {
            remote,
            tag: &tag.name,
        })
        .map_err(|err| (progress, err))?;
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

/// `handoff::discover` reads the worktree, so for a tag cut at an older commit
/// the listener set may not be the one GitHub evaluated. Saying so is the
/// difference between a missing run and a workflow that never existed there.
fn listener_caveat(err: CliError, tag: &PlannedTag, head: &str) -> CliError {
    if tag.commit.as_str() == head {
        return err;
    }
    CliError::unverified(format!(
        "{err}; the workflows looked for were read from the worktree, which may \
         differ from the tree at `{}`",
        tag.commit.as_str()
    ))
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

fn refuse_dirty_worktree(git: &Git) -> Result<(), CliError> {
    let porcelain = git.text(Op::WorktreeStatus)?;
    if porcelain.is_empty() {
        return Ok(());
    }
    Err(CliError::new(
        "working tree is dirty; commit or stash before releasing",
    ))
}

fn refuse_skip_ci(git: &Git) -> Result<(), CliError> {
    let message = git.text(Op::CommitMessage { commit: "HEAD" })?;
    if skip_ci(&message) {
        return Err(CliError::new(
            "HEAD commit message contains a skip-ci annotation, which GitHub \
             honours for tag pushes: the release workflow would never start. \
             Land a new commit without the annotation (an empty one works), \
             then rerun",
        ));
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

/// GitHub reads the skip annotation from the commit a push delivers, and a
/// resumed tag delivers its own commit, not HEAD.
fn refuse_skip_ci_tagged(git: &Git, name: &str, commit: &Commit) -> Result<(), CliError> {
    let message = git.text(Op::CommitMessage {
        commit: commit.as_str(),
    })?;
    if skip_ci(&message) {
        return Err(CliError::new(format!(
            "tag `{name}` points at a commit whose message contains a skip-ci \
             annotation, which GitHub honours for tag pushes: the release \
             workflow would never start. Start the downstream workflow by \
             hand, or re-cut the tag at a commit without the annotation"
        )));
    }
    Ok(())
}

fn skip_ci(message: &str) -> bool {
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
        || skip_checks_trailer(&lower)
}

/// GitHub's documented alternative to the bracketed annotations: a
/// `skip-checks` trailer valued `true`, last in the trailer block ending the
/// message. Parsed as a block so a prose mention does not refuse a release.
/// Where the doc and git's parser disagree — blank lines before the block,
/// whitespace around the value, prose lines sharing a block with a real
/// trailer (all measured against `git interpret-trailers --parse`) — the
/// broader reading refuses, trading a spurious local refusal for never
/// pushing a tag whose workflow was suppressed.
fn skip_checks_trailer(lower: &str) -> bool {
    let lines: Vec<&str> = lower.trim_end().lines().collect();
    let Some(blank) = lines.iter().rposition(|line| line.trim().is_empty()) else {
        return false;
    };
    // git's unfold: a line starting with whitespace continues the one above.
    let mut block: Vec<String> = Vec::new();
    for line in &lines[blank + 1..] {
        match block.last_mut() {
            Some(prev) if line.starts_with(char::is_whitespace) => {
                prev.push(' ');
                prev.push_str(line.trim_start());
            }
            _ => block.push((*line).to_owned()),
        }
    }
    let Some((last, rest)) = block.split_last() else {
        return false;
    };
    let Some((key, value)) = last.split_once(':') else {
        return false;
    };
    if key.trim() != "skip-checks" || value.trim() != "true" {
        return false;
    }
    rest.is_empty()
        || rest
            .iter()
            .any(|line| trailer_shaped(line) || line.starts_with("(cherry picked from commit "))
}

/// `key: value` with a git trailer token — letters, digits, and hyphens.
fn trailer_shaped(line: &str) -> bool {
    let Some((key, _)) = line.split_once(':') else {
        return false;
    };
    let key = key.trim();
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn local_tag_commit(git: &Git, name: &str) -> Result<Option<Commit>, CliError> {
    Ok(git
        .optional_text(Op::LocalTagCommit { tag: name })?
        .map(Commit))
}

#[cfg(test)]
mod tests {
    /// The helper is pure, so every combination is a unit assertion rather
    /// than a spawned binary.
    #[test]
    fn a_look_needs_both_a_remote_and_a_token() {
        use super::{refuse_without_a_look, HaveRemote, HaveToken};

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

    use super::{
        refuse_dirty_worktree, refuse_skip_ci, skip_ci, unverified_look, valid_tag_name, Git,
    };
    use crate::cli::git::Reply;
    use crate::cli::CliError;

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
        refuse_dirty_worktree(&clean).expect("a clean worktree releases");
        assert_eq!(clean.asked().len(), 1, "{:?}", clean.asked());

        let dirty = Git::answering([(STATUS, Reply::said(" M crates/oakum/src/lib.rs"))]);
        let err = refuse_dirty_worktree(&dirty).expect_err("a dirty worktree stops");
        assert!(err.to_string().contains("dirty"), "{err}");
    }

    #[test]
    fn a_skip_ci_marker_on_head_stops_the_release() {
        let marked = Git::answering([("log -1", Reply::said("fix: typo [skip ci]"))]);
        let err = refuse_skip_ci(&marked).expect_err("a skip-ci HEAD stops");
        assert!(err.to_string().contains("skip-ci"), "{err}");

        let plain = Git::answering([("log -1", Reply::said("fix: typo"))]);
        refuse_skip_ci(&plain).expect("an unmarked HEAD releases");
        assert_eq!(plain.asked().len(), 1, "{:?}", plain.asked());
    }

    /// The five bracketed strings and the two trailer forms come from
    /// github/docs skip-workflow-runs.md, which is the whole list GitHub
    /// honours; a tag push is an `on: push` event, so it is covered too.
    #[test]
    fn skip_ci_matches_github_annotations_and_trailers() {
        assert!(skip_ci("fix: typo [skip ci]"));
        assert!(skip_ci("[CI SKIP] docs"));
        assert!(skip_ci("chore: [no ci]"));
        assert!(skip_ci("[skip actions]\nmore"));
        assert!(skip_ci("[actions skip]"));
        assert!(!skip_ci("fix: do not skip"));
        assert!(!skip_ci("skip ci without brackets"));

        assert!(skip_ci("fix: typo\n\nskip-checks:true"));
        assert!(skip_ci("fix: typo\n\nskip-checks: true"));
        assert!(skip_ci("fix: typo\n\nSkip-Checks: true"));
        // Whitespace-padded values git's parser accepts; the doc lists only
        // the two exact spellings, and refusing broadly is the safe side.
        assert!(skip_ci("fix: typo\n\nskip-checks:  true"));
        assert!(skip_ci("fix: typo\n\nskip-checks:\ttrue"));
        assert!(skip_ci("fix: typo\n\nskip-checks : true"));
        assert!(skip_ci(
            "fix: typo\n\nbody\n\nSigned-off-by: a\nskip-checks: true"
        ));
        assert!(skip_ci(
            "fix: typo\n\nfoo: bar\n  folded value\nskip-checks: true"
        ));
        assert!(skip_ci("fix: typo\n\nskip-checks: true\n"));
        // A prose block with a trailer among its lines is refused (git parses
        // such mixed blocks); prose alone, prose mentions, and non-final
        // trailers are not.
        assert!(skip_ci(
            "fix: typo\n\nbody text\nSigned-off-by: a\nskip-checks: true"
        ));
        assert!(skip_ci(
            "fix: t\n\n(cherry picked from commit abc)\nskip-checks: true"
        ));
        assert!(skip_ci("fix: t\n\nskip-checks:\n true"));
        assert!(!skip_ci("docs: explain the skip-checks:true trailer"));
        assert!(!skip_ci(
            "fix: typo\n\nsee skip-checks: true, then more prose"
        ));
        assert!(!skip_ci("fix: typo\n\nskip-checks: true\nSigned-off-by: a"));
        assert!(!skip_ci("skip-checks: true"));
        assert!(!skip_ci("fix: typo\n\nskip-checks: false"));
        assert!(!skip_ci(
            "fix: typo\n\nparagraph one\n\nbody text\nskip-checks: true"
        ));
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
