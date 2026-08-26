//! `oakum ci`: GitHub writes for CI. `version-pr` opens or updates the version PR.

use std::fmt::Write;
use std::path::Path;
use std::process::Command;

use clap::{Args, Subcommand};
use oakum::state::{ReleaseState, RenderTarget};

use super::github::{self, FileAddition, FileChanges, FileDeletion, Look};
use super::status;
use super::template::load_template_body;
use super::version::{self, VersionArgs, VersionWritePlan};
use super::CliError;

const VERSION_BRANCH: &str = "oakum/version-packages";
const DEFAULT_TITLE: &str = "Version Packages";
const DEFAULT_COMMIT: &str = "Version Packages";

#[derive(Debug, Args)]
pub(super) struct CiArgs {
    #[command(subcommand)]
    command: CiCommand,
}

#[derive(Debug, Subcommand)]
enum CiCommand {
    /// Create or update the version pull request.
    VersionPr(VersionArgs),
}

pub(super) fn run(args: &CiArgs) -> Result<(), CliError> {
    match &args.command {
        CiCommand::VersionPr(args) => run_version_pr(args),
    }
}

fn run_version_pr(args: &VersionArgs) -> Result<(), CliError> {
    let prepared = version::plan_writes(args).map_err(CliError::from_boxed)?;
    if !prepared.needs_github() {
        println!("nothing to version");
        return Ok(());
    }
    let client = github::Client::new(github_token()?)?;
    let (owner, name) = repository_slug(&prepared.repo_path)?;
    let default_branch = client.default_branch(&owner, &name)?;
    let base_oid = match client.branch_head(&owner, &name, &default_branch)? {
        Look::Found(oid) => oid,
        Look::Empty => {
            return Err(CliError::new(format!(
                "default branch `{default_branch}` has no head"
            )));
        }
    };
    let head = local_head(&prepared.repo_path)?;
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
    let existing = match client.open_pulls_for_head(&owner, &name, VERSION_BRANCH)? {
        Look::Found(pulls) if pulls.len() > 1 => {
            return Err(CliError::new(format!(
                "multiple open version pull requests on `{VERSION_BRANCH}` ({})",
                pulls.len()
            )));
        }
        Look::Found(pulls) if pulls.len() == 1 => Some(pulls.into_iter().next().expect("one pull")),
        Look::Found(_) | Look::Empty => None,
    };
    client.point_branch(&owner, &name, VERSION_BRANCH, &base_oid)?;
    client.create_commit_on_branch(
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

fn local_head(repo: &Path) -> Result<String, CliError> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("failed to run git rev-parse HEAD: {err}")))?;
    if !output.status.success() {
        return Err(CliError::new(
            "`oakum ci version-pr` needs a git HEAD to compare with the default branch",
        ));
    }
    let sha = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new("git HEAD is not valid UTF-8"))?;
    let sha = sha.trim();
    if sha.is_empty() {
        return Err(CliError::new("git HEAD is empty"));
    }
    Ok(sha.to_owned())
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
        "`oakum ci version-pr` needs GITHUB_TOKEN or GH_TOKEN",
    ))
}

fn repository_slug(repo: &Path) -> Result<(String, String), CliError> {
    if let Ok(value) = std::env::var("GITHUB_REPOSITORY") {
        let value = value.trim();
        if !value.is_empty() {
            return parse_slug(value).ok_or_else(|| {
                CliError::new(format!("GITHUB_REPOSITORY `{value}` is not owner/repo"))
            });
        }
    }
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("failed to run git remote get-url origin: {err}")))?;
    if !output.status.success() {
        return Err(CliError::new(
            "`oakum ci version-pr` needs GITHUB_REPOSITORY or a git origin remote",
        ));
    }
    let url = String::from_utf8(output.stdout)
        .map_err(|_| CliError::new("git origin URL is not valid UTF-8"))?;
    parse_github_origin(url.trim()).ok_or_else(|| {
        CliError::new(format!(
            "git origin `{url}` is not a github.com owner/repo URL"
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
    use super::{github_path, parse_github_origin, parse_slug, pr_body, VersionWritePlan};
    use crate::cli::write_set::{PlannedDelete, PlannedWrite};
    use oakum::plan::Plan;
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
}
