//! GitHub HTTP (okm-dlo). Lives in the binary so it does not add a second
//! `src/` I/O marker (ADR-0002). No octocrab.
//!
//! Tag push is git, not this client ([github-release-path.md](../../../../docs/research/github-release-path.md)).

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "check_runs is not on a write path yet")
)]

use std::collections::HashSet;
use std::fmt::{self, Write};

use reqwest::blocking::Client as Http;
use reqwest::header::{HeaderMap, IF_NONE_MATCH};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};

const USER_AGENT: &str = "oakum";
const CREATE_COMMIT: &str = r"
mutation($input: CreateCommitOnBranchInput!) {
  createCommitOnBranch(input: $input) {
    commit { oid }
  }
}
";

pub(crate) struct Client {
    http: Http,
    api: String,
    graphql: String,
    token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileAddition {
    pub path: String,
    pub contents_base64: String,
}

impl FileAddition {
    pub(crate) fn from_text(path: impl Into<String>, text: &str) -> Self {
        Self {
            path: path.into(),
            contents_base64: encode_base64(text.as_bytes()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileDeletion {
    pub path: String,
}

impl FileDeletion {
    pub(crate) fn new(path: impl Into<String>) -> Result<Self, Error> {
        let path = path.into();
        if path.is_empty() {
            return Err(Error::new("a file deletion path is empty"));
        }
        Ok(Self { path })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FileChanges<'a> {
    pub additions: &'a [FileAddition],
    pub deletions: &'a [FileDeletion],
}

impl FileChanges<'_> {
    fn validate(self) -> Result<Self, Error> {
        let mut additions = HashSet::new();
        for file in self.additions {
            let path = git_path(&file.path, "addition")?;
            if !additions.insert(path) {
                return Err(Error::new(format!(
                    "fileChanges addition path is duplicated: {}",
                    file.path
                )));
            }
        }
        let mut deletions = HashSet::new();
        for file in self.deletions {
            let path = git_path(&file.path, "deletion")?;
            if additions.contains(&path) {
                return Err(Error::new(format!(
                    "fileChanges path appears in both additions and deletions: {}",
                    file.path
                )));
            }
            if !deletions.insert(path) {
                return Err(Error::new(format!(
                    "fileChanges deletion path is duplicated: {}",
                    file.path
                )));
            }
        }
        Ok(self)
    }
}

fn encode_path_segment(value: &str) -> String {
    path_segment(value)
}

/// The token from the environment, if either name carries a non-empty one.
/// Callers name the command that needs it, so the remedy is specific.
pub(crate) fn token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|token| !token.is_empty())
}

/// Pin for printed workflows. A baked-in major goes stale.
pub(crate) fn latest_release_tag(owner: &str, repo: &str) -> Result<String, Error> {
    Client::public()?.latest_release_tag(owner, repo)
}

fn action_ref(tag: &str) -> Result<String, Error> {
    if tag.starts_with('-') || tag.contains("..") {
        return Err(Error::unverified(format!(
            "unverified: GitHub latest release tag is not an action pin: {tag}"
        )));
    }
    if tag
        .bytes()
        .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
    {
        Ok(tag.to_owned())
    } else {
        Err(Error::unverified(format!(
            "unverified: GitHub latest release tag is not an action pin: {tag}"
        )))
    }
}

pub(crate) fn git_path(path: &str, kind: &str) -> Result<String, Error> {
    let path = path.trim().replace('\\', "/");
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() {
            return Err(Error::new(format!("a file {kind} path is not a git path")));
        }
        if part == "." {
            continue;
        }
        if part == ".." {
            return Err(Error::new(format!("a file {kind} path is not a git path")));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(Error::new(format!("a file {kind} path is empty")));
    }
    Ok(parts.join("/"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PullRequest {
    pub number: u64,
    pub html_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: String,
}

const PLAN_AUTHOR: &str = "github-actions[bot]";
const COMMENT_PAGES: u32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreatedCommit {
    pub oid: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreatedRelease {
    pub html_url: String,
    pub tag_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct WorkflowRun {
    pub id: u64,
    pub head_sha: String,
    /// For a push-event run this is the pushed ref's short name — a tag
    /// push's run carries the tag name, not a branch (measured, okm-e9e.17).
    pub head_branch: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub path: Option<String>,
    pub event: Option<String>,
    #[serde(default)]
    pub html_url: String,
}

/// Empty means we asked and GitHub had nothing — not "we did not look."
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Look<T> {
    Found(T),
    Empty,
}

impl<T> Look<Vec<T>> {
    pub(crate) fn of(items: Vec<T>) -> Self {
        if items.is_empty() {
            Self::Empty
        } else {
            Self::Found(items)
        }
    }
}

/// Conditional poll: cache hit, not an empty look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refresh<T> {
    Fresh(Look<T>),
    NotModified,
}

#[derive(Debug)]
pub(crate) enum Error {
    Unverified {
        detail: String,
    },
    Forbidden {
        path: String,
    },
    /// Structural rather than a formatted `Other`, so classifying a 401 never
    /// depends on text the response body controls.
    Unauthorized {
        path: String,
    },
    Other(String),
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    fn unverified(detail: impl Into<String>) -> Self {
        Self::Unverified {
            detail: detail.into(),
        }
    }

    fn forbidden(path: impl Into<String>) -> Self {
        Self::Forbidden { path: path.into() }
    }

    pub(crate) fn is_forbidden(&self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }
}

/// An expired token leaves a lookup as unread as a 502, so auth failures are
/// `unverified` — but only auth: a 400 or 422 is GitHub refusing a request it
/// read, a look that happened. Per call site, so writes keep their classes.
fn looked<T>(result: Result<T, Error>) -> Result<T, Error> {
    result.map_err(|err| match err {
        auth @ (Error::Forbidden { .. } | Error::Unauthorized { .. }) => {
            Error::unverified(format!("unverified: {auth}"))
        }
        other => other,
    })
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unverified { detail } | Self::Other(detail) => f.write_str(detail),
            Self::Forbidden { path } => write!(f, "GitHub {path} returned 403"),
            Self::Unauthorized { path } => write!(f, "GitHub {path} returned 401"),
        }
    }
}

impl std::error::Error for Error {}

impl Client {
    pub(crate) fn new(token: impl Into<String>) -> Result<Self, Error> {
        let api =
            env_url("GITHUB_API_URL").unwrap_or_else(|| String::from("https://api.github.com"));
        let graphql = env_url("GITHUB_GRAPHQL_URL").unwrap_or_else(|| graphql_from_api(&api));
        Self::at_urls(api, graphql, token)
    }

    /// `pub(super)` is the unit seam: without it every handoff assertion
    /// costs a spawned binary, a temp repo, and a bare origin.
    pub(super) fn at(api: impl Into<String>, token: impl Into<String>) -> Result<Self, Error> {
        let api = api.into().trim_end_matches('/').to_owned();
        let graphql = graphql_from_api(&api);
        Self::at_urls(api, graphql, token)
    }

    fn at_urls(
        api: impl Into<String>,
        graphql: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, Error> {
        let token = token.into();
        if token.is_empty() {
            return Err(Error::new("GitHub token is empty"));
        }
        let http = Http::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| Error::new(err.to_string()))?;
        Ok(Self {
            http,
            api: api.into().trim_end_matches('/').to_owned(),
            graphql: graphql.into().trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// Token optional; used only for the rate limit.
    pub(crate) fn public() -> Result<Self, Error> {
        let token = env_url("GITHUB_TOKEN")
            .or_else(|| env_url("GH_TOKEN"))
            .unwrap_or_default();
        let api =
            env_url("GITHUB_API_URL").unwrap_or_else(|| String::from("https://api.github.com"));
        let graphql = env_url("GITHUB_GRAPHQL_URL").unwrap_or_else(|| graphql_from_api(&api));
        let http = Http::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| Error::new(err.to_string()))?;
        Ok(Self {
            http,
            api: api.trim_end_matches('/').to_owned(),
            graphql: graphql.trim_end_matches('/').to_owned(),
            token,
        })
    }

    pub(crate) fn latest_release_tag(&self, owner: &str, repo: &str) -> Result<String, Error> {
        let path = format!(
            "/repos/{}/{}/releases/latest",
            encode_path_segment(owner),
            encode_path_segment(repo)
        );
        let Some(value) = self.json_or_missing(reqwest::Method::GET, &path, None)? else {
            return Err(Error::unverified(format!(
                "unverified: GitHub {path} returned 404"
            )));
        };
        let tag = value
            .get("tag_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .ok_or_else(|| {
                Error::unverified(format!("unverified: GitHub {path} omitted tag_name"))
            })?;
        action_ref(tag)
    }

    pub(crate) fn create_commit_on_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        expected_head_oid: &str,
        headline: &str,
        files: FileChanges<'_>,
    ) -> Result<CreatedCommit, Error> {
        let files = files.validate()?;
        let additions = files
            .additions
            .iter()
            .map(|file| {
                Ok(json!({
                    "path": git_path(&file.path, "addition")?,
                    "contents": file.contents_base64,
                }))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let deletions = files
            .deletions
            .iter()
            .map(|file| Ok(json!({ "path": git_path(&file.path, "deletion")? })))
            .collect::<Result<Vec<_>, Error>>()?;
        let body = json!({
            "query": CREATE_COMMIT,
            "variables": {
                "input": {
                    "branch": {
                        "repositoryNameWithOwner": format!("{owner}/{repo}"),
                        "branchName": branch,
                    },
                    "expectedHeadOid": expected_head_oid,
                    "message": { "headline": headline },
                    "fileChanges": { "additions": additions, "deletions": deletions },
                }
            }
        });
        let value = self.graphql(&body)?;
        let oid = value
            .pointer("/data/createCommitOnBranch/commit/oid")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|oid| !oid.is_empty())
            .ok_or_else(|| Error::new("createCommitOnBranch returned no commit oid"))?;
        Ok(CreatedCommit {
            oid: oid.to_owned(),
        })
    }

    pub(crate) fn create_release(
        &self,
        owner: &str,
        repo: &str,
        tag_name: &str,
        name: &str,
        body: &str,
    ) -> Result<CreatedRelease, Error> {
        let payload = json!({
            "tag_name": tag_name,
            "name": name,
            "body": body,
        });
        let value = self.json(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/releases"),
            Some(&payload),
        )?;
        Ok(CreatedRelease {
            html_url: release_html_url(&value, "create release")?,
            tag_name: tag_name.to_owned(),
        })
    }

    pub(crate) fn release_for_tag(
        &self,
        owner: &str,
        repo: &str,
        tag_name: &str,
    ) -> Result<Look<CreatedRelease>, Error> {
        let path = format!(
            "/repos/{owner}/{repo}/releases/tags/{}",
            encode_path_segment(tag_name)
        );
        match looked(self.json_or_missing(reqwest::Method::GET, &path, None))? {
            None => Ok(Look::Empty),
            Some(value) => {
                let tag_name = value
                    .get("tag_name")
                    .and_then(Value::as_str)
                    .unwrap_or(tag_name);
                Ok(Look::Found(CreatedRelease {
                    html_url: release_html_url(&value, "release lookup")?,
                    tag_name: tag_name.to_owned(),
                }))
            }
        }
    }

    pub(crate) fn check_runs(
        &self,
        owner: &str,
        repo: &str,
        git_ref: &str,
    ) -> Result<Look<Vec<CheckRun>>, Error> {
        #[derive(Deserialize)]
        struct Payload {
            total_count: u64,
            check_runs: Vec<CheckRun>,
        }
        let value = self.json(
            reqwest::Method::GET,
            &format!(
                "/repos/{owner}/{repo}/commits/{}/check-runs?per_page=100",
                path_segment(git_ref)
            ),
            None,
        )?;
        let payload: Payload = serde_json::from_value(value)
            .map_err(|err| Error::unverified(format!("unverified: check-runs body: {err}")))?;
        complete_page(payload.total_count, payload.check_runs, "check-runs")
    }

    pub(crate) fn workflow_runs(
        &self,
        owner: &str,
        repo: &str,
        head_sha: &str,
        etag: Option<&str>,
    ) -> Result<(Refresh<Vec<WorkflowRun>>, Option<String>), Error> {
        #[derive(Deserialize)]
        struct Payload {
            total_count: u64,
            workflow_runs: Vec<WorkflowRun>,
        }
        let (status, headers, value) = self.raw_json(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}/actions/runs?head_sha={head_sha}&per_page=100"),
            None,
            etag,
        )?;
        if status == StatusCode::NOT_MODIFIED {
            return Ok((Refresh::NotModified, etag.map(str::to_owned)));
        }
        let payload: Payload = serde_json::from_value(value)
            .map_err(|err| Error::unverified(format!("unverified: workflow-runs body: {err}")))?;
        let next_etag = headers
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let look = complete_page(payload.total_count, payload.workflow_runs, "workflow-runs")?;
        Ok((Refresh::Fresh(look), next_etag))
    }

    pub(crate) fn default_branch(&self, owner: &str, repo: &str) -> Result<String, Error> {
        let value = self.json(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}"),
            None,
        )?;
        value
            .get("default_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::new("repository returned no default_branch"))
    }

    pub(crate) fn branch_head(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Look<String>, Error> {
        let path = format!(
            "/repos/{owner}/{repo}/git/ref/heads/{}",
            path_segment(branch)
        );
        match self.json_or_missing(reqwest::Method::GET, &path, None)? {
            None => Ok(Look::Empty),
            Some(value) => {
                let sha = value
                    .pointer("/object/sha")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::new("git ref returned no object.sha"))?;
                let sha = sha.trim();
                if sha.is_empty() {
                    return Err(Error::new("git ref returned empty object.sha"));
                }
                Ok(Look::Found(sha.to_owned()))
            }
        }
    }

    pub(crate) fn point_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        sha: &str,
    ) -> Result<(), Error> {
        match self.branch_head(owner, repo, branch)? {
            Look::Empty => {
                let payload = json!({
                    "ref": format!("refs/heads/{branch}"),
                    "sha": sha,
                });
                self.json(
                    reqwest::Method::POST,
                    &format!("/repos/{owner}/{repo}/git/refs"),
                    Some(&payload),
                )?;
            }
            Look::Found(_) => {
                let payload = json!({ "sha": sha, "force": true });
                self.json(
                    reqwest::Method::PATCH,
                    &format!(
                        "/repos/{owner}/{repo}/git/refs/heads/{}",
                        path_segment(branch)
                    ),
                    Some(&payload),
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn open_pulls_for_head(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Look<Vec<PullRequest>>, Error> {
        let head = format!("{owner}:{branch}");
        let path = format!(
            "/repos/{owner}/{repo}/pulls?head={}&state=open&per_page=100",
            path_segment(&head)
        );
        let value = self.json(reqwest::Method::GET, &path, None)?;
        let pulls: Vec<PullJson> = serde_json::from_value(value)
            .map_err(|err| Error::unverified(format!("unverified: pulls body: {err}")))?;
        if pulls.len() == 100 {
            return Err(Error::unverified(
                "unverified: pulls page is incomplete (100/unknown)",
            ));
        }
        Ok(Look::of(
            pulls
                .into_iter()
                .map(|pull| PullRequest {
                    number: pull.number,
                    html_url: pull.html_url,
                })
                .collect(),
        ))
    }

    pub(crate) fn create_pull(
        &self,
        owner: &str,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, Error> {
        let payload = json!({
            "title": title,
            "body": body,
            "head": head,
            "base": base,
        });
        let value = self.json(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/pulls"),
            Some(&payload),
        )?;
        pull_from_value(value)
    }

    pub(crate) fn update_pull(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, Error> {
        let payload = json!({ "title": title, "body": body });
        let value = self.json(
            reqwest::Method::PATCH,
            &format!("/repos/{owner}/{repo}/pulls/{number}"),
            Some(&payload),
        )?;
        pull_from_value(value)
    }

    pub(crate) fn issue_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Look<Vec<IssueComment>>, Error> {
        let mut comments = Vec::new();
        for page in 1u32..=COMMENT_PAGES {
            let path =
                format!("/repos/{owner}/{repo}/issues/{number}/comments?per_page=100&page={page}");
            let value = self.json(reqwest::Method::GET, &path, None)?;
            let batch: Vec<IssueCommentJson> = serde_json::from_value(value).map_err(|err| {
                Error::unverified(format!("unverified: issue comments body: {err}"))
            })?;
            let count = batch.len();
            comments.extend(batch.into_iter().filter_map(|comment| {
                let user = comment.user?.login;
                if user.is_empty() {
                    return None;
                }
                Some(IssueComment {
                    id: comment.id,
                    body: comment.body,
                    user,
                })
            }));
            if count < 100 {
                return Ok(Look::of(comments));
            }
        }
        Err(Error::unverified(format!(
            "unverified: issue comments page is incomplete ({}/unknown)",
            COMMENT_PAGES * 100
        )))
    }

    fn create_issue_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<u64, Error> {
        let path = format!("/repos/{owner}/{repo}/issues/{number}/comments");
        let value = self.json(reqwest::Method::POST, &path, Some(&json!({ "body": body })))?;
        value
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::new("create issue comment returned no id"))
    }

    fn update_issue_comment(
        &self,
        owner: &str,
        repo: &str,
        id: u64,
        body: &str,
    ) -> Result<(), Error> {
        let path = format!("/repos/{owner}/{repo}/issues/comments/{id}");
        self.json(
            reqwest::Method::PATCH,
            &path,
            Some(&json!({ "body": body })),
        )?;
        Ok(())
    }

    fn delete_issue_comment(&self, owner: &str, repo: &str, id: u64) -> Result<(), Error> {
        let path = format!("/repos/{owner}/{repo}/issues/comments/{id}");
        self.json(reqwest::Method::DELETE, &path, None)?;
        Ok(())
    }

    pub(crate) fn upsert_plan_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        marker: &str,
        body: &str,
    ) -> Result<u64, Error> {
        let mut ours = owned_plan_comments(self.issue_comments(owner, repo, number)?, marker);
        let newest = ours.pop();
        for stale in ours {
            self.delete_issue_comment(owner, repo, stale.id)?;
        }
        match newest {
            Some(comment) => {
                self.update_issue_comment(owner, repo, comment.id, body)?;
                Ok(comment.id)
            }
            None => self.create_issue_comment(owner, repo, number, body),
        }
    }

    pub(crate) fn delete_plan_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        marker: &str,
    ) -> Result<(), Error> {
        for comment in owned_plan_comments(self.issue_comments(owner, repo, number)?, marker) {
            self.delete_issue_comment(owner, repo, comment.id)?;
        }
        Ok(())
    }

    fn json_or_missing(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>, Error> {
        let (status, _, value) = self.raw_json_inner(
            method,
            &format!("{}{path}", self.api),
            body,
            None,
            true,
            path,
        )?;
        if status == StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    fn graphql(&self, body: &Value) -> Result<Value, Error> {
        let value = self.json_url(&self.graphql, reqwest::Method::POST, Some(body))?;
        if let Some(errors) = value.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let messages = errors
                    .iter()
                    .filter_map(|err| err.get("message").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(Error::new(format!("GraphQL: {messages}")));
            }
        }
        Ok(value)
    }

    fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let (_, _, value) = self.raw_json(method, path, body, None)?;
        Ok(value)
    }

    fn json_url(
        &self,
        url: &str,
        method: reqwest::Method,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let (_, _, value) = self.raw_json_inner(method, url, body, None, false, url)?;
        Ok(value)
    }

    fn raw_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<(StatusCode, HeaderMap, Value), Error> {
        self.raw_json_inner(
            method,
            &format!("{}{path}", self.api),
            body,
            etag,
            false,
            path,
        )
    }

    fn raw_json_inner(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
        etag: Option<&str>,
        missing_ok: bool,
        error_path: &str,
    ) -> Result<(StatusCode, HeaderMap, Value), Error> {
        let mut request = self
            .http
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().map_err(|err| {
            Error::unverified(format!(
                "unverified: GitHub request to {error_path} failed: {err}"
            ))
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::NOT_MODIFIED {
            return Ok((status, headers, Value::Null));
        }
        if missing_ok && status == StatusCode::NOT_FOUND {
            return Ok((status, headers, Value::Null));
        }
        if status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN && rate_limited(&headers))
        {
            return Err(Error::unverified(format!(
                "unverified: GitHub {error_path} returned {status}"
            )));
        }
        if status == StatusCode::FORBIDDEN {
            let body = response.text().unwrap_or_default();
            if secondary_rate_limited(&body) {
                return Err(Error::unverified(format!(
                    "unverified: GitHub {error_path} returned {status}"
                )));
            }
            return Err(Error::forbidden(error_path));
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized {
                path: error_path.to_owned(),
            });
        }
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(Error::new(format!(
                "GitHub {error_path} returned {status}: {body}"
            )));
        }
        let text = response.text().map_err(|err| {
            Error::unverified(format!("unverified: GitHub {error_path} body: {err}"))
        })?;
        if text.trim().is_empty() {
            return Ok((status, headers, Value::Null));
        }
        let value = serde_json::from_str(&text).map_err(|err| {
            Error::unverified(format!("unverified: GitHub {error_path} JSON: {err}"))
        })?;
        Ok((status, headers, value))
    }
}

#[derive(Deserialize)]
struct PullJson {
    number: u64,
    html_url: String,
}

#[derive(Deserialize)]
struct IssueCommentJson {
    id: u64,
    #[serde(default)]
    body: String,
    user: Option<IssueUserJson>,
}

#[derive(Deserialize)]
struct IssueUserJson {
    login: String,
}

fn owned_plan_comments(comments: Look<Vec<IssueComment>>, marker: &str) -> Vec<IssueComment> {
    let mut ours: Vec<_> = match comments {
        Look::Empty => Vec::new(),
        Look::Found(items) => items
            .into_iter()
            .filter(|comment| comment.user == PLAN_AUTHOR && comment.body.contains(marker))
            .collect(),
    };
    ours.sort_by_key(|comment| comment.id);
    ours
}

fn pull_from_value(value: Value) -> Result<PullRequest, Error> {
    let pull: PullJson =
        serde_json::from_value(value).map_err(|err| Error::new(format!("pull body: {err}")))?;
    Ok(PullRequest {
        number: pull.number,
        html_url: pull.html_url,
    })
}

fn env_url(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
}

fn graphql_from_api(api: &str) -> String {
    let api = api.trim_end_matches('/');
    if let Some(prefix) = api.strip_suffix("/api/v3") {
        format!("{prefix}/api/graphql")
    } else {
        format!("{api}/graphql")
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied();
        let b2 = bytes.get(i + 2).copied();
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4)) as usize] as char);
        if b1.is_none() {
            out.push('=');
            out.push('=');
        } else {
            out.push(
                TABLE[(((b1.unwrap_or(0) & 0x0f) << 2) | (b2.unwrap_or(0) >> 6)) as usize] as char,
            );
            if b2.is_none() {
                out.push('=');
            } else {
                out.push(TABLE[(b2.unwrap_or(0) & 0x3f) as usize] as char);
            }
        }
        i += 3;
    }
    out
}

fn release_html_url(value: &Value, kind: &str) -> Result<String, Error> {
    let html_url = value.get("html_url").and_then(Value::as_str).unwrap_or("");
    if html_url.is_empty() {
        return Err(Error::unverified(format!(
            "unverified: {kind} body: missing html_url"
        )));
    }
    Ok(html_url.to_owned())
}

fn path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn secondary_rate_limited(body: &str) -> bool {
    body.to_ascii_lowercase().contains("secondary rate")
}

fn rate_limited(headers: &HeaderMap) -> bool {
    headers.contains_key("retry-after")
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            == Some("0")
}

fn complete_page<T>(total_count: u64, items: Vec<T>, what: &str) -> Result<Look<Vec<T>>, Error> {
    if total_count > items.len() as u64 {
        Err(Error::unverified(format!(
            "unverified: {what} page is incomplete ({}/{})",
            items.len(),
            total_count
        )))
    } else {
        Ok(Look::of(items))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CheckRun, Client, CreatedCommit, CreatedRelease, FileAddition, FileChanges, FileDeletion,
        Look, Refresh, WorkflowRun,
    };
    use httpmock::prelude::*;
    use serde_json::json;

    fn client(server: &MockServer) -> Client {
        Client::at(server.base_url(), "token").expect("client")
    }

    #[test]
    fn empty_token_is_refused() {
        let Err(err) = Client::new("") else {
            panic!("empty token should be refused");
        };
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn latest_release_tag_returns_tag_name() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/actions/checkout/releases/latest");
            then.status(200).json_body(json!({ "tag_name": "v7.0.1" }));
        });
        let tag = client(&server)
            .latest_release_tag("actions", "checkout")
            .expect("tag");
        mock.assert();
        assert_eq!(tag, "v7.0.1");
    }

