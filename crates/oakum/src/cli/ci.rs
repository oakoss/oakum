//! `oakum ci`: GitHub writes for CI. `version-pr` opens or updates the version
//! PR. `pr-status` posts the contributor-PR comment and job summary.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use oakum::config::PrStatus;
use oakum::state::{ReleaseState, RenderTarget};
use serde_json::Value;

use super::changelog::{self, ReleaseNote};
use super::config::{load_config, LoadedConfig};
use super::git::{Git, Op};
use super::github::{self, FileAddition, FileChanges, FileDeletion, Look};
use super::release_state;
use super::render;
use super::repository;
use super::template::load_template_body;
use super::version::{self, VersionArgs, VersionWritePlan};
use super::CliError;
use super::{deliver_out, deliver_report, say_err, say_out};

pub(super) const VERSION_BRANCH: &str = "oakum/version-packages";
/// The default commit subject and the default title, which are one sentence:
/// where a repository squashes with `squash_merge_commit_title = PR_TITLE`, the
/// title is the commit a default-branch `cog check` reads. A configured
/// `commit-message` moves only the commit; `title` is its own key.
const VERSION_HEADLINE: &str = "chore(release): version packages";
const DEFAULT_TITLE: &str = VERSION_HEADLINE;
/// `pub(super)` so migrate's "equal to what `ci version-pr` writes anyway" skip
/// compares against this literal rather than a copy that could drift.
pub(super) const DEFAULT_COMMIT: &str = VERSION_HEADLINE;

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
    /// Post, or remove, the comment `ci pr-status --emit-comment` wrote, from
    /// a trusted job with no checkout.
    PostComment(PostCommentArgs),
}

