//! `oakum ci`: GitHub writes for CI. `version-pr` opens or updates the version
//! PR. `pr-status` posts the contributor-PR comment and job summary.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use oakum::config::PrStatus;
use oakum::plan::{aggregate, compose, CascadeAs};
use oakum::state::{ReleaseState, RenderTarget};
use serde_json::Value;

use super::add;
use super::config::load_config;
use super::coverage;
use super::git::{Git, Op};
use super::github::{self, FileAddition, FileChanges, FileDeletion, Look};
use super::intent::load_plan_bump_files;
use super::repository;
use super::status;
use super::template::load_template_body;
use super::version::{self, VersionArgs, VersionWritePlan};
use super::CliError;

const VERSION_BRANCH: &str = "oakum/version-packages";
const DEFAULT_TITLE: &str = "Version Packages";
/// Conventional so dogfood `cog check` accepts the version commit.
const DEFAULT_COMMIT: &str = "chore(release): version packages";

#[derive(Debug, Args)]
pub(super) struct CiArgs {
    #[command(subcommand)]
    command: CiCommand,
}

#[derive(Debug, Subcommand)]
enum CiCommand {
    /// Create or update the version pull request.
    VersionPr(VersionArgs),
    /// Post the contributor-PR plan comment and job summary.
    PrStatus(PrStatusArgs),
}

#[derive(Debug, Args)]
struct PrStatusArgs {
    /// Git ref to scan from (exclusive). Same default as `check` / `status`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
    /// Write the sticky-comment body to DIR instead of posting it to GitHub.
    /// Escape hatch for fork PRs that use a trusted `workflow_run` job to post
    /// (ADR-0015); not the default path.
    #[arg(long, value_name = "DIR")]
    emit_comment: Option<PathBuf>,
}

pub(super) fn run(args: &CiArgs) -> Result<(), CliError> {
    match &args.command {
        CiCommand::VersionPr(args) => run_version_pr(args),
        CiCommand::PrStatus(args) => run_pr_status(args),
    }
}

fn run_pr_status(args: &PrStatusArgs) -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    let config = load_config(&repo).map_err(CliError::from_boxed)?;
    let channels = config.pr_status();
    let emit = args.emit_comment.as_deref();
    // Emit mode never touches GitHub on this run — including stale-comment
    // cleanup — so a fork's untrusted job cannot write with a read-only token.
    if channels == PrStatus::None {
        if emit.is_some() {
            return Err(CliError::new(
                "pr-status=none refuses --emit-comment; set pr-status to comment, summary, or both, or drop the flag",
            ));
        }
        clear_stale_comment(&repo);
        return Ok(());
    }
    // The version PR already carries the release plan in its body; bump files
    // were consumed to produce it. Coverage comments are for contributor PRs.
    if on_version_packages_branch() {
        if let Some(dir) = emit {
            clear_emitted_comment(dir)?;
        } else if matches!(channels, PrStatus::Comment | PrStatus::Both) {
            clear_stale_comment(&repo);
        }
        return Ok(());
    }
    let state = pr_status_state(&repo, args.from.as_deref())?;
    let want_comment = matches!(channels, PrStatus::Comment | PrStatus::Both);
    let want_summary = matches!(channels, PrStatus::Summary | PrStatus::Both);
    if !has_opinion(&state) {
        if let Some(dir) = emit {
            // Same lifecycle as clear_stale_comment: a reused artifact dir must
            // not upload yesterday's plan when this run has nothing to say.
            clear_emitted_comment(dir)?;
        } else if want_comment {
            clear_stale_comment(&repo);
        }
        return Ok(());
    }
    let comment = status::render_comment(&state);
    let summary = status::render_summary(&state);
    if want_summary {
        write_step_summary(&summary)?;
    }
    if let Some(dir) = emit {
        return emit_comment_file(dir, &comment);
    }
    if !want_comment {
        return Ok(());
    }
    match post_pr_comment(&repo, &comment) {
        Ok(()) => Ok(()),
        Err(err) if github_forbidden(&err) => {
            degrade_to_summary(
                "comment requested but this run has no write permission (fork pull request); wrote the plan to the job summary instead.",
                want_summary,
                &summary,
            )
        }
        Err(err) if missing_comment_token(&err) => {
            degrade_to_summary(
                "comment requested but GITHUB_TOKEN is unset; wrote the plan to the job summary instead.",
                want_summary,
                &summary,
            )
        }
        Err(err) if missing_pull_number(&err) => {
            degrade_to_summary(
                "comment requested but this run is not a pull request; wrote the plan to the job summary instead.",
                want_summary,
                &summary,
            )
        }
        Err(err) => degrade_to_summary(
            &format!(
                "comment requested but GitHub did not accept the comment ({err}); wrote the plan to the job summary instead."
            ),
            want_summary,
            &summary,
        ),
    }
}

