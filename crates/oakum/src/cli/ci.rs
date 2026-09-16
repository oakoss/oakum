//! `oakum ci`: GitHub writes for CI. `version-pr` opens or updates the version
//! PR. `pr-status` posts the contributor-PR comment and job summary.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use oakum::config::PrStatus;
use oakum::state::{CoverageOutcome, ReleaseState, RenderTarget};
use serde_json::Value;

use super::config::{load_config, LoadedConfig};
use super::git::{Git, Op};
use super::github::{self, FileAddition, FileChanges, FileDeletion, Look};
use super::release_state;
use super::render;
use super::repository;
use super::template::load_template_body;
use super::version::{self, VersionArgs, VersionWritePlan};
use super::CliError;
use super::{deliver_block, deliver_out, say_err, say_out};

pub(super) const VERSION_BRANCH: &str = "oakum/version-packages";
const DEFAULT_TITLE: &str = "Version Packages";
/// Conventional so dogfood `cog check` accepts the version commit.
/// `pub(super)` so migrate's "equal to what `ci version-pr` writes anyway" skip
/// compares against this literal rather than a copy that could drift.
pub(super) const DEFAULT_COMMIT: &str = "chore(release): version packages";

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
    if config.is_default() {
        say_err(super::config::DEFAULTS_NOTE);
    }
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
    let state = pr_status_state(&repo, &config, args.from.as_deref())?;
    let want_comment = matches!(channels, PrStatus::Comment | PrStatus::Both);
    let want_summary = matches!(channels, PrStatus::Summary | PrStatus::Both);
    let Some(comment) = render::render_comment(&state) else {
        if let Some(dir) = emit {
            // Same lifecycle as clear_stale_comment: a reused artifact dir must
            // not upload yesterday's plan when this run has nothing to say.
            clear_emitted_comment(dir)?;
        } else if want_comment {
            clear_stale_comment(&repo);
        }
        return Ok(());
    };
    let summary = render::render_summary(&state);
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
                "comment requested but no pull request number could be read from GITHUB_EVENT_PATH or GITHUB_REF; wrote the plan to the job summary instead.",
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
    say_err(message);
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
    config: &LoadedConfig,
    from: Option<&str>,
) -> Result<ReleaseState, CliError> {
    // Required, not reported: a comment is a claim about the pull request, and
    // one built on a look that did not happen would be worse than none.
    release_state::release_state(
        repo,
        config,
        from,
        RenderTarget::Comment,
        release_state::CoverageMode::Required,
    )
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
    deliver_block(text).map_err(|err| CliError::undelivered("the plan", &err))?;
    Ok(())
}

fn post_pr_comment(repo: &repository::Repository, body: &str) -> Result<(), CliError> {
    let token = actions_token().ok_or(CliError::MissingActionsToken)?;
    let number = pull_number().ok_or(CliError::MissingPullNumber)?;
    let git = Git::at_repository(repo).map_err(CliError::from_boxed)?;
    let (owner, name) = repository_slug(&git)?;
    let client = github::Client::new(token).map_err(CliError::from)?;
    client
        .upsert_plan_comment(&owner, &name, number, render::PR_PLAN_MARKER, body)
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

/// True when this Actions run is the bot's own version-packages PR. A fork or a
/// collaborator can use the branch name, so the payload must also say the head
/// repository is this one and that both the author and the sender are bots: the
/// four terms the scaffolded workflow tests. Anything short of that is "not the
/// version PR", because a wrong answer that way costs a redundant coverage
/// comment, and the other way costs a contributor's unreported coverage gap.
fn on_version_packages_branch() -> bool {
    let head = std::env::var("GITHUB_HEAD_REF").ok();
    if head.as_deref().is_some_and(|head| head != VERSION_BRANCH) {
        return false;
    }
    let Ok(path) = std::env::var("GITHUB_EVENT_PATH") else {
        if head.is_some() {
            say_err("GITHUB_EVENT_PATH is not set, so the version pull request cannot be recognised; treating this run as a contributor pull request");
        }
        return false;
    };
    // Still "not the version PR"; the line only makes that cost diagnosable.
    let value = match std::fs::read(&path)
        .map_err(|err| err.to_string())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|err| err.to_string()))
    {
        Ok(value) => value,
        Err(err) => {
            say_err(&format!(
                "event payload at {path} could not be read ({err}); treating this run as a contributor pull request"
            ));
            return false;
        }
    };
    let text = |pointer: &str| value.pointer(pointer).and_then(Value::as_str);
    if text("/pull_request/head/ref") != Some(VERSION_BRANCH) {
        return false;
    }
    let Ok(this_repo) = std::env::var("GITHUB_REPOSITORY") else {
        say_err("GITHUB_REPOSITORY is not set, so the version pull request cannot be recognised; treating this run as a contributor pull request");
        return false;
    };
    text("/pull_request/head/repo/full_name") == Some(this_repo.as_str())
        && text("/pull_request/user/type") == Some("Bot")
        && text("/sender/type") == Some("Bot")
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
            say_err(&format!("could not remove a leftover plan comment ({err})"));
        }
    }
}