#[derive(Debug, Args)]
struct PrStatusArgs {
    #[arg(long, value_name = "REF", help = super::FROM_PLAN)]
    from: Option<String>,
    /// Write the sticky comment to DIR instead of posting it to GitHub, for a
    /// trusted job to post with `oakum ci post-comment DIR`: the
    /// body as `oakum-pr-comment.md`, or `oakum-pr-comment.none` when this
    /// run has no comment.
    #[arg(long, value_name = "DIR")]
    emit_comment: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PostCommentArgs {
    /// The directory `ci pr-status --emit-comment` wrote.
    #[arg(value_name = "DIR")]
    dir: PathBuf,
}

pub(super) fn run(args: &CiArgs) -> Result<(), CliError> {
    match &args.command {
        CiCommand::VersionPr(args) => run_version_pr(args),
        CiCommand::PrStatus(args) => run_pr_status(args),
        CiCommand::PostComment(args) => run_post_comment(args),
    }
}

fn run_pr_status(args: &PrStatusArgs) -> Result<(), CliError> {
    // Before anything that can fail: a run that stops leaves neither file, and
    // the poster refuses rather than posting an earlier run's comment.
    if let Some(dir) = args.emit_comment.as_deref() {
        reset_emitted(dir)?;
    }
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
        say_err(
            "pr-status is set to `none`, so no coverage comment and no job summary were written.",
        );
        // The trusted job that posts an emitted comment removes an earlier one
        // when there is none, as `clear_stale_comment` does without the flag.
        if let Some(dir) = emit {
            return emit_no_comment(dir);
        }
        clear_stale_comment(&repo);
        return Ok(());
    }
    // The version PR already carries the release plan in its body; bump files
    // were consumed to produce it. Coverage comments are for contributor PRs.
    if on_version_packages_branch() {
        say_err(&format!(
            "this is the version pull request, which carries the plan in its body, so {} written.",
            nothing_was(channels)
        ));
        if let Some(dir) = emit {
            emit_no_comment(dir)?;
        } else if matches!(channels, PrStatus::Comment | PrStatus::Both) {
            clear_stale_comment(&repo);
        }
        return Ok(());
    }
    let (state, next) = pr_status_state(&repo, &config, args.from.as_deref())?;
    let want_comment = matches!(channels, PrStatus::Comment | PrStatus::Both);
    let want_summary = matches!(channels, PrStatus::Summary | PrStatus::Both);
    let Some(comment) = render::render_comment(&state, Some(&next)) else {
        // Without this line an empty plan, a post nobody could see and a
        // swallowed failure are one silence from the workflow's side.
        say_err(&format!(
            "nothing to post: no package these commits touch plans a release or lacks covering intent, so {} written.",
            nothing_was(channels)
        ));
        if let Some(dir) = emit {
            // Same lifecycle as clear_stale_comment: a reused artifact dir must
            // not upload yesterday's plan when this run has nothing to say.
            emit_no_comment(dir)?;
        } else if want_comment {
            clear_stale_comment(&repo);
        }
        return Ok(());
    };
    let summary = render::render_summary(&state, Some(&next));
    if want_summary {
        write_step_summary(&summary)?;
    }
    if let Some(dir) = emit {
        if !want_comment {
            say_err(
                "pr-status is set to `summary`, so the plan went to the job summary and --emit-comment wrote no comment.",
            );
            return emit_no_comment(dir);
        }
        return emit_comment_file(dir, &comment);
    }
    if !want_comment {
        return Ok(());
    }
    match post_pr_comment(&repo, &comment) {
        Ok(None) => Ok(()),
        // The plan is up; only an older copy stayed, so nothing falls back.
        Ok(Some(left)) => {
            say_err(&left);
            Ok(())
        }
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

/// What a run that posts nothing skipped, named by the channels it was asked
/// for. Naming the comment alone told an operator who had configured `summary`
/// about a channel they never wanted, while the summary they did want went
/// unwritten and unmentioned.
const fn nothing_was(channels: PrStatus) -> &'static str {
    match channels {
        PrStatus::Comment => "no coverage comment was",
        PrStatus::Summary => "no job summary was",
        PrStatus::Both => "no coverage comment and no job summary were",
        // Unreachable: every caller sits below the `PrStatus::None` early
        // return, which has already said its own piece.
        PrStatus::None => "nothing was",
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

/// Stable names so a trusted job can find the artifact without parsing the
/// untrusted job's logs. A run writes exactly one: the body, or the marker
/// that it had none, so a missing artifact never reads as nothing to post.
const EMITTED_COMMENT_FILE: &str = "oakum-pr-comment.md";
const NO_COMMENT_FILE: &str = "oakum-pr-comment.none";
const NO_COMMENT_TEXT: &str = "oakum ci pr-status had no comment for this pull request\n";
/// Closes an emitted body, so a file cut short reads as cut short rather than
/// as a shorter comment; `post-comment` strips it before posting.
const EMITTED_END: &str = "<!-- /oakum:pr-plan -->\n";

fn emit_comment_file(dir: &Path, comment: &str) -> Result<(), CliError> {
    let mut body = comment.to_owned();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(EMITTED_END);
    write_emitted(dir, EMITTED_COMMENT_FILE, &body)
}

fn emit_no_comment(dir: &Path) -> Result<(), CliError> {
    write_emitted(dir, NO_COMMENT_FILE, NO_COMMENT_TEXT)
}

/// Written beside its name and renamed into place, so a run that stops leaves
/// the whole file or none of it.
fn write_emitted(dir: &Path, name: &str, text: &str) -> Result<(), CliError> {
    std::fs::create_dir_all(dir).map_err(|err| {
        CliError::new(format!(
            "failed to create --emit-comment directory {}: {err}",
            dir.display()
        ))
    })?;
    let path = dir.join(name);
    let staged = dir.join(format!(".{name}.oakum-write"));
    // Created fresh, never opened: a link or FIFO left at the staged name
    // would send the write elsewhere or block it.
    let created = || {
        match std::fs::remove_file(&staged) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        std::io::Write::write_all(&mut file, text.as_bytes())
    };
    created()
        .and_then(|()| std::fs::rename(&staged, &path))
        .map_err(|err| {
            let _ = std::fs::remove_file(&staged);
            CliError::new(format!(
                "failed to write --emit-comment file {}: {err}",
                path.display()
            ))
        })
}

fn reset_emitted(dir: &Path) -> Result<(), CliError> {
    for name in [EMITTED_COMMENT_FILE, NO_COMMENT_FILE] {
        let path = dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CliError::new(format!(
                    "failed to remove stale --emit-comment file {}: {err}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

/// GitHub's limit on an issue comment or pull request body, in characters.
const MAX_BODY_CHARS: usize = 65_536;

/// The trusted half of `--emit-comment`. Nothing from the artifact decides
/// where the token writes: the pull request comes from this job's own event,
/// and the body must be one oakum rendered whole, marker to end.
fn run_post_comment(args: &PostCommentArgs) -> Result<(), CliError> {
    let dir = &args.dir;
    let body_path = dir.join(EMITTED_COMMENT_FILE);
    let none_path = dir.join(NO_COMMENT_FILE);
    let body = read_emitted(&body_path)?;
    let none = match read_emitted(&none_path)? {
        Some(text) if text == NO_COMMENT_TEXT => true,
        Some(_) => {
            return Err(CliError::unverified(format!(
                "unverified: {} is not the marker `ci pr-status` writes; nothing was posted or removed",
                none_path.display()
            )));
        }
        None => false,
    };
    let body = match (body, none) {
        (Some(_), true) => {
            return Err(CliError::unverified(format!(
                "unverified: {} holds both {EMITTED_COMMENT_FILE} and {NO_COMMENT_FILE}, so which run it came from cannot be told; nothing was posted",
                dir.display()
            )));
        }
        (None, false) => {
            return Err(CliError::unverified(format!(
                "unverified: {} holds neither {EMITTED_COMMENT_FILE} nor {NO_COMMENT_FILE}, so the emitted comment may have been lost on the way; nothing was posted or removed",
                dir.display()
            )));
        }
        (body, _) => body,
    };
    let body = body
        .map(|text| emitted_body(&body_path, &text))
        .transpose()?;
    let token = actions_token().ok_or_else(|| {
        CliError::new("`oakum ci post-comment` needs GITHUB_TOKEN with pull-requests: write")
    })?;
    let number = post_pull_number().ok_or_else(|| {
        CliError::new(
            "`oakum ci post-comment` takes the pull request from its own event (a `pull_request`, or a `workflow_run` from a same-repository pull request) and found none; a fork's `workflow_run` names no pull request",
        )
    })?;
    let (owner, name) = env_repository_slug()?;
    let client = github::Client::new(token).map_err(CliError::from)?;
    let refused = |err: github::Error| {
        match CliError::from(err) {
        err if github_forbidden(&err) => CliError::unverified(format!(
            "unverified: this job's token cannot write to #{number} (a fork pull request, or a token without pull-requests: write); the comment was not posted or removed"
        )),
        err => err,
    }
    };
    if let Some(body) = body {
        let upserted = client
            .upsert_plan_comment(&owner, &name, number, render::PR_PLAN_MARKER, &body)
            .map_err(refused)?;
        upserted.cleanup.map_err(|err| {
            CliError::unverified(format!("unverified: {}", older_copies_left(number, &err)))
        })?;
        say_err(&format!("posted the plan comment on #{number}."));
    } else {
        client
            .delete_plan_comments(&owner, &name, number, render::PR_PLAN_MARKER)
            .map_err(refused)?;
        say_err(&format!(
            "the emitting run had no comment, so oakum's plan comment was removed from #{number} if it had one."
        ));
    }
    Ok(())
}

/// The comment an emitted body holds: oakum's marker first and its closing
/// line last, which is stripped. One without the marker is not oakum's; one
/// without the closing line was cut short, or never finished.
fn emitted_body(path: &Path, text: &str) -> Result<String, CliError> {
    if !text.is_empty() && !text.starts_with(render::PR_PLAN_MARKER) {
        return Err(CliError::new(format!(
            "{} does not start with oakum's `{}` marker, so it is not a comment oakum wrote; nothing was posted",
            path.display(),
            render::PR_PLAN_MARKER
        )));
    }
    let Some(body) = text.strip_suffix(EMITTED_END) else {
        return Err(CliError::unverified(format!(
            "unverified: {} does not end with `{}`, so it may have been cut short; nothing was posted",
            path.display(),
            EMITTED_END.trim_end()
        )));
    };
    let chars = body.chars().count();
    if chars > MAX_BODY_CHARS {
        return Err(CliError::new(format!(
            "{} is {chars} characters, over GitHub's {MAX_BODY_CHARS}-character comment limit; nothing was posted",
            path.display()
        )));
    }
    Ok(body.to_owned())
}

/// The file's text, or `None` when nothing is there. Anything but a regular
/// file is refused before it is opened: a symlink could lead outside DIR, and
/// a FIFO would block the job.
fn read_emitted(path: &Path) -> Result<Option<String>, CliError> {
    let unread =
        |why: String| CliError::unverified(format!("unverified: {}: {why}", path.display()));
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Err(unread(String::from(
                "not a regular file, so it was not read; nothing was posted or removed",
            )));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(unread(format!("could not be inspected: {err}"))),
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|err| unread(format!("could not be read: {err}")))
}

/// [`pull_number`], and a `workflow_run` naming exactly one pull request: the
/// trusted job a same-repository pull request's run starts. A fork's names
/// none, and two name no single one.
fn post_pull_number() -> Option<u64> {
    pull_number().or_else(|| {
        let bytes = std::fs::read(std::env::var("GITHUB_EVENT_PATH").ok()?).ok()?;
        let value = serde_json::from_slice::<Value>(&bytes).ok()?;
        workflow_run_pull_number(&value)
    })
}

fn workflow_run_pull_number(value: &Value) -> Option<u64> {
    match value
        .pointer("/workflow_run/pull_requests")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([only]) => only.pointer("/number").and_then(Value::as_u64),
        _ => None,
    }
}

/// `owner/name` from `GITHUB_REPOSITORY`: the poster has no checkout to read
/// a remote from.
fn env_repository_slug() -> Result<(String, String), CliError> {
    let value = std::env::var("GITHUB_REPOSITORY").unwrap_or_default();
    value
        .trim()
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .map(|(owner, name)| (owner.to_owned(), name.to_owned()))
        .ok_or_else(|| {
            CliError::new(format!(
                "GITHUB_REPOSITORY is `{value}`, not `owner/name`, so the pull request cannot be found"
            ))
        })
}

fn pr_status_state(
    repo: &repository::Repository,
    config: &LoadedConfig,
    from: Option<&str>,
) -> Result<(ReleaseState, render::NextStep), CliError> {
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
    deliver_report(text).map_err(|err| CliError::undelivered("the plan", &err))?;
    Ok(())
}

/// `Ok(Some(line))` when the plan went up but an older copy could not be
/// removed, kept apart from a plan that was not posted.
fn post_pr_comment(repo: &repository::Repository, body: &str) -> Result<Option<String>, CliError> {
    let token = actions_token().ok_or(CliError::MissingActionsToken)?;
    let number = pull_number().ok_or(CliError::MissingPullNumber)?;
    let git = Git::at_repository(repo).map_err(CliError::from_boxed)?;
    let (owner, name) = repository_slug(&git)?;
    let client = github::Client::new(token).map_err(CliError::from)?;
    let upserted = client
        .upsert_plan_comment(&owner, &name, number, render::PR_PLAN_MARKER, body)
        .map_err(CliError::from)?;
    Ok(upserted
        .cleanup
        .err()
        .map(|err| older_copies_left(number, &err)))
}

/// The plan is up but an older copy stayed: never read as "not posted", since
/// the write happened.
fn older_copies_left(number: u64, err: &github::Error) -> String {
    format!("posted the plan comment on #{number}, but removing its older copies failed: {err}")
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
        // Expected states, not failures — but the caller has already said what this
        // run did not write, and a reader takes that for "nothing was left
        // behind". One line covers all three: which of them it was changes
        // nothing a reader would do about it.
        Err(err)
            if github_forbidden(&err)
                || missing_comment_token(&err)
                || missing_pull_number(&err) =>
        {
            say_err(
                "a leftover plan comment could not be removed with this run's token, so an earlier plan may still be visible on the pull request",
            );
        }
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
    prepared.say_dropped_intent();
    let releases = prepared.needs_github();
    if !releases && prepared.deletes.is_empty() {
        say_out("nothing to version");
        return Ok(());
    }
    let client =
        github::Client::new(github::token().ok_or_else(|| {
            CliError::new("`oakum ci version-pr` needs GITHUB_TOKEN or GH_TOKEN")
        })?)?;
    let git = Git::at_repository(&prepared.repo).map_err(CliError::from_boxed)?;
    let (owner, name) = repository_slug(&git)?;
    // Deletes alone open nothing, but a version pull request an earlier plan
    // opened is still refreshed, or it goes on offering a release whose intent
    // left the default branch.
    // The pull request found here is the one refreshed: looking again later
    // could miss one merged in between and open a new one.
    let open_for_deletes = if releases {
        None
    } else {
        let Some(open) = open_version_pull(&client, &owner, &name)? else {
            say_out(&format!(
                "nothing to version: {} pending bump file(s) release nothing, so no pull request was opened; the next release consumes them",
                prepared.deletes.len()
            ));
            return Ok(());
        };
        Some(open)
    };
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
        // Only a tip that descends from HEAD is a newer push; anything else
        // would build the version PR from a commit off the branch.
        let why = match client.compare(&owner, &name, &head, &base_oid)? {
            Look::Found(github::Comparison::Ahead) => {
                say_out(&format!(
                    "superseded: `{default_branch}` moved on from checkout HEAD `{head}` \
                     to `{base_oid}`; this run writes nothing, and the version pull request \
                     waits for a run on `{base_oid}`"
                ));
                return Ok(());
            }
            Look::Found(github::Comparison::Behind) => "behind it",
            Look::Found(github::Comparison::Diverged) => "diverged from it",
            Look::Found(github::Comparison::Identical) => "identical by GitHub's compare",
            Look::Empty => "GitHub's compare returned 404",
        };
        return Err(CliError::new(format!(
            "checkout HEAD `{head}` is not `{default_branch}` at `{base_oid}` ({why})"
        )));
    }
    let additions = github_additions(&prepared)?;
    let deletions = github_deletions(&prepared)?;
    let headline = commit_headline(&prepared)?;
    let title = pr_title(&prepared)?;
    let body = pr_body(&prepared)?;
    let existing = match open_for_deletes {
        Some(open) => Some(open),
        None => version_pull(&client, &owner, &name)?,
    };
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
    prepared.say_landed_notes();
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
        err.recast(format!(
            "`oakum ci version-pr` needs a git HEAD to compare with the default branch ({})",
            err.detail()
        ))
    })?;
    if sha.is_empty() {
        return Err(CliError::new("git HEAD is empty"));
    }
    Ok(sha)
}

fn open_version_pull(
    client: &github::Client,
    owner: &str,
    name: &str,
) -> Result<Option<github::PullRequest>, CliError> {
    match client.open_pulls_for_head(owner, name, VERSION_BRANCH)? {
        Look::Found(pulls) if pulls.len() > 1 => Err(CliError::new(format!(
            "multiple open version pull requests on `{VERSION_BRANCH}` ({})",
            pulls.len()
        ))),
        Look::Found(pulls) => Ok(pulls.into_iter().next()),
        Look::Empty => Ok(None),
    }
}

fn version_pull(
    client: &github::Client,
    owner: &str,
    name: &str,
) -> Result<Option<github::PullRequest>, CliError> {
    match open_version_pull(client, owner, name)? {
        Some(open) => Ok(Some(open)),
        None => match client.pulls_for_head(owner, name, VERSION_BRANCH, "closed")? {
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
        },
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
        err.recast(format!(
            "needs GITHUB_REPOSITORY or a git `{remote}` remote ({})",
            err.detail()
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
    let state = prepared.release_state();
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

/// Checked before the branch moves: GitHub refuses the body only after the
/// commit is pushed, leaving a branch the pull request does not describe.
fn pr_body(prepared: &VersionWritePlan) -> Result<String, CliError> {
    let state = prepared.release_state();
    let mut head = render::render_summary(&state, None);
    if !head.ends_with('\n') {
        head.push('\n');
    }
    let tail = pr_body_tail(prepared);
    let length = github_len(&head) + github_len(&tail);
    if length > MAX_BODY_CHARS {
        return Err(CliError::new(format!(
            "the version pull request body is {length} characters before its release notes, over GitHub's {MAX_BODY_CHARS}-character limit; nothing was pushed"
        )));
    }
    let Some(notes) = release_notes_section(&prepared.release_notes, MAX_BODY_CHARS - length)
    else {
        return Err(CliError::new(format!(
            "the version pull request body is {length} characters before its release notes, leaving no room under GitHub's {MAX_BODY_CHARS}-character limit to name them; nothing was pushed"
        )));
    };
    Ok(head + &notes + &tail)
}

/// In UTF-16 units, where a character outside the BMP counts two: the
/// stricter reading of [`MAX_BODY_CHARS`].
fn github_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// One collapsible block per package. An entry that does not fit, reads back
/// empty, or holds raw HTML that could break the page is named with its file
/// instead, never cut mid-entry; when even those lines do not fit, one line
/// says the section was left out, and `None` means not even that fits.
fn release_notes_section(notes: &[ReleaseNote], budget: usize) -> Option<String> {
    const HEADING: &str = "\n### Release notes\n\n";
    const COMPACT: &str = "\n### Release notes\n\nLeft out for length; each package's entry is in its changelog on this branch.\n";
    if notes.is_empty() {
        return Some(String::new());
    }
    let open = if notes.len() == 1 { " open" } else { "" };
    let pointer = |note: &ReleaseNote, why: &str| {
        format!(
            "- {} {} ({}): {why}; see `{}` on this branch\n\n",
            note.package,
            note.version,
            changelog::ecosystem_label(note.ecosystem),
            note.path
        )
    };
    let rendered: Vec<(Option<String>, String)> = notes
        .iter()
        .map(|note| {
            if note.entry.trim().is_empty() {
                (None, pointer(note, "the entry is empty"))
            } else if holds_raw_html(&note.entry) {
                (None, pointer(note, "left out because it holds raw HTML"))
            } else {
                let block = format!(
                    "<details{open}>\n<summary>{} {} ({})</summary>\n\n{}\n\n</details>\n\n",
                    note.package,
                    note.version,
                    changelog::ecosystem_label(note.ecosystem),
                    note.entry
                );
                (Some(block), pointer(note, "left out for length"))
            }
        })
        .collect();
    // Every piece ends in a blank line, and the section keeps one newline of the last.
    let fits = |pieces: usize| pieces - 1 <= budget;
    let mut reserve: usize = rendered.iter().map(|(_, line)| github_len(line)).sum();
    if !fits(github_len(HEADING) + reserve) {
        return (github_len(COMPACT) <= budget).then(|| String::from(COMPACT));
    }
    let mut section = String::from(HEADING);
    for (block, line) in rendered {
        reserve -= github_len(&line);
        match block {
            Some(block) if fits(github_len(&section) + github_len(&block) + reserve) => {
                section.push_str(&block);
            }
            _ => section.push_str(&line),
        }
    }
    section.truncate(section.trim_end().len());
    section.push('\n');
    Some(section)
}

/// `CommonMark` opens an HTML block at a leading `<`, inside a list item or
/// blockquote too, and one left open swallows the blocks and footer after it.
/// A comment, processing instruction, or declaration (`<!`, `<?`) runs to its
/// own end marker, and a `<details>` tag closes or nests the wrapper, wherever
/// on the line they sit. Errs toward naming the file.
fn holds_raw_html(entry: &str) -> bool {
    // Code lines are checked too: where a fence or indented block ends inside
    // a list depends on container rules this does not track.
    let mut read_whole = false;
    entry.lines().any(|line| {
        if line.trim().is_empty() {
            read_whole = false;
        }
        // A code span renders its text literally, so a tag inside one is text.
        // A paragraph's next line can continue a span, link title or attribute
        // this one opened, so after one the lines up to the next blank are
        // read whole rather than guessed at.
        let line = if read_whole {
            line.to_owned()
        } else {
            let (text, continues) = outside_code_spans(line);
            read_whole = continues;
            text
        };
        let opens_block = after_container_markers(&line)
            .strip_prefix('<')
            .is_some_and(|tag| tag.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/'));
        let lower = line.to_ascii_lowercase();
        opens_block
            || ["<!", "<?", "<details", "</details"]
                .iter()
                .any(|marker| lower.contains(marker))
    })
}

/// `line` without its code spans, and whether the next line may continue
/// something it opened: an unclosed run, or a `<` or `](` outside a span. A
/// run closes at the next run of its length; an escaped backtick opens
/// nothing. After `<` or `](` a backtick may be in an autolink, attribute or
/// link, so the rest of the line is kept whole.
fn outside_code_spans(line: &str) -> (String, bool) {
    let mut out = String::new();
    let mut rest = line;
    let mut left_open = false;
    while let Some(start) = rest.find('`') {
        if rest[..start].contains('<') || rest[..start].contains("](") {
            break;
        }
        let backslashes = rest[..start].len() - rest[..start].trim_end_matches('\\').len();
        if backslashes % 2 == 1 {
            out.push_str(&rest[..=start]);
            rest = &rest[start + 1..];
            continue;
        }
        out.push_str(&rest[..start]);
        let after_start = &rest[start..];
        let run = after_start.len() - after_start.trim_start_matches('`').len();
        let body = &after_start[run..];
        let mut search = 0;
        let close = loop {
            let Some(found) = body[search..].find('`') else {
                break None;
            };
            let at = search + found;
            let len = body[at..].len() - body[at..].trim_start_matches('`').len();
            if len == run {
                break Some(at);
            }
            search = at + len;
        };
        if let Some(at) = close {
            rest = &body[at + run..];
        } else {
            out.push_str(&after_start[..run]);
            rest = body;
            left_open = true;
        }
    }
    out.push_str(rest);
    let continues = left_open || out.contains('<') || out.contains("](");
    (out, continues)
}

/// The line past any blockquote (`>`) and list (`-`, `*`, `+`, `1.`, `1)`)
/// markers that open it.
fn after_container_markers(line: &str) -> &str {
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        if let Some(after) = trimmed.strip_prefix('>') {
            rest = after;
            continue;
        }
        let digits = trimmed.len()
            - trimmed
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .len();
        let marker = if digits > 0 {
            trimmed[digits..].strip_prefix(['.', ')'])
        } else {
            trimmed.strip_prefix(['-', '*', '+'])
        };
        match marker {
            Some(after) if after.starts_with(char::is_whitespace) => rest = after,
            _ => return trimmed,
        }
    }
}

fn pr_body_tail(prepared: &VersionWritePlan) -> String {
    let mut body = String::new();
    if !prepared.lockfile_left.is_empty() {
        body.push_str(
            "\n### Lockfile entries left for `pnpm install`\n\n\
             A frozen install fails on this branch until `pnpm install` updates these \
             entries. Each `oakum ci version-pr` run replaces this branch, so fix an \
             entry that had already drifted on the default branch, and push any other \
             fix here after the last run.\n\n",
        );
        for line in &prepared.lockfile_left {
            let _ = writeln!(body, "- {line}");
        }
    }
    if !prepared.anchor_notes.is_empty() {
        body.push_str("\n### Catalog entries sharing a YAML anchor\n\n");
        for note in &prepared.anchor_notes {
            let _ = writeln!(body, "- {note}");
        }
    }
    let _ = write!(body, "\nGenerated by oakum {}.\n", prepared.tool_version);
    body
}

#[cfg(test)]
mod tests {
    use super::{
        commit_headline, github_len, github_path, parse_github_origin, parse_slug, pr_body,
        pr_title, pull_number_from_event, pull_number_from_ref, release_notes_section,
        workflow_run_pull_number, ReleaseNote, VersionWritePlan, MAX_BODY_CHARS, VERSION_HEADLINE,
    };
    use crate::cli::intent::DroppedIntent;
    use crate::cli::repository;
    use crate::cli::write_set::{PlannedDelete, PlannedWrite};
    use oakum::plan::Plan;
    use oakum::state::{CoverageOutcome, ReleaseState, RenderTarget, UnresolvedScope};
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
            lockfile_left: Vec::new(),
            anchor_notes: Vec::new(),
            release_notes: Vec::new(),
            dropped: DroppedIntent::default(),
        }
    }

    fn section(notes: &[ReleaseNote], budget: usize) -> String {
        release_notes_section(notes, budget).unwrap_or_default()
    }

    fn note(package: &str, entry: String) -> ReleaseNote {
        ReleaseNote {
            ecosystem: oakum::plan::Ecosystem::Cargo,
            package: package.to_owned(),
            version: semver::Version::new(1, 2, 3),
            path: format!("crates/{package}/CHANGELOG.md"),
            entry,
        }
    }

    #[test]
    fn the_version_pr_body_quotes_each_entry_in_a_collapsed_block() {
        let plan = VersionWritePlan {
            release_notes: vec![
                note("alpha", String::from("### Fixed\n\n- alpha fix")),
                note("beta", String::from("### Added\n\n- beta feature")),
            ],
            ..stub_plan(Vec::new(), Vec::new())
        };
        let body = pr_body(&plan).expect("body");
        assert!(
            body.contains(
                "### Release notes\n\n<details>\n<summary>alpha 1.2.3 (cargo)</summary>\n\n### Fixed\n\n- alpha fix\n\n</details>\n\n<details>\n<summary>beta 1.2.3 (cargo)</summary>"
            ),
            "{body}"
        );
        assert!(body.ends_with("Generated by oakum 0.0.0.\n"), "{body}");
    }

    #[test]
    fn a_single_entry_renders_open() {
        let plan = VersionWritePlan {
            release_notes: vec![note("alpha", String::from("- fix"))],
            ..stub_plan(Vec::new(), Vec::new())
        };
        let body = pr_body(&plan).expect("body");
        assert!(
            body.contains("<details open>\n<summary>alpha 1.2.3 (cargo)</summary>"),
            "{body}"
        );
    }

    /// GitHub refuses a body over its limit, so an entry that does not fit is
    /// named with its file rather than cut mid-sentence.
    #[test]
    fn an_entry_past_the_body_limit_names_its_file() {
        let plan = VersionWritePlan {
            release_notes: vec![
                note("alpha", "a".repeat(MAX_BODY_CHARS / 2)),
                note("beta", "b".repeat(MAX_BODY_CHARS / 2)),
                note("gamma", String::from("- small")),
            ],
            ..stub_plan(Vec::new(), Vec::new())
        };
        let body = pr_body(&plan).expect("body");
        assert!(github_len(&body) <= MAX_BODY_CHARS, "{}", body.len());
        assert!(body.contains("<summary>alpha 1.2.3 (cargo)</summary>"));
        assert!(!body.contains(&"b".repeat(100)));
        assert!(
            body.contains(
                "- beta 1.2.3 (cargo): left out for length; see `crates/beta/CHANGELOG.md` on this branch"
            ),
            "{}",
            &body[body.len().saturating_sub(800)..]
        );
        assert!(body.contains("<summary>gamma 1.2.3 (cargo)</summary>"));
        assert!(body.ends_with("Generated by oakum 0.0.0.\n"));
    }

    /// A block that fits must leave room for the lines naming the entries after
    /// it, and below even those one line stands for the section.
    #[test]
    fn the_release_notes_section_stays_within_any_budget() {
        let notes = [
            note("alpha", "a".repeat(100)),
            note("beta", "b".repeat(100)),
        ];
        let full = github_len(&section(&notes, usize::MAX));
        for budget in 0..full + 50 {
            let section = section(&notes, budget);
            assert!(github_len(&section) <= budget, "{budget}: {section}");
        }
        assert!(section(&notes, 120).contains("Left out for length"));
    }

    #[test]
    fn an_entry_that_just_fits_is_quoted() {
        let notes = [note("alpha", "a".repeat(100))];
        let full = section(&notes, usize::MAX);
        assert_eq!(section(&notes, github_len(&full)), full);
        assert!(section(&notes, github_len(&full) - 1).contains("left out for length"));
    }

    #[test]
    fn the_budget_counts_utf16_units() {
        let notes = [note("alpha", "😀".repeat(50))];
        let full = section(&notes, usize::MAX);
        assert!(section(&notes, full.chars().count()).contains("left out for length"));
    }

    /// An HTML block left open swallows every block and the footer after it.
    #[test]
    fn an_entry_holding_raw_html_names_its_file() {
        for entry in [
            "<pre>example",
            "- fix\n\n</details>",
            "  <!-- open",
            "- a <details> tag",
            "- <!-- open",
            "> <pre>",
            "1. <script>",
            "- fix <?processing",
            "- one\n  ```\n  code\noutdent\n<!-- open\n```",
            "```html\n<pre>\n```",
            "-\t<pre>",
            "- a `` <details> `` block with an unmatched `<details> tag",
            "- an ``unclosed span <details>",
            "use \\`<details>` now",
            "[x](url \"`\") <details> `",
            "see <a title=\"`\"> <details> `",
            "[x](/u\n\"`\") <details> `",
            "a <a title=\"\n`\"> <details> `",
            "`start\n` <details> ` end`",
            "- a `b\n  c` <details> `d",
        ] {
            let notes = [
                note("alpha", entry.to_owned()),
                note("beta", String::from("- ok")),
            ];
            let section = section(&notes, usize::MAX);
            assert!(
                section.contains("- alpha 1.2.3 (cargo): left out because it holds raw HTML"),
                "{entry}: {section}"
            );
            assert!(
                section.contains("<summary>beta 1.2.3 (cargo)</summary>"),
                "{section}"
            );
        }
        for entry in [
            "```rust\nlet a = b;\n```",
            "- use `Vec<T>` now",
            "a < b holds",
            // The 0.8.0 note that left the whole entry out of its own version
            // pull request.
            "A heading inside an HTML comment or a `<details>` block no longer counts.",
            "- `<!-- x -->` and ``a `<?pi` b`` are text",
            "- ``a`b <details> c`` closes only on a run of its own length",
            "a stray ` in one paragraph\n\n- a `<details>` block in the next",
        ] {
            let quoted = section(&[note("alpha", entry.to_owned())], usize::MAX);
            assert!(quoted.contains("<summary>"), "{entry}: {quoted}");
        }
    }

    #[test]
    fn an_empty_entry_says_so() {
        let section = section(&[note("alpha", String::from("\n"))], usize::MAX);
        assert!(
            section.contains("- alpha 1.2.3 (cargo): the entry is empty"),
            "{section}"
        );
        assert!(!section.contains("<details"), "{section}");
    }

    /// GitHub refuses the body only after the branch moved, so a body the
    /// notes cannot shrink under the limit stops the run first.
    #[test]
    fn a_body_over_the_limit_is_refused() {
        let plan = VersionWritePlan {
            anchor_notes: vec!["x".repeat(MAX_BODY_CHARS)],
            release_notes: vec![note("alpha", String::from("- fix"))],
            ..stub_plan(Vec::new(), Vec::new())
        };
        let err = pr_body(&plan).expect_err("over the limit");
        assert!(err.to_string().contains("over GitHub's"), "{err}");
        assert!(err.to_string().contains("nothing was pushed"), "{err}");
        let plan = VersionWritePlan {
            anchor_notes: Vec::new(),
            ..plan
        };
        assert!(pr_body(&plan).is_ok());
    }

    /// Notes that cannot be named even in one line refuse the body rather
    /// than leave it without a word of them.
    #[test]
    fn notes_with_no_room_for_a_line_are_refused() {
        let stub = stub_plan(Vec::new(), Vec::new());
        let bare = github_len(&pr_body(&stub).expect("body"));
        let plan = VersionWritePlan {
            // The anchor heading takes about fifty of these; the compact line needs more than the rest.
            anchor_notes: vec!["x".repeat(MAX_BODY_CHARS - bare - 80)],
            release_notes: vec![note("alpha", String::from("- fix"))],
            ..stub
        };
        let err = pr_body(&plan).expect_err("no room for the notes");
        assert!(err.to_string().contains("leaving no room"), "{err}");
    }

    /// The version PR is what a commits-only repository reads before it
    /// merges, so it names what `status` named.
    #[test]
    fn the_version_pr_body_names_intent_commits_dropped() {
        let plan = VersionWritePlan {
            dropped: DroppedIntent {
                named_unmanaged: vec![oakum::plan::PackageId::new(
                    oakum::plan::Ecosystem::Cargo,
                    "beta",
                )],
                unresolved: vec![UnresolvedScope {
                    commit: String::from("0123456789abcdef"),
                    scope: String::from("gamma"),
                }],
                awaiting_tag: Vec::new(),
            },
            ..stub_plan(Vec::new(), Vec::new())
        };
        let body = pr_body(&plan).expect("body");
        assert!(
            body.contains("Named by a commit but not version-managed: beta (`cargo`)"),
            "{body}"
        );
        assert!(
            body.contains("commit 0123456 asks for a release under scope `gamma`"),
            "{body}"
        );

        let templated = VersionWritePlan {
            title: Some(oakum::template::TemplateSource::Inline(String::from(
                "release{% for p in named_unmanaged %} without {{ p.name }}{% endfor %}{% for c in unresolved_scopes %} or {{ c.scope }}{% endfor %}",
            ))),
            ..plan
        };
        assert_eq!(
            pr_title(&templated).expect("title"),
            "release without beta or gamma"
        );
    }

    #[test]
    fn an_unconfigured_title_is_the_commit_headline_and_is_conventional() {
        let plan = stub_plan(Vec::new(), Vec::new());
        let title = pr_title(&plan).expect("title");

        assert_eq!(title, commit_headline(&plan).expect("headline"));
        // `cog verify` refuses a type outside the conventional set; parsing
        // alone accepts any.
        let parsed = git_conventional::Commit::parse(&title)
            .expect("the title a squash merge lands parses as a conventional commit");
        assert_eq!(parsed.type_(), git_conventional::Type::CHORE);
    }

    /// Each key moves its own surface and nothing else. The configured message is
    /// itself conventional on purpose: a re-coupling would most plausibly mirror
    /// `commit-message` into the title only where it already parses, and a
    /// non-conventional fixture walks straight past that.
    #[test]
    fn a_configured_message_and_title_stay_on_their_own_surfaces() {
        let mut message_only = stub_plan(Vec::new(), Vec::new());
        message_only.commit_message = Some(oakum::template::TemplateSource::Inline(String::from(
            "chore(release): v9.9.9",
        )));
        assert_eq!(
            commit_headline(&message_only).expect("headline"),
            "chore(release): v9.9.9"
        );
        assert_eq!(pr_title(&message_only).expect("title"), VERSION_HEADLINE);

        let mut title_only = stub_plan(Vec::new(), Vec::new());
        title_only.title = Some(oakum::template::TemplateSource::Inline(String::from(
            "Version Packages",
        )));
        assert_eq!(pr_title(&title_only).expect("title"), "Version Packages");
        assert_eq!(
            commit_headline(&title_only).expect("headline"),
            VERSION_HEADLINE
        );
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
        let body = pr_body(&prepared).expect("body");
        assert!(body.contains("No packages planned."), "{body}");
        assert!(body.contains("Generated by oakum 0.0.0.\n"), "{body}");
        assert!(!body.contains("pnpm install"), "{body}");
    }

    /// A workflow's stderr is read by nobody, so the version pull request
    /// itself names what a frozen install will refuse.
    #[test]
    fn pr_body_names_lockfile_entries_left() {
        let mut prepared = stub_plan(Vec::new(), Vec::new());
        prepared.lockfile_left = vec![String::from(
            "pnpm-lock.yaml: left `importers/packages/b/dependencies/a` specifier is `x`, not the `y` the manifest had",
        )];
        let body = pr_body(&prepared).expect("body");
        assert!(
            body.contains("### Lockfile entries left for `pnpm install`"),
            "{body}"
        );
        assert!(
            body.contains("\n- pnpm-lock.yaml: left `importers/packages/b/dependencies/a`"),
            "{body}"
        );
        assert!(body.ends_with("Generated by oakum 0.0.0.\n"), "{body}");
    }

    #[test]
    fn pr_body_names_catalog_entries_sharing_an_anchor() {
        let mut prepared = stub_plan(Vec::new(), Vec::new());
        assert!(!pr_body(&prepared).expect("body").contains("YAML anchor"));
        prepared.anchor_notes = vec![String::from(
            "pnpm-workspace.yaml: `catalog/a` was the alias `*caret`; it is written out, so `&caret` does not move with it",
        )];
        let body = pr_body(&prepared).expect("body");
        assert!(
            body.contains(
                "### Catalog entries sharing a YAML anchor\n\n- pnpm-workspace.yaml: `catalog/a` was the alias"
            ),
            "{body}"
        );
        assert!(body.ends_with("Generated by oakum 0.0.0.\n"), "{body}");
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
    fn needs_github_is_false_for_a_delete_without_writes() {
        let prepared = stub_plan(
            Vec::new(),
            vec![PlannedDelete::new(PathBuf::from(".changeset/one.md"))],
        );
        assert!(!prepared.needs_github());
    }

    #[test]
    fn needs_github_is_true_for_a_delete_beside_a_write() {
        let prepared = stub_plan(
            vec![PlannedWrite::new(
                PathBuf::from("Cargo.toml"),
                "0.1.0",
                "0.1.1",
            )],
            vec![PlannedDelete::new(PathBuf::from(".changeset/one.md"))],
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
        // `pr-status` posts from the pull request's own run, never a
        // `workflow_run`; only `post-comment` reads one.
        let run = json!({"workflow_run":{"pull_requests":[{"number":7}]}});
        assert_eq!(pull_number_from_event(&run), None);
        assert_eq!(workflow_run_pull_number(&run), Some(7));
        for none in [json!([]), json!([{"number":7},{"number":8}])] {
            assert_eq!(
                workflow_run_pull_number(&json!({"workflow_run":{"pull_requests":none}})),
                None
            );
        }
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