fn degrade_to_summary(
    message: &str,
    summary_already_written: bool,
    summary: &str,
) -> Result<(), CliError> {
    eprintln!("{message}");
    if !summary_already_written {
        write_step_summary(summary)?;
    }
    Ok(())
}

/// Stable name so a trusted `workflow_run` job can find the artifact without
/// parsing the untrusted job's logs.
const EMITTED_COMMENT_FILE: &str = "oakum-pr-comment.md";

fn emit_comment_file(dir: &Path, comment: &str) -> Result<(), CliError> {
    std::fs::create_dir_all(dir).map_err(|err| {
        CliError::new(format!(
            "failed to create --emit-comment directory {}: {err}",
            dir.display()
        ))
    })?;
    let path = dir.join(EMITTED_COMMENT_FILE);
    let mut body = comment.to_owned();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|err| {
        CliError::new(format!(
            "failed to write --emit-comment file {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn clear_emitted_comment(dir: &Path) -> Result<(), CliError> {
    let path = dir.join(EMITTED_COMMENT_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CliError::new(format!(
            "failed to remove stale --emit-comment file {}: {err}",
            path.display()
        ))),
    }
}

fn pr_status_state(
    repo: &repository::Repository,
    from: Option<&str>,
) -> Result<ReleaseState, CliError> {
    let config = load_config(repo).map_err(CliError::from_boxed)?;
    let workspace = status::apply_package_overrides(
        &add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?,
        &config,
    )
    .map_err(CliError::from_boxed)?;
    config.validate_workspace_selection(&workspace)?;
    let git = Git::at(repo.path());
    let files = load_plan_bump_files(&git, repo.path(), &workspace, &config, from)
        .map_err(CliError::from_boxed)?;
    let uncovered = coverage::uncovered_packages(&git, &workspace, &files, from, |package| {
        config.version_managed(package)
    })?;
    let intent = aggregate(files);
    let mut plan = compose(
        &workspace,
        &intent,
        |id| config.versioning_for(&id.name),
        CascadeAs::Patch,
        |_, dep| Some(dep.range.clone()),
        |id| {
            workspace
                .get(id)
                .expect("compose only asks for workspace packages")
                .version()
                .clone()
        },
    )
    .map_err(|err| CliError::new(err.to_string()))?;
    status::apply_version_selection(&config, &workspace, &mut plan)?;
    Ok(ReleaseState::from_plan(
        &plan,
        uncovered,
        RenderTarget::Comment,
    ))
}

fn has_opinion(state: &ReleaseState) -> bool {
    !state.packages().is_empty() || !state.uncovered().is_empty()
}

fn write_step_summary(text: &str) -> Result<(), CliError> {
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") {
        let path = path.trim();
        if !path.is_empty() {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|err| {
                    CliError::new(format!("failed to write GITHUB_STEP_SUMMARY: {err}"))
                })?;
            file.write_all(text.as_bytes()).map_err(|err| {
                CliError::new(format!("failed to write GITHUB_STEP_SUMMARY: {err}"))
            })?;
            if !text.ends_with('\n') {
                file.write_all(b"\n").map_err(|err| {
                    CliError::new(format!("failed to write GITHUB_STEP_SUMMARY: {err}"))
                })?;
            }
            return Ok(());
        }
    }
    print!("{text}");
    Ok(())
}

fn post_pr_comment(repo: &repository::Repository, body: &str) -> Result<(), CliError> {
    let token = actions_token().ok_or(CliError::MissingActionsToken)?;
    let number = pull_number().ok_or(CliError::MissingPullNumber)?;
    let git = Git::at(repo.path());
    let (owner, name) = repository_slug(&git)?;
    let client = github::Client::new(token).map_err(CliError::from)?;
    client
        .upsert_plan_comment(&owner, &name, number, status::PR_PLAN_MARKER, body)
        .map_err(CliError::from)?;
    Ok(())
}

fn actions_token() -> Option<String> {
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) if !token.is_empty() => Some(token),
        _ => None,
    }
}

fn pull_number() -> Option<u64> {
    if let Ok(path) = std::env::var("GITHUB_EVENT_PATH") {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(number) = pull_number_from_event(&value) {
                    return Some(number);
                }
            }
        }
    }
    pull_number_from_ref(std::env::var("GITHUB_REF").ok().as_deref())
}