fn delete_pr_comment(repo: &repository::Repository) -> Result<(), CliError> {
    let token = actions_token().ok_or(CliError::MissingActionsToken)?;
    let number = pull_number().ok_or(CliError::MissingPullNumber)?;
    let git = Git::at_repository(repo).map_err(CliError::from_boxed)?;
    let (owner, name) = repository_slug(&git)?;
    let client = github::Client::new(token).map_err(CliError::from)?;
    client
        .delete_plan_comments(&owner, &name, number, render::PR_PLAN_MARKER)
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
        say_out("nothing to version");
        return Ok(());
    }
    let client =
        github::Client::new(github::token().ok_or_else(|| {
            CliError::new("`oakum ci version-pr` needs GITHUB_TOKEN or GH_TOKEN")
        })?)?;
    let git = Git::at_repository(&prepared.repo).map_err(CliError::from_boxed)?;
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
    let opened = if let Some(pull) = existing {
        client.update_pull(&owner, &name, pull.number, &title, &body)?
    } else {
        client.create_pull(
            &owner,
            &name,
            VERSION_BRANCH,
            &default_branch,
            &title,
            &body,
        )?
    };
    deliver_out(&opened.html_url).map_err(|err| {
        CliError::undelivered(
            format!(
                "version pull request {} is open, but its URL",
                opened.html_url
            ),
            &err,
        )
    })?;
    // stderr: stdout is the URL a caller captures, and a diagnostic that
    // lands there turns `URL=$(oakum ci version-pr)` into two lines.
    say_err(&author_note(opened.author.as_ref()));
    Ok(())
}

/// Says who the token opened the pull request as, where that was measured,
/// rather than leaving it to be inferred from a workflow that did or did not
/// fire. A personal token opens it as a person and every check then runs on the
/// version pull request, whose only other symptom is a longer bill.
fn author_note(author: Option<&github::PullAuthor>) -> String {
    let Some(author) = author else {
        return String::from(
            "version pull request author: not reported by GitHub; \
             whether the scaffolded `oakum check --strict` skip applies is unverified",
        );
    };
    // Only a named non-bot settles it: that term alone defeats the skip. A bot
    // does not, because the skip also tests the sender of the event this push
    // raises, which no run can observe from here.
    let consequence = match author.kind.as_deref() {
        Some("Bot") => "the scaffolded `oakum check --strict` skip also tests the event sender, \
                        which this run cannot observe",
        Some(_) => {
            "the scaffolded `oakum check --strict` skip does not apply, so it runs on this pull request"
        }
        None => "GitHub reported no author type, so whether the scaffolded \
                 `oakum check --strict` skip applies is unverified",
    };
    format!(
        "version pull request author: {} ({}); {consequence}",
        author.login.as_deref().unwrap_or("login not reported"),
        author.kind.as_deref().unwrap_or("type not reported")
    )
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
        &prepared.repo,
        "commit-message",
        prepared.commit_message.as_ref(),
        DEFAULT_COMMIT,
        prepared,
    )
}

fn pr_title(prepared: &VersionWritePlan) -> Result<String, CliError> {
    render_pref(
        &prepared.repo,
        "title",
        prepared.title.as_ref(),
        DEFAULT_TITLE,
        prepared,
    )
}

