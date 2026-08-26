//! `oakum release` writes tags and GitHub releases (ADR-0023). Same local
//! preconditions as `check` (ADR-0020); no rollback (ADR-0011).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use clap::Args;
use semver::Version;
use serde_json::json;

use super::add;
use super::ci;
use super::config::{enforce_tool_version, load_config};
use super::git_env;
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
    fn new(package: String, version: Version, name: String) -> Result<Self, CliError> {
        valid_tag_name(&name)?;
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
    let evaluation = preconditions::evaluate(&repo, args.from.as_deref(), false, false, 3)?;
    let pending = evaluation.pending();
    if !pending.is_empty() {
        refuse_dirty_worktree(repo.path())?;
        refuse_skip_ci(repo.path())?;
    }
    let mut planned = plan_tags(&repo, &pending)?;
    let remote = tags::first_remote(repo.path())?;
    if planned.is_empty() && (github_token().is_err() || remote.is_none()) {
        println!("nothing to release");
        return Ok(());
    }
    let remote = remote.ok_or_else(|| {
        CliError::unverified("unverified: this repository has no remotes to push tags to")
    })?;
    let token = github_token()?;
    let client = github::Client::new(token)?;
    let (owner, name) = ci::repository_slug_from(repo.path(), &remote)?;
    planned.extend(resume_tags(
        &repo,
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
    refuse_dirty_worktree(repo.path())?;
    preflight(&repo, &client, &owner, &name, &remote, &planned)?;
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
    act(
        &repo,
        &client,
        &owner,
        &name,
        &remote,
        &planned,
        &downstream,
    )
}

fn plan_tags(
    repo: &repository::Repository,
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
        valid_tag_name(&name)?;
        rendered.push((item, name));
    }
    let mut planned = Vec::new();
    for (item, name) in rendered {
        readable_for_package(&workspace, item, &name)?;
        planned.push(PlannedTag::new(
            item.id().name.clone(),
            item.version().clone(),
            name,
        )?);
    }
    Ok(planned)
}

fn resume_tags(
    repo: &repository::Repository,
    client: &github::Client,
    owner: &str,
    name: &str,
    evaluation: &TagEvaluation,
    already: &[PlannedTag],
) -> Result<Vec<PlannedTag>, CliError> {
    let existing: BTreeSet<&str> = already.iter().map(|tag| tag.name.as_str()).collect();
    let template = tag_template(repo)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let head = git_stdout(repo.path(), &["rev-parse", "HEAD"])?;
    let mut extra = Vec::new();
    for item in evaluation.current() {
        let rendered = render_tag(&template, &item)?;
        if existing.contains(rendered.as_str()) {
            continue;
        }
        if local_tag_commit(repo.path(), &rendered)?.as_deref() != Some(head.as_str()) {
            continue;
        }
        readable_for_package(&workspace, &item, &rendered)?;
        let tag = PlannedTag::new(item.id().name.clone(), item.version().clone(), rendered)?;
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

fn valid_tag_name(name: &str) -> Result<(), CliError> {
    if name.starts_with('-') {
        return Err(CliError::new(format!(
            "tag-format rendered an invalid git ref: {name:?}"
        )));
    }
    let spec = format!("refs/tags/{name}");
    let output = Command::new("git")
        .args(["check-ref-format", &spec])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| CliError::new(format!("failed to run git check-ref-format: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(CliError::new(format!(
        "tag-format rendered an invalid git ref: {name:?}"
    )))
}

fn preflight(
    repo: &repository::Repository,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: &[PlannedTag],
) -> Result<(), CliError> {
    let advertised = tags::remote_tag_commits(repo.path(), remote)?;
    let head = git_stdout(repo.path(), &["rev-parse", "HEAD"])?;
    for tag in planned {
        if let Some(existing) = advertised.get(&tag.name) {
            if existing != &head {
                return Err(CliError::new(format!(
                    "remote {remote:?} already has tag `{}` at `{existing}`, not HEAD `{head}`",
                    tag.name
                )));
            }
        }
        if let Some(existing) = local_tag_commit(repo.path(), &tag.name)? {
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
    repo: &repository::Repository,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    planned: &[PlannedTag],
    downstream: &Downstream,
) -> Result<(), CliError> {
    let mut completed = Vec::new();
    let head = git_stdout(repo.path(), &["rev-parse", "HEAD"])?;
    let mut seen = match downstream {
        Downstream::PushTags { .. } => Some(handoff::snapshot(client, owner, name, &head)?),
        _ => None,
    };
    for (index, tag) in planned.iter().enumerate() {
        match release_one(repo, client, owner, name, remote, &head, tag) {
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
    repo: &repository::Repository,
    client: &github::Client,
    owner: &str,
    name: &str,
    remote: &str,
    head: &str,
    tag: &PlannedTag,
) -> Result<bool, (Option<Progress>, CliError)> {
    let mut progress = None;
    let advertised =
        tags::remote_tag_commits(repo.path(), remote).map_err(|err| (progress, err))?;
    let remote_at_head = advertised.get(&tag.name).is_some_and(|sha| sha == head);
    if local_tag_commit(repo.path(), &tag.name)
        .map_err(|err| (progress, err))?
        .is_none()
        && !remote_at_head
    {
        git_ok(
            repo.path(),
            &["tag", "-m", &tag.name, "--", &tag.name, head],
        )
        .map_err(|err| (progress, err))?;
        progress = Some(Progress::Tagged);
    }
    let did_push = advertised.get(&tag.name).is_none_or(|sha| sha != head);
    if did_push {
        git_ok_remote(
            repo.path(),
            &["push", "--", remote, &format!("refs/tags/{}", tag.name)],
        )
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

fn refuse_dirty_worktree(repo: &Path) -> Result<(), CliError> {
    let porcelain = git_stdout(repo, &["status", "--porcelain", "--untracked-files=all"])?;
    if porcelain.is_empty() {
        return Ok(());
    }
    Err(CliError::new(
        "working tree is dirty; commit or stash before tagging HEAD",
    ))
}

fn refuse_skip_ci(repo: &Path) -> Result<(), CliError> {
    let message = git_stdout(repo, &["log", "-1", "--format=%B"])?;
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

fn local_tag_commit(repo: &Path, name: &str) -> Result<Option<String>, CliError> {
    let spec = format!("refs/tags/{name}^{{}}");
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &spec])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| CliError::new(format!("failed to run git rev-parse: {err}")))?;
    if !output.status.success() {
        return Ok(None);
    }
    let sha = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new("git rev-parse output is not valid UTF-8"))?;
    let sha = sha.trim();
    if sha.is_empty() {
        return Ok(None);
    }
    Ok(Some(sha.to_owned()))
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String, CliError> {
    let output = git(repo, args)?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new(format!("git {} output is not valid UTF-8", args.join(" "))))?;
    Ok(stdout.trim().to_owned())
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<(), CliError> {
    git(repo, args)?;
    Ok(())
}

/// For a child that contacts a remote, where a prompt blocks instead of failing.
fn git_ok_remote(repo: &Path, args: &[&str]) -> Result<(), CliError> {
    let output = git_env::remote_command(repo, args)?
        .output()
        .map_err(|err| CliError::new(format!("failed to run git {}: {err}", args.join(" "))))?;
    check_status(&output, args)?;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<std::process::Output, CliError> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0");
    finish(&mut command, args)
}

fn finish(command: &mut Command, args: &[&str]) -> Result<std::process::Output, CliError> {
    let output = command
        .output()
        .map_err(|err| CliError::new(format!("failed to run git {}: {err}", args.join(" "))))?;
    check_status(&output, args)?;
    Ok(output)
}

fn check_status(output: &std::process::Output, args: &[&str]) -> Result<(), CliError> {
    if !output.status.success() {
        return Err(CliError::new(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

fn github_token() -> Result<String, CliError> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(token) = std::env::var(key) {
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }
    Err(CliError::new(
        "`oakum release` needs GITHUB_TOKEN or GH_TOKEN",
    ))
}

#[cfg(test)]
mod tests {
    use super::{skip_ci, valid_tag_name};

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
        assert!(valid_tag_name("v0.1.1").is_ok());
        assert!(valid_tag_name("pkg/v0.1.1").is_ok());
        assert!(valid_tag_name("foo..bar").is_err());
        assert!(valid_tag_name("foo.lock").is_err());
        assert!(valid_tag_name("foo@{bar}").is_err());
        assert!(valid_tag_name("-v0.1.1").is_err());
        assert!(valid_tag_name("--delete").is_err());
    }
}