/// True when this Actions run is the version-packages PR.
fn on_version_packages_branch() -> bool {
    if std::env::var("GITHUB_HEAD_REF").ok().as_deref() == Some(VERSION_BRANCH) {
        return true;
    }
    let Ok(path) = std::env::var("GITHUB_EVENT_PATH") else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    value
        .pointer("/pull_request/head/ref")
        .and_then(Value::as_str)
        == Some(VERSION_BRANCH)
}

fn pull_number_from_event(value: &Value) -> Option<u64> {
    if let Some(number) = value
        .pointer("/pull_request/number")
        .and_then(Value::as_u64)
    {
        return Some(number);
    }
    if value
        .pointer("/issue/pull_request")
        .is_some_and(|value| !value.is_null())
    {
        return value.pointer("/issue/number").and_then(Value::as_u64);
    }
    None
}

fn pull_number_from_ref(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    let mut parts = value.split('/');
    if parts.next()? != "refs" || parts.next()? != "pull" {
        return None;
    }
    parts.next()?.parse().ok()
}

fn clear_stale_comment(repo: &repository::Repository) {
    match delete_pr_comment(repo) {
        Ok(()) => {}
        Err(err)
            if github_forbidden(&err)
                || missing_comment_token(&err)
                || missing_pull_number(&err) => {}
        Err(err) => {
            eprintln!("could not remove a leftover plan comment ({err})");
        }
    }
}

fn delete_pr_comment(repo: &repository::Repository) -> Result<(), CliError> {
    let token = actions_token().ok_or(CliError::MissingActionsToken)?;
    let number = pull_number().ok_or(CliError::MissingPullNumber)?;
    let git = Git::at(repo.path());
    let (owner, name) = repository_slug(&git)?;
    let client = github::Client::new(token).map_err(CliError::from)?;
    client
        .delete_plan_comments(&owner, &name, number, status::PR_PLAN_MARKER)
        .map_err(CliError::from)?;
    Ok(())
}

fn github_forbidden(err: &CliError) -> bool {
    matches!(err, CliError::Forbidden { .. })
}

fn missing_comment_token(err: &CliError) -> bool {
    matches!(err, CliError::MissingActionsToken)
}

fn missing_pull_number(err: &CliError) -> bool {
    matches!(err, CliError::MissingPullNumber)
}

fn run_version_pr(args: &VersionArgs) -> Result<(), CliError> {
    let prepared = version::plan_writes(args).map_err(CliError::from_boxed)?;
    if !prepared.needs_github() {
        println!("nothing to version");
        return Ok(());
    }
    let client =
        github::Client::new(github::token().ok_or_else(|| {
            CliError::new("`oakum ci version-pr` needs GITHUB_TOKEN or GH_TOKEN")
        })?)?;
    let git = Git::at(&prepared.repo_path);
    let (owner, name) = repository_slug(&git)?;
    let default_branch = client.default_branch(&owner, &name)?;
    let base_oid = match client.branch_head(&owner, &name, &default_branch)? {
        Look::Found(oid) => oid,
        Look::Empty => {
            return Err(CliError::new(format!(
                "default branch `{default_branch}` has no head"
            )));
        }
    };
    let head = local_head(&git)?;
    if head != base_oid {
        return Err(CliError::new(format!(
            "checkout HEAD `{head}` is not `{default_branch}` at `{base_oid}`"
        )));
    }
    let additions = github_additions(&prepared)?;
    let deletions = github_deletions(&prepared)?;
    let headline = commit_headline(&prepared)?;
    let title = pr_title(&prepared)?;
    let body = pr_body(&prepared);
    let existing = version_pull(&client, &owner, &name)?;
    client.replace_branch_commit(
        &owner,
        &name,
        VERSION_BRANCH,
        &base_oid,
        &headline,
        FileChanges {
            additions: &additions,
            deletions: &deletions,
        },
    )?;
    let html_url = if let Some(pull) = existing {
        client.update_pull(&owner, &name, pull.number, &title, &body)?;
        pull.html_url
    } else {
        client
            .create_pull(
                &owner,
                &name,
                VERSION_BRANCH,
                &default_branch,
                &title,
                &body,
            )?
            .html_url
    };
    println!("{html_url}");
    Ok(())
}