fn render_pref(
    repo: &repository::Repository,
    name: &str,
    source: Option<&oakum::template::TemplateSource>,
    default: &str,
    prepared: &VersionWritePlan,
) -> Result<String, CliError> {
    let Some(source) = source else {
        return Ok(default.to_owned());
    };
    let body = load_template_body(repo.dir(), repo.path(), source).map_err(CliError::from_boxed)?;
    let state = ReleaseState::from_plan(
        &prepared.plan,
        CoverageOutcome::NotAsked,
        RenderTarget::Status,
    );
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
    let state = ReleaseState::from_plan(
        &prepared.plan,
        CoverageOutcome::NotAsked,
        RenderTarget::Status,
    );
    let mut body = render::render_summary(&state);
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
    use crate::cli::repository;
    use crate::cli::write_set::{PlannedDelete, PlannedWrite};
    use oakum::plan::Plan;
    use oakum::state::{CoverageOutcome, ReleaseState, RenderTarget};
    use serde_json::json;
    use std::path::PathBuf;

    fn stub_plan(writes: Vec<PlannedWrite>, deletes: Vec<PlannedDelete>) -> VersionWritePlan {
        VersionWritePlan {
            repo: repository::discover().expect("test checkout is a git repository"),
            writes,
            deletes,
            plan: Plan::default(),
            tool_version: String::from("0.0.0"),
            title: None,
            commit_message: None,
        }
    }

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
    fn author_note_settles_only_what_the_author_decides() {
        fn note(login: Option<&str>, kind: Option<&str>) -> String {
            super::author_note(Some(&super::github::PullAuthor {
                login: login.map(str::to_owned),
                kind: kind.map(str::to_owned),
            }))
        }
        assert!(super::author_note(None).contains("not reported by GitHub"));
        assert!(super::author_note(None).contains("unverified"));
        // A person defeats the skip on its own; a bot leaves the sender open.
        let person = note(Some("jbabin91"), Some("User"));
        assert!(person.contains("jbabin91 (User)"), "{person}");
        assert!(person.contains("does not apply"), "{person}");
        let bot = note(Some("oakoss[bot]"), Some("Bot"));
        assert!(bot.contains("oakoss[bot] (Bot)"), "{bot}");
        assert!(bot.contains("also tests the event sender"), "{bot}");
        assert!(!bot.contains("does not apply"), "{bot}");
        let no_kind = note(Some("oakum-bot"), None);
        assert!(
            no_kind.contains("oakum-bot (type not reported)"),
            "{no_kind}"
        );
        assert!(no_kind.contains("unverified"), "{no_kind}");
        assert!(!no_kind.contains("does not apply"), "{no_kind}");
        let no_login = note(None, Some("Bot"));
        assert!(no_login.contains("login not reported (Bot)"), "{no_login}");
    }

    #[test]
    fn pr_body_stamps_the_tool_version() {
        let prepared = stub_plan(Vec::new(), Vec::new());
        let body = pr_body(&prepared);
        assert!(body.contains("No packages planned."), "{body}");
        assert!(body.contains("Generated by oakum 0.0.0.\n"), "{body}");
    }

    #[test]
    fn needs_github_is_true_for_a_write_without_deletes() {
        let prepared = stub_plan(
            vec![PlannedWrite::new(
                PathBuf::from("Cargo.toml"),
                "0.1.0",
                "0.1.1",
            )],
            Vec::new(),
        );
        assert!(prepared.needs_github());
    }

    #[test]
    fn needs_github_is_true_for_a_delete_without_writes() {
        let prepared = stub_plan(
            Vec::new(),
            vec![PlannedDelete::new(
                PathBuf::from(".changeset/one.md"),
                "---\n",
            )],
        );
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

    /// `commit-message` and `title` render with the `ReleaseState` document,
    /// so a field added there is a variable those surfaces gain. Their schema
    /// descriptions name the set, and this is what fails when it drifts.
    #[test]
    fn the_schema_names_every_variable_the_state_surfaces_render_with() {
        for surface in ["commit-message", "title"] {
            crate::cli::schema_names_every_variable(
                surface,
                ReleaseState::from_plan(
                    &oakum::plan::Plan::default(),
                    CoverageOutcome::NotAsked,
                    RenderTarget::Status,
                ),
            );
        }
    }
}