    #[test]
    fn latest_release_tag_500_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/actions/checkout/releases/latest");
            then.status(500);
        });
        let err = client(&server)
            .latest_release_tag("actions", "checkout")
            .expect_err("500");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn latest_release_tag_rejects_a_non_pin() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/actions/checkout/releases/latest");
            then.status(200)
                .json_body(json!({ "tag_name": "v7.0.1@evil" }));
        });
        let err = client(&server)
            .latest_release_tag("actions", "checkout")
            .expect_err("pin");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn action_ref_rejects_dotdot_and_leading_dash() {
        assert!(super::action_ref("v7.0.1").is_ok());
        assert!(super::action_ref("v7").is_ok());
        assert!(super::action_ref("..").is_err());
        assert!(super::action_ref("-v7").is_err());
        assert!(super::action_ref("--").is_err());
    }

    #[test]
    fn create_commit_posts_the_mutation_and_returns_the_oid() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/graphql")
                .header("authorization", "Bearer token")
                .header("user-agent", "oakum")
                .body_includes("createCommitOnBranch")
                .body_includes("deadbeef")
                .body_includes("CHANGELOG.md")
                .body_includes("bm90ZXM=");
            then.status(200).json_body(json!({
                "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
            }));
        });

        let commit = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "main",
                "deadbeef",
                "feat(cli): bump",
                FileChanges {
                    additions: &[FileAddition {
                        path: String::from("CHANGELOG.md"),
                        contents_base64: String::from("bm90ZXM="),
                    }],
                    deletions: &[],
                },
            )
            .expect("commit");

        mock.assert();
        assert_eq!(
            commit,
            CreatedCommit {
                oid: String::from("abc123")
            }
        );
    }

    #[test]
    fn graphql_errors_are_not_a_commit() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/graphql");
            then.status(200).json_body(json!({
                "errors": [{ "message": "expectedHeadOid mismatch" }]
            }));
        });

        let err = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "main",
                "old",
                "msg",
                FileChanges {
                    additions: &[],
                    deletions: &[],
                },
            )
            .expect_err("graphql");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
        assert!(
            err.to_string().contains("expectedHeadOid mismatch"),
            "{err}"
        );
    }

    #[test]
    fn encode_path_segment_percent_encodes_slash_and_at() {
        assert_eq!(super::encode_path_segment("oakum/v0.1.0"), "oakum%2Fv0.1.0");
        assert_eq!(super::encode_path_segment("demo@1.0.0"), "demo%401.0.0");
        assert_eq!(super::encode_path_segment("v0.1.0"), "v0.1.0");
    }

    #[test]
    fn release_for_tag_empty_is_a_completed_look() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/releases/tags/oakum%2Fv0.1.0");
            then.status(404).body("Not Found");
        });
        let look = client(&server)
            .release_for_tag("oakoss", "oakum", "oakum/v0.1.0")
            .expect("lookup");
        mock.assert();
        assert_eq!(look, Look::Empty);
    }

    #[test]
    fn release_for_tag_found_reads_html_url_and_response_tag_name() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/releases/tags/v0.1.0");
            then.status(200).json_body(json!({
                "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.0",
                "tag_name": "v0.1.0"
            }));
        });
        let look = client(&server)
            .release_for_tag("oakoss", "oakum", "v0.1.0")
            .expect("lookup");
        mock.assert();
        assert_eq!(
            look,
            Look::Found(CreatedRelease {
                html_url: String::from("https://github.com/oakoss/oakum/releases/tag/v0.1.0"),
                tag_name: String::from("v0.1.0"),
            })
        );
    }

    #[test]
    fn release_for_tag_without_html_url_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/releases/tags/v0.1.0");
            then.status(200).json_body(json!({ "tag_name": "v0.1.0" }));
        });
        let err = client(&server)
            .release_for_tag("oakoss", "oakum", "v0.1.0")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("html_url"), "{err}");
    }

    #[test]
    fn release_for_tag_empty_html_url_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/releases/tags/v0.1.0");
            then.status(200).json_body(json!({
                "html_url": "",
                "tag_name": "v0.1.0"
            }));
        });
        let err = client(&server)
            .release_for_tag("oakoss", "oakum", "v0.1.0")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("html_url"), "{err}");
    }

    /// An expired token leaves the question as unread as a 502 does, so the
    /// auth statuses agree with it on the `unverified` class — while a 422 is
    /// GitHub refusing a request it read, which stays a plain error.
    #[test]
    fn release_for_tag_auth_failures_are_unverified() {
        for status in [401u16, 403] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(GET)
                    .path("/repos/oakoss/oakum/releases/tags/v0.1.0");
                then.status(status)
                    .json_body(json!({ "message": "denied" }));
            });
            let err = client(&server)
                .release_for_tag("oakoss", "oakum", "v0.1.0")
                .expect_err("an auth failure is not a verdict");
            assert!(
                matches!(err, super::Error::Unverified { .. }),
                "{status}: {err:?}"
            );
        }

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/releases/tags/v0.1.0");
            then.status(422)
                .json_body(json!({ "message": "unprocessable" }));
        });
        let err = client(&server)
            .release_for_tag("oakoss", "oakum", "v0.1.0")
            .expect_err("a refused request is not a verdict either");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
    }

    #[test]
    fn create_release_posts_the_tag_and_returns_the_url() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/releases")
                .body_includes("\"tag_name\":\"oakum-v0.1.0\"")
                .body_includes("\"name\":\"oakum 0.1.0\"")
                .body_includes("\"body\":\"notes\"");
            then.status(201).json_body(json!({
                "html_url": "https://github.com/oakoss/oakum/releases/tag/oakum-v0.1.0",
                "tag_name": "oakum-v0.1.0"
            }));
        });

        let release = client(&server)
            .create_release("oakoss", "oakum", "oakum-v0.1.0", "oakum 0.1.0", "notes")
            .expect("release");

        mock.assert();
        assert_eq!(
            release,
            CreatedRelease {
                html_url: String::from("https://github.com/oakoss/oakum/releases/tag/oakum-v0.1.0"),
                tag_name: String::from("oakum-v0.1.0"),
            }
        );
    }

    #[test]
    fn check_runs_empty_is_a_completed_look() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs")
                .query_param("per_page", "100");
            then.status(200)
                .json_body(json!({ "total_count": 0, "check_runs": [] }));
        });

        let look = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect("look");
        assert_eq!(look, Look::Empty);
    }

    #[test]
    fn check_runs_returns_the_named_runs() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs")
                .query_param("per_page", "100");
            then.status(200).json_body(json!({
                "total_count": 1,
                "check_runs": [{
                    "name": "Tests",
                    "status": "completed",
                    "conclusion": "success"
                }]
            }));
        });

        let look = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect("look");
        assert_eq!(
            look,
            Look::Found(vec![CheckRun {
                name: String::from("Tests"),
                status: String::from("completed"),
                conclusion: Some(String::from("success")),
            }])
        );
    }

    #[test]
    fn workflow_runs_empty_is_not_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/actions/runs")
                .query_param("head_sha", "abc")
                .query_param("per_page", "100");
            then.status(200)
                .header("etag", "\"w1\"")
                .json_body(json!({ "total_count": 0, "workflow_runs": [] }));
        });

        let (refresh, etag) = client(&server)
            .workflow_runs("oakoss", "oakum", "abc", None)
            .expect("refresh");
        assert_eq!(refresh, Refresh::Fresh(Look::Empty));
        assert_eq!(etag.as_deref(), Some("\"w1\""));
    }

    #[test]
    fn workflow_runs_sends_if_none_match_and_treats_304_as_not_modified() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/actions/runs")
                .header("if-none-match", "\"w1\"");
            then.status(304);
        });

        let (refresh, etag) = client(&server)
            .workflow_runs("oakoss", "oakum", "abc", Some("\"w1\""))
            .expect("refresh");
        mock.assert();
        assert_eq!(refresh, Refresh::NotModified);
        assert_eq!(etag.as_deref(), Some("\"w1\""));
    }

    #[test]
    fn workflow_runs_returns_the_run() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/actions/runs")
                .query_param("head_sha", "abc")
                .query_param("per_page", "100");
            then.status(200).json_body(json!({
                "total_count": 1,
                "workflow_runs": [{
                    "id": 9,
                    "head_sha": "abc",
                    "status": "completed",
                    "conclusion": "success"
                }]
            }));
        });

        let (refresh, _) = client(&server)
            .workflow_runs("oakoss", "oakum", "abc", None)
            .expect("refresh");
        assert_eq!(
            refresh,
            Refresh::Fresh(Look::Found(vec![WorkflowRun {
                id: 9,
                head_sha: String::from("abc"),
                head_branch: None,
                status: String::from("completed"),
                conclusion: Some(String::from("success")),
                path: None,
                event: None,
                html_url: String::new(),
            }]))
        );
    }

    #[test]
    fn a_server_error_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(502).body("bad gateway");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_transport_failure_is_unverified() {
        let err = Client::at("http://127.0.0.1:1", "token")
            .expect("client")
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_missing_ref_is_not_empty_or_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(404).body("Not Found");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("other");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
    }

    #[test]
    fn an_unreadable_check_runs_body_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(200).json_body(json!({}));
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_rate_limit_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(429).body("slow down");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_forbidden_rate_limit_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(403)
                .header("x-ratelimit-remaining", "0")
                .body("rate limited");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_secondary_rate_limit_body_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(403)
                .body("You have exceeded a secondary rate limit. Please wait.");
        });

        let err = client(&server)
            .issue_comments("oakoss", "oakum", 4)
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(!err.is_forbidden(), "{err:?}");
    }

    #[test]
    fn a_forbidden_ref_is_not_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(403).body("no access");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("forbidden");
        assert!(err.is_forbidden(), "{err:?}");
        assert!(!matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn an_unreadable_workflow_runs_body_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/oakoss/oakum/actions/runs");
            then.status(200).json_body(json!({}));
        });

        let err = client(&server)
            .workflow_runs("oakoss", "oakum", "abc", None)
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_non_json_success_body_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(200).body("not-json");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn create_commit_without_oid_is_not_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/graphql");
            then.status(200)
                .json_body(json!({ "data": { "createCommitOnBranch": { "commit": {} } } }));
        });

        let err = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "main",
                "old",
                "msg",
                FileChanges {
                    additions: &[],
                    deletions: &[],
                },
            )
            .expect_err("other");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
    }

    #[test]
    fn create_release_without_html_url_is_not_a_release() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/releases");
            then.status(201)
                .json_body(json!({ "tag_name": "oakum-v0.1.0" }));
        });

        let err = client(&server)
            .create_release("oakoss", "oakum", "oakum-v0.1.0", "oakum 0.1.0", "notes")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("html_url"), "{err}");
    }

    #[test]
    fn create_release_empty_html_url_is_not_a_release() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/releases");
            then.status(201).json_body(json!({
                "tag_name": "oakum-v0.1.0",
                "html_url": ""
            }));
        });

        let err = client(&server)
            .create_release("oakoss", "oakum", "oakum-v0.1.0", "oakum 0.1.0", "notes")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("html_url"), "{err}");
    }

    #[test]
    fn check_runs_encodes_a_slash_in_the_ref() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/feature%2Ffoo/check-runs");
            then.status(200)
                .json_body(json!({ "total_count": 0, "check_runs": [] }));
        });

        let look = client(&server)
            .check_runs("oakoss", "oakum", "feature/foo")
            .expect("look");
        mock.assert();
        assert_eq!(look, Look::Empty);
    }

    #[test]
    fn an_incomplete_check_runs_page_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(200).json_body(json!({
                "total_count": 2,
                "check_runs": [{
                    "name": "Tests",
                    "status": "completed",
                    "conclusion": "success"
                }]
            }));
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn an_incomplete_workflow_runs_page_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/oakoss/oakum/actions/runs");
            then.status(200).json_body(json!({
                "total_count": 2,
                "workflow_runs": [{
                    "id": 9,
                    "head_sha": "abc",
                    "status": "completed",
                    "conclusion": "success"
                }]
            }));
        });

        let err = client(&server)
            .workflow_runs("oakoss", "oakum", "abc", None)
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn a_forbidden_retry_after_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(403)
                .header("retry-after", "1")
                .body("slow down");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn create_commit_sends_deletions() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/graphql")
                .body_includes(r#""deletions":[{"path":".changeset/one.md"}]"#)
                .body_includes(r#""additions":[]"#);
            then.status(200).json_body(json!({
                "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
            }));
        });

        client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[],
                    deletions: &[FileDeletion::new(".changeset/one.md").expect("path")],
                },
            )
            .expect("commit");
        mock.assert();
    }

    #[test]
    fn create_commit_rejects_overlap_and_empty_paths() {
        let server = MockServer::start();
        let overlap = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[FileAddition::from_text("same.md", "notes")],
                    deletions: &[FileDeletion::new("same.md").expect("path")],
                },
            )
            .expect_err("overlap");
        assert!(
            overlap
                .to_string()
                .contains("both additions and deletions: same.md"),
            "{overlap}"
        );

        let empty = FileDeletion::new("").expect_err("empty");
        assert!(empty.to_string().contains("empty"), "{empty}");

        let alias = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[FileAddition::from_text("./same.md", "notes")],
                    deletions: &[FileDeletion::new("same.md").expect("path")],
                },
            )
            .expect_err("alias overlap");
        assert!(
            alias
                .to_string()
                .contains("both additions and deletions: same.md")
                || alias
                    .to_string()
                    .contains("both additions and deletions: ./same.md"),
            "{alias}"
        );

        let inner = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[FileAddition::from_text("foo/./same.md", "notes")],
                    deletions: &[FileDeletion::new("foo/same.md").expect("path")],
                },
            )
            .expect_err("inner alias");
        assert!(
            inner.to_string().contains("both additions and deletions"),
            "{inner}"
        );

        let dup = client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[
                        FileAddition::from_text("./same.md", "a"),
                        FileAddition::from_text("same.md", "b"),
                    ],
                    deletions: &[],
                },
            )
            .expect_err("dup addition");
        assert!(dup.to_string().contains("duplicated"), "{dup}");
    }

    #[test]
    fn create_commit_sends_normalized_paths() {
        let server = MockServer::start();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let writer = captured.clone();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/graphql").is_true(move |req| {
                *writer.lock().expect("body") = req.body_string();
                true
            });
            then.status(200).json_body(json!({
                "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
            }));
        });

        client(&server)
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[FileAddition::from_text("./same.md", "notes")],
                    deletions: &[FileDeletion::new("./gone.md").expect("path")],
                },
            )
            .expect("commit");
        mock.assert();

        let body: serde_json::Value =
            serde_json::from_str(&captured.lock().expect("body")).expect("graphql json");
        let files = &body["variables"]["input"]["fileChanges"];
        assert_eq!(
            files["additions"],
            json!([{ "path": "same.md", "contents": "bm90ZXM=" }])
        );
        assert_eq!(files["deletions"], json!([{ "path": "gone.md" }]));
    }

    #[test]
    fn empty_branch_sha_is_an_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/main");
            then.status(200)
                .json_body(json!({ "object": { "sha": "" } }));
        });

        let err = client(&server)
            .branch_head("oakoss", "oakum", "main")
            .expect_err("empty sha");
        assert!(err.to_string().contains("empty object.sha"), "{err}");
    }

    #[test]
    fn whitespace_branch_sha_is_an_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/main");
            then.status(200)
                .json_body(json!({ "object": { "sha": " " } }));
        });

        let err = client(&server)
            .branch_head("oakoss", "oakum", "main")
            .expect_err("whitespace sha");
        assert!(err.to_string().contains("empty object.sha"), "{err}");
    }

    #[test]
    fn empty_default_branch_is_an_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/oakoss/oakum");
            then.status(200).json_body(json!({ "default_branch": "" }));
        });

        let err = client(&server)
            .default_branch("oakoss", "oakum")
            .expect_err("empty default");
        assert!(err.to_string().contains("default_branch"), "{err}");
    }

    #[test]
    fn graphql_uses_the_v3_enterprise_path() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/graphql")
                .body_includes("createCommitOnBranch");
            then.status(200).json_body(json!({
                "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
            }));
        });
        let rest = server.mock(|when, then| {
            when.method(POST).path("/api/v3/graphql");
            then.status(404).body("not graphql");
        });

        super::Client::at(format!("{}/api/v3", server.base_url()), "token")
            .expect("client")
            .create_commit_on_branch(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "Version Packages",
                FileChanges {
                    additions: &[],
                    deletions: &[],
                },
            )
            .expect("commit");
        mock.assert();
        rest.assert_calls(0);
    }

    #[test]
    fn graphql_prefers_an_explicit_graphql_url() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/custom-graphql")
                .body_includes("createCommitOnBranch");
            then.status(200).json_body(json!({
                "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
            }));
        });
        let derived = server.mock(|when, then| {
            when.method(POST).path("/api/graphql");
            then.status(404).body("derived");
        });

        super::Client::at_urls(
            format!("{}/api/v3", server.base_url()),
            format!("{}/custom-graphql", server.base_url()),
            "token",
        )
        .expect("client")
        .create_commit_on_branch(
            "oakoss",
            "oakum",
            "oakum/version-packages",
            "deadbeef",
            "Version Packages",
            FileChanges {
                additions: &[],
                deletions: &[],
            },
        )
        .expect("commit");
        mock.assert();
        derived.assert_calls(0);
    }

    #[test]
    fn update_pull_returns_number_and_url() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/pulls/7")
                .body_includes("\"title\":\"Version Packages\"")
                .body_includes("Generated by oakum 0.0.0.");
            then.status(200).json_body(json!({
                "number": 7,
                "html_url": "https://github.com/oakoss/oakum/pull/7"
            }));
        });

        let pull = client(&server)
            .update_pull(
                "oakoss",
                "oakum",
                7,
                "Version Packages",
                "Generated by oakum 0.0.0.",
            )
            .expect("update");
        mock.assert();
        assert_eq!(
            pull,
            super::PullRequest {
                number: 7,
                html_url: String::from("https://github.com/oakoss/oakum/pull/7"),
            }
        );
    }

    #[test]
    fn file_addition_from_text_is_standard_base64() {
        let file = FileAddition::from_text("CHANGELOG.md", "notes");
        assert_eq!(file.contents_base64, "bm90ZXM=");
    }

    #[test]
    fn missing_branch_is_an_empty_look() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(404).body("not found");
        });

        let look = client(&server)
            .branch_head("oakoss", "oakum", "oakum/version-packages")
            .expect("look");
        assert_eq!(look, Look::Empty);
    }

    #[test]
    fn point_branch_creates_when_missing() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(404).body("not found");
        });
        let created = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/git/refs")
                .body_includes("refs/heads/oakum/version-packages")
                .body_includes("deadbeef");
            then.status(201).json_body(json!({
                "ref": "refs/heads/oakum/version-packages",
                "object": { "sha": "deadbeef" }
            }));
        });

        client(&server)
            .point_branch("oakoss", "oakum", "oakum/version-packages", "deadbeef")
            .expect("create");
        created.assert();
    }

    #[test]
    fn point_branch_force_updates_when_present() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(200).json_body(json!({
                "object": { "sha": "old" }
            }));
        });
        let updated = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages")
                .body_includes("\"force\":true")
                .body_includes("deadbeef");
            then.status(200).json_body(json!({
                "object": { "sha": "deadbeef" }
            }));
        });

        client(&server)
            .point_branch("oakoss", "oakum", "oakum/version-packages", "deadbeef")
            .expect("update");
        updated.assert();
    }

    #[test]
    fn create_pull_returns_number_and_url() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/pulls")
                .body_includes("\"head\":\"oakum/version-packages\"")
                .body_includes("\"base\":\"main\"");
            then.status(201).json_body(json!({
                "number": 12,
                "html_url": "https://github.com/oakoss/oakum/pull/12"
            }));
        });

        let pull = client(&server)
            .create_pull(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "main",
                "Version Packages",
                "Generated by oakum 0.0.0.",
            )
            .expect("create");
        mock.assert();
        assert_eq!(
            pull,
            super::PullRequest {
                number: 12,
                html_url: String::from("https://github.com/oakoss/oakum/pull/12"),
            }
        );
    }

    #[test]
    fn open_pulls_empty_is_a_completed_look() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/pulls")
                .query_param("head", "oakoss:oakum/version-packages")
                .query_param("state", "open")
                .query_param("per_page", "100");
            then.status(200).json_body(json!([]));
        });

        let look = client(&server)
            .open_pulls_for_head("oakoss", "oakum", "oakum/version-packages")
            .expect("look");
        assert_eq!(look, Look::Empty);
    }

    #[test]
    fn a_full_pulls_page_is_unverified() {
        let server = MockServer::start();
        let pulls = (1..=100)
            .map(|number| {
                json!({
                    "number": number,
                    "html_url": format!("https://github.com/oakoss/oakum/pull/{number}")
                })
            })
            .collect::<Vec<_>>();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/pulls")
                .query_param("head", "oakoss:oakum/version-packages")
                .query_param("state", "open")
                .query_param("per_page", "100");
            then.status(200).json_body(json!(pulls));
        });

        let err = client(&server)
            .open_pulls_for_head("oakoss", "oakum", "oakum/version-packages")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
    }

    #[test]
    fn encode_base64_pads_one_and_two_remainders() {
        assert_eq!(super::encode_base64(b"n"), "bg==");
        assert_eq!(super::encode_base64(b"no"), "bm8=");
        assert_eq!(super::encode_base64(b"notes"), "bm90ZXM=");
    }

    #[test]
    fn upsert_creates_when_no_marker_comment_exists() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!([
                { "id": 1, "body": "human note", "user": { "login": "alice" } }
            ]));
        });
        let created = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/issues/4/comments")
                .body_includes("<!-- oakum:pr-plan -->");
            then.status(201).json_body(json!({ "id": 9 }));
        });

        let id = client(&server)
            .upsert_plan_comment(
                "oakoss",
                "oakum",
                4,
                "<!-- oakum:pr-plan -->",
                "<!-- oakum:pr-plan -->\nplan\n",
            )
            .expect("create");
        created.assert();
        assert_eq!(id, 9);
    }

    #[test]
    fn upsert_updates_newest_and_deletes_the_rest() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!([
                { "id": 2, "body": "<!-- oakum:pr-plan -->\nold", "user": { "login": "github-actions[bot]" } },
                { "id": 5, "body": "<!-- oakum:pr-plan -->\nnewer", "user": { "login": "github-actions[bot]" } },
                { "id": 9, "body": "<!-- oakum:pr-plan -->\nquoted", "user": { "login": "alice" } }
            ]));
        });
        let deleted = server.mock(|when, then| {
            when.method(DELETE)
                .path("/repos/oakoss/oakum/issues/comments/2");
            then.status(204).body("");
        });
        let updated = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/issues/comments/5")
                .body_includes("current");
            then.status(200).json_body(json!({ "id": 5 }));
        });

        let id = client(&server)
            .upsert_plan_comment(
                "oakoss",
                "oakum",
                4,
                "<!-- oakum:pr-plan -->",
                "<!-- oakum:pr-plan -->\ncurrent\n",
            )
            .expect("update");
        deleted.assert();
        updated.assert();
        assert_eq!(id, 5);
    }

    #[test]
    fn a_401_body_that_mentions_403_is_not_forbidden() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(401)
                .body("GitHub /repos/oakoss/oakum/issues/4/comments returned 403");
        });

        let err = client(&server)
            .issue_comments("oakoss", "oakum", 4)
            .expect_err("unauthorized");
        assert!(!err.is_forbidden(), "{err:?}");
        assert!(matches!(err, super::Error::Unauthorized { .. }), "{err:?}");
    }

    #[test]
    fn a_forbidden_comment_list_is_forbidden() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(403).body("read-only token");
        });

        let err = client(&server)
            .issue_comments("oakoss", "oakum", 4)
            .expect_err("forbidden");
        assert!(err.is_forbidden(), "{err:?}");
    }

    #[test]
    fn issue_comments_walks_pages_until_a_short_page() {
        let server = MockServer::start();
        let first = (1..=100)
            .map(|id| {
                json!({
                    "id": id,
                    "body": "other",
                    "user": { "login": "alice" }
                })
            })
            .collect::<Vec<_>>();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments")
                .query_param("per_page", "100")
                .query_param("page", "1");
            then.status(200).json_body(json!(first));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments")
                .query_param("per_page", "100")
                .query_param("page", "2");
            then.status(200).json_body(json!([
                { "id": 101, "body": "last", "user": { "login": "alice" } }
            ]));
        });

        let look = client(&server)
            .issue_comments("oakoss", "oakum", 4)
            .expect("pages");
        let Look::Found(comments) = look else {
            panic!("expected comments");
        };
        assert_eq!(comments.len(), 101);
        assert_eq!(comments[100].id, 101);
    }

    #[test]
    fn issue_comments_walks_past_ten_full_pages() {
        let server = MockServer::start();
        for page in 1u32..=10 {
            let batch = ((page - 1) * 100 + 1..=page * 100)
                .map(|id| {
                    json!({
                        "id": id,
                        "body": "other",
                        "user": { "login": "alice" }
                    })
                })
                .collect::<Vec<_>>();
            server.mock(|when, then| {
                when.method(GET)
                    .path("/repos/oakoss/oakum/issues/4/comments")
                    .query_param("per_page", "100")
                    .query_param("page", page.to_string());
                then.status(200).json_body(json!(batch));
            });
        }
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments")
                .query_param("per_page", "100")
                .query_param("page", "11");
            then.status(200).json_body(json!([
                { "id": 1001, "body": "last", "user": { "login": "alice" } }
            ]));
        });

        let look = client(&server)
            .issue_comments("oakoss", "oakum", 4)
            .expect("pages");
        let Look::Found(comments) = look else {
            panic!("expected comments");
        };
        assert_eq!(comments.len(), 1001);
        assert_eq!(comments[1000].id, 1001);
    }

    #[test]
    fn a_full_comment_page_budget_is_unverified() {
        let server = MockServer::start();
        let batch = (1..=100)
            .map(|id| {
                json!({
                    "id": id,
                    "body": "other",
                    "user": { "login": "alice" }
                })
            })
            .collect::<Vec<_>>();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!(batch));
        });

        let err = client(&server)
            .issue_comments("oakoss", "oakum", 4)
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("2000/unknown"), "{err}");
    }

    #[test]
    fn a_null_comment_user_is_skipped() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!([
                { "id": 2, "body": "<!-- oakum:pr-plan -->\nplan", "user": { "login": "github-actions[bot]" } },
                { "id": 9, "body": "<!-- oakum:pr-plan -->\nquoted", "user": null }
            ]));
        });
        let updated = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/issues/comments/2");
            then.status(200).json_body(json!({ "id": 2 }));
        });
        let ghost = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/issues/comments/9");
            then.status(200).json_body(json!({ "id": 9 }));
        });

        let id = client(&server)
            .upsert_plan_comment(
                "oakoss",
                "oakum",
                4,
                "<!-- oakum:pr-plan -->",
                "<!-- oakum:pr-plan -->\ncurrent\n",
            )
            .expect("update");
        updated.assert();
        ghost.assert_calls(0);
        assert_eq!(id, 2);
    }

    #[test]
    fn a_null_user_plan_comment_is_not_owned() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!([
                { "id": 9, "body": "<!-- oakum:pr-plan -->\nquoted", "user": null }
            ]));
        });
        let created = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(201).json_body(json!({ "id": 11 }));
        });
        let patched = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/issues/comments/9");
            then.status(200).json_body(json!({ "id": 9 }));
        });

        let id = client(&server)
            .upsert_plan_comment(
                "oakoss",
                "oakum",
                4,
                "<!-- oakum:pr-plan -->",
                "<!-- oakum:pr-plan -->\ncurrent\n",
            )
            .expect("create");
        created.assert();
        patched.assert_calls(0);
        assert_eq!(id, 11);
    }

    #[test]
    fn owned_plan_comments_drops_an_empty_user() {
        let comments = Look::Found(vec![super::IssueComment {
            id: 9,
            body: String::from("<!-- oakum:pr-plan -->\nquoted"),
            user: String::new(),
        }]);
        assert!(super::owned_plan_comments(comments, "<!-- oakum:pr-plan -->").is_empty());
    }

    #[test]
    fn delete_plan_comments_skips_a_human_quote() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!([
                { "id": 2, "body": "<!-- oakum:pr-plan -->\nold", "user": { "login": "github-actions[bot]" } },
                { "id": 9, "body": "<!-- oakum:pr-plan -->\nquoted", "user": { "login": "alice" } }
            ]));
        });
        let deleted = server.mock(|when, then| {
            when.method(DELETE)
                .path("/repos/oakoss/oakum/issues/comments/2");
            then.status(204).body("");
        });
        let human = server.mock(|when, then| {
            when.method(DELETE)
                .path("/repos/oakoss/oakum/issues/comments/9");
            then.status(204).body("");
        });

        client(&server)
            .delete_plan_comments("oakoss", "oakum", 4, "<!-- oakum:pr-plan -->")
            .expect("delete");
        deleted.assert();
        human.assert_calls(0);
    }
}