fn local_head(git: &Git) -> Result<String, CliError> {
    let sha = git.text(Op::Head).map_err(|err| {
        CliError::new(format!(
            "`oakum ci version-pr` needs a git HEAD to compare with the default branch ({err})"
        ))
    })?;
    if sha.is_empty() {
        return Err(CliError::new("git HEAD is empty"));
    }
    Ok(sha)
}

fn version_pull(
    client: &github::Client,
    owner: &str,
    name: &str,
) -> Result<Option<github::PullRequest>, CliError> {
    match client.open_pulls_for_head(owner, name, VERSION_BRANCH)? {
        Look::Found(pulls) if pulls.len() > 1 => Err(CliError::new(format!(
            "multiple open version pull requests on `{VERSION_BRANCH}` ({})",
            pulls.len()
        ))),
        Look::Found(pulls) if pulls.len() == 1 => {
            Ok(Some(pulls.into_iter().next().expect("one pull")))
        }
        Look::Found(_) | Look::Empty => {
            match client.pulls_for_head(owner, name, VERSION_BRANCH, "closed")? {
                Look::Found(pulls) => {
                    let unmerged: Vec<_> = pulls.into_iter().filter(|pull| !pull.merged).collect();
                    match unmerged.len() {
                        0 => Ok(None),
                        1 => Ok(Some(unmerged.into_iter().next().expect("one pull"))),
                        count => Err(CliError::new(format!(
                            "multiple closed unmerged version pull requests on `{VERSION_BRANCH}` ({count})"
                        ))),
                    }
                }
                Look::Empty => Ok(None),
            }
        }
    }
}

pub(super) fn repository_slug(git: &Git) -> Result<(String, String), CliError> {
    repository_slug_from(git, "origin")
}

pub(super) fn repository_slug_from(git: &Git, remote: &str) -> Result<(String, String), CliError> {
    if let Ok(value) = std::env::var("GITHUB_REPOSITORY") {
        let value = value.trim();
        if !value.is_empty() {
            return parse_slug(value).ok_or_else(|| {
                CliError::new(format!("GITHUB_REPOSITORY `{value}` is not owner/repo"))
            });
        }
    }
    let url = git.text(Op::RemoteUrl { remote }).map_err(|err| {
        CliError::new(format!(
            "needs GITHUB_REPOSITORY or a git `{remote}` remote ({err})"
        ))
    })?;
    parse_github_origin(&url).ok_or_else(|| {
        CliError::new(format!(
            "git `{remote}` `{url}` is not a github.com owner/repo URL"
        ))
    })
}

fn parse_slug(value: &str) -> Option<(String, String)> {
    let (owner, name) = value.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_owned(), name.to_owned()))
}

fn parse_github_origin(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("git://github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    parse_slug(rest)
}

fn github_path(path: &Path) -> Result<String, CliError> {
    let raw = path
        .to_str()
        .ok_or_else(|| CliError::new("a version write path is not valid UTF-8"))?;
    github::git_path(raw, "write").map_err(CliError::from)
}

fn github_additions(prepared: &VersionWritePlan) -> Result<Vec<FileAddition>, CliError> {
    let mut additions = Vec::new();
    for write in &prepared.writes {
        if write.original() == write.next() {
            continue;
        }
        additions.push(FileAddition::from_text(
            github_path(write.path())?,
            write.next(),
        ));
    }
    Ok(additions)
}

fn github_deletions(prepared: &VersionWritePlan) -> Result<Vec<FileDeletion>, CliError> {
    prepared
        .deletes
        .iter()
        .map(|delete| FileDeletion::new(github_path(delete.path())?).map_err(CliError::from))
        .collect()
}

fn commit_headline(prepared: &VersionWritePlan) -> Result<String, CliError> {
    render_pref(
        &prepared.repo_path,
        "commit-message",
        prepared.commit_message.as_ref(),
        DEFAULT_COMMIT,
        prepared,
    )
}

fn pr_title(prepared: &VersionWritePlan) -> Result<String, CliError> {
    render_pref(
        &prepared.repo_path,
        "title",
        prepared.title.as_ref(),
        DEFAULT_TITLE,
        prepared,
    )
}

fn render_pref(
    repo: &Path,
    name: &str,
    source: Option<&oakum::template::TemplateSource>,
    default: &str,
    prepared: &VersionWritePlan,
) -> Result<String, CliError> {
    let Some(source) = source else {
        return Ok(default.to_owned());
    };
    let dir = cap_std::fs::Dir::open_ambient_dir(repo, cap_std::ambient_authority())
        .map_err(|err| CliError::new(err.to_string()))?;
    let body = load_template_body(&dir, repo, source).map_err(CliError::from_boxed)?;
    let state = ReleaseState::from_plan(&prepared.plan, [], RenderTarget::Status);
    let rendered = oakum::template::render(name, &body, &state)
        .map_err(|err| CliError::new(err.to_string()))?;
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return Err(CliError::new(format!(
            "{name} template rendered an empty string"
        )));
    }
    Ok(rendered.to_owned())
}

