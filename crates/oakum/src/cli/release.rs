//! `oakum release` writes tags and GitHub releases (ADR-0023). Same local
//! preconditions as `check` (ADR-0020); no rollback (ADR-0011).

use std::collections::BTreeSet;

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

struct PlannedTag {
    package: String,
    version: Version,
    name: String,
}

impl PlannedTag {
    fn new(git: &Git, package: String, version: Version, name: String) -> Result<Self, CliError> {
        valid_tag_name(git, &name)?;
        Ok(Self {
            package,
            version,
            name,
        })
    }
}

#[derive(Clone, Copy)]
enum Progress {
    Tagged,
    Pushed,
    Released,
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
    let remote = tags::first_remote(&git)?;
    if planned.is_empty() && (github::token().is_none() || remote.is_none()) {
        println!("nothing to release");
        return Ok(());
    }
    let remote = remote.ok_or_else(|| {
        CliError::unverified("unverified: this repository has no remotes to push tags to")
    })?;
    let token = github::token()
        .ok_or_else(|| CliError::new("`oakum release` needs GITHUB_TOKEN or GH_TOKEN"))?;
    let client = github::Client::new(token)?;
    let (owner, name) = ci::repository_slug_from(repo.path(), &remote)?;
    planned.extend(resume_tags(
        &repo,
        &git,
        &client,
        &owner,
        &name,
        &evaluation,
        &planned,
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
    evaluation: &TagEvaluation,
    already: &[PlannedTag],
) -> Result<Vec<PlannedTag>, CliError> {
    let existing: BTreeSet<&str> = already.iter().map(|tag| tag.name.as_str()).collect();
    let template = tag_template(repo)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let head = git.text(Op::Head)?;
    let mut extra = Vec::new();
    for item in evaluation.current() {
        let rendered = render_tag(&template, &item)?;
        if existing.contains(rendered.as_str()) {
            continue;
        }
        if local_tag_commit(git, &rendered)?.as_deref() != Some(head.as_str()) {
            continue;
        }
        readable_for_package(&workspace, &item, &rendered)?;
        let tag = PlannedTag::new(
            git,
            item.id().name.clone(),
            item.version().clone(),
            rendered,
        )?;
        match client.release_for_tag(owner, name, &tag.name)? {
            Look::Empty => extra.push(tag),
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
    let head = git.text(Op::Head)?;
    for tag in planned {
        if let Some(existing) = advertised.get(&tag.name) {
            if existing != &head {
                return Err(CliError::new(format!(
                    "remote {remote:?} already has tag `{}` at `{existing}`, not HEAD `{head}`",
                    tag.name
                )));
            }
        }
        if let Some(existing) = local_tag_commit(git, &tag.name)? {
            if existing != head {
                return Err(CliError::new(format!(
                    "local tag `{}` points at `{existing}`, not HEAD `{head}`",
                    tag.name
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
    let mut seen = match downstream {
        Downstream::PushTags { .. } => Some(handoff::snapshot(client, owner, name, &head)?),
        _ => None,
    };
    for (index, tag) in planned.iter().enumerate() {
        match release_one(git, client, owner, name, remote, &head, tag) {
            Ok(did_push) => {
                if let (Downstream::PushTags { paths }, Some(seen)) = (downstream, seen.as_mut()) {
                    match handoff::confirm(client, owner, name, &head, paths, seen, did_push) {
                        Ok(run) => {
                            if !run.html_url.is_empty() {
                                println!("{}", run.html_url);
                            }
                            if index + 1 < planned.len() {
                                if let Err(err) = handoff::absorb(client, owner, name, &head, seen)
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
                                &err,
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
    head: &str,
    tag: &PlannedTag,
) -> Result<bool, (Option<Progress>, CliError)> {
    let mut progress = None;
    let advertised = tags::remote_tag_commits(git, remote).map_err(|err| (progress, err))?;
    let remote_at_head = advertised.get(&tag.name).is_some_and(|sha| sha == head);
    if local_tag_commit(git, &tag.name)
        .map_err(|err| (progress, err))?
        .is_none()
        && !remote_at_head
    {
        git.run(Op::AnnotatedTag {
            name: &tag.name,
            commit: head,
        })
        .map_err(|err| (progress, err))?;
        progress = Some(Progress::Tagged);
    }
    let did_push = advertised.get(&tag.name).is_none_or(|sha| sha != head);
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
    detail.push_str("remaining:\n");
    for tag in remaining {
        detail.push_str("  ");
        detail.push_str(&tag.name);
        detail.push('\n');
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
        "working tree is dirty; commit or stash before tagging HEAD",
    ))
}

fn refuse_skip_ci(git: &Git) -> Result<(), CliError> {
    let message = git.text(Op::HeadMessage)?;
    if skip_ci(&message) {
        return Err(CliError::new(
            "HEAD commit message contains a skip-ci annotation; refusing to tag",
        ));
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
}

fn local_tag_commit(git: &Git, name: &str) -> Result<Option<String>, CliError> {
    git.optional_text(Op::LocalTagCommit { tag: name })
}

#[cfg(test)]
mod tests {
    use super::{refuse_dirty_worktree, refuse_skip_ci, skip_ci, valid_tag_name, Git};
    use crate::cli::git::Reply;

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

    #[test]
    fn skip_ci_matches_github_annotations() {
        assert!(skip_ci("fix: typo [skip ci]"));
        assert!(skip_ci("[CI SKIP] docs"));
        assert!(skip_ci("chore: [no ci]"));
        assert!(skip_ci("[skip actions]\nmore"));
        assert!(skip_ci("[actions skip]"));
        assert!(!skip_ci("fix: do not skip"));
        assert!(!skip_ci("skip ci without brackets"));
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