fn pr_body(prepared: &VersionWritePlan) -> String {
    let state = ReleaseState::from_plan(&prepared.plan, [], RenderTarget::Status);
    let mut body = status::render_summary(&state);
    if !body.ends_with('\n') {
        body.push('\n');
    }
    let _ = write!(body, "\nGenerated by oakum {}.\n", prepared.tool_version);
    body
}

#[cfg(test)]
mod tests {
    use super::{
        github_path, parse_github_origin, parse_slug, pr_body, pull_number_from_event,
        pull_number_from_ref, VersionWritePlan,
    };
    use crate::cli::write_set::{PlannedDelete, PlannedWrite};
    use oakum::plan::Plan;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn slug_rejects_extra_segments() {
        assert!(parse_slug("oakoss/oakum/extra").is_none());
        assert_eq!(
            parse_slug("oakoss/oakum"),
            Some((String::from("oakoss"), String::from("oakum")))
        );
    }

    #[test]
    fn origin_urls_resolve_owner_and_repo() {
        for url in [
            "git@github.com:oakoss/oakum.git",
            "https://github.com/oakoss/oakum.git",
            "https://github.com/oakoss/oakum",
            "ssh://git@github.com/oakoss/oakum.git",
            "git://github.com/oakoss/oakum.git",
        ] {
            assert_eq!(
                parse_github_origin(url),
                Some((String::from("oakoss"), String::from("oakum"))),
                "{url}"
            );
        }
        assert!(parse_github_origin("git@gitlab.com:oakoss/oakum.git").is_none());
    }

    #[test]
    fn pr_body_stamps_the_tool_version() {
        let prepared = VersionWritePlan {
            repo_path: std::path::PathBuf::from("."),
            writes: Vec::new(),
            deletes: Vec::new(),
            plan: Plan::default(),
            tool_version: String::from("0.0.0"),
            title: None,
            commit_message: None,
        };
        let body = pr_body(&prepared);
        assert!(body.contains("No packages planned."), "{body}");
        assert!(body.contains("Generated by oakum 0.0.0.\n"), "{body}");
    }

    #[test]
    fn needs_github_is_true_for_a_write_without_deletes() {
        let prepared = VersionWritePlan {
            repo_path: PathBuf::from("."),
            writes: vec![PlannedWrite::new(
                PathBuf::from("Cargo.toml"),
                "0.1.0",
                "0.1.1",
            )],
            deletes: Vec::new(),
            plan: Plan::default(),
            tool_version: String::from("0.0.0"),
            title: None,
            commit_message: None,
        };
        assert!(prepared.needs_github());
    }

    #[test]
    fn needs_github_is_true_for_a_delete_without_writes() {
        let prepared = VersionWritePlan {
            repo_path: PathBuf::from("."),
            writes: Vec::new(),
            deletes: vec![PlannedDelete::new(
                PathBuf::from(".changeset/one.md"),
                "---\n",
            )],
            plan: Plan::default(),
            tool_version: String::from("0.0.0"),
            title: None,
            commit_message: None,
        };
        assert!(prepared.needs_github());
    }

    #[test]
    fn github_path_normalizes_backslashes() {
        assert_eq!(
            github_path(std::path::Path::new("foo\\bar.md")).expect("path"),
            "foo/bar.md"
        );
    }

    #[test]
    fn pull_number_parses_actions_ref() {
        assert_eq!(pull_number_from_ref(Some("refs/pull/12/merge")), Some(12));
        assert_eq!(pull_number_from_ref(Some("refs/heads/main")), None);
        assert_eq!(pull_number_from_ref(None), None);
    }

    #[test]
    fn pull_number_from_event_accepts_pr_shapes_only() {
        assert_eq!(
            pull_number_from_event(&json!({"pull_request":{"number":4}})),
            Some(4)
        );
        assert_eq!(
            pull_number_from_event(&json!({"issue":{"number":4,"pull_request":{}}})),
            Some(4)
        );
        assert_eq!(pull_number_from_event(&json!({"issue":{"number":4}})), None);
        assert_eq!(
            pull_number_from_event(&json!({"issue":{"number":4,"pull_request":null}})),
            None
        );
    }
}
