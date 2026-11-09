//! GitHub HTTP (okm-dlo). Lives in the binary, which owns network I/O; the
//! library stays free of it (ADR-0002). No octocrab.
//!
//! Tag push is git, not this client ([github-release-path.md](../../../../docs/research/github-release-path.md)).

use std::collections::HashSet;
use std::fmt::{self, Write};

use reqwest::blocking::Client as Http;
use reqwest::header::{HeaderMap, IF_NONE_MATCH};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};

const USER_AGENT: &str = "oakum";

pub(crate) struct Client {
    http: Http,
    api: String,
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

/// The token from the environment, if either name carries a non-empty one.
/// Callers name the command that needs it, so the remedy is specific.
pub(crate) fn token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|token| !token.is_empty())
}

/// An action's latest release and the commit it tags, for a printed workflow
/// to pin by SHA. A baked-in major goes stale.
pub(crate) struct ReleasePin {
    pub(crate) commit: String,
    pub(crate) tag: String,
}

pub(crate) fn latest_release_pin(owner: &str, repo: &str) -> Result<ReleasePin, Error> {
    let client = Client::public()?;
    let tag = client.latest_release_tag(owner, repo)?;
    let commit = client.tag_commit(owner, repo, &tag)?;
    Ok(ReleasePin { commit, tag })
}

fn is_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
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
    pub merged: bool,
    /// Absent when the response omitted `user`; GitHub always sends it, and a
    /// report built on it says so rather than inventing an identity.
    pub author: Option<PullAuthor>,
}

/// Who opened a pull request. `kind` is GitHub's `type` — `Bot` or `User` —
/// and `None` when the response did not carry one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PullAuthor {
    pub login: Option<String>,
    pub kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: String,
}

/// The plan comment that is up, and whether its older copies went. Kept apart
/// so a failed removal is never reported as a plan that was not posted.
#[derive(Debug)]
pub(crate) struct Upserted {
    pub cleanup: Result<(), Error>,
}

const COMMENT_PAGES: u32 = 20;

fn is_owned_plan_author(login: &str) -> bool {
    login.ends_with("[bot]")
}

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
pub(crate) struct WorkflowRun {
    pub id: u64,
    pub head_sha: String,
    /// For push- and create-event runs this is the ref's short name — a tag
    /// run carries the tag name, not a branch (measured: push okm-e9e.17,
    /// create okm-8gh).
    pub head_branch: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub path: Option<String>,
    pub event: Option<String>,
    #[serde(default)]
    pub html_url: String,
}

/// Where a compare's head commit stands against its base, as GitHub's
/// `status` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Comparison {
    /// Head descends from base.
    Ahead,
    Behind,
    Diverged,
    Identical,
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

    #[cfg(test)]
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
        Self::at_url(api, token)
    }

    /// `pub(super)` is the unit seam: without it every handoff assertion
    /// costs a spawned binary, a temp repo, and a bare origin.
    #[cfg(test)]
    pub(super) fn at(api: impl Into<String>, token: impl Into<String>) -> Result<Self, Error> {
        Self::at_url(api, token)
    }

    fn at_url(api: impl Into<String>, token: impl Into<String>) -> Result<Self, Error> {
        let token = token.into();
        if token.is_empty() {
            return Err(Error::new("GitHub token is empty"));
        }
        let http = Http::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| Error::new(with_causes(&err)))?;
        Ok(Self {
            http,
            api: api.into().trim_end_matches('/').to_owned(),
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
        let http = Http::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| Error::new(with_causes(&err)))?;
        Ok(Self {
            http,
            api: api.trim_end_matches('/').to_owned(),
            token,
        })
    }

    pub(crate) fn latest_release_tag(&self, owner: &str, repo: &str) -> Result<String, Error> {
        let path = format!(
            "/repos/{}/{}/releases/latest",
            path_segment(owner),
            path_segment(repo)
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

    /// The commit `tag` names. The commits endpoint peels an annotated tag to
    /// its commit, where `git/ref` answers with the tag object; `refs/tags/`
    /// keeps a branch of the same name from answering instead.
    pub(crate) fn tag_commit(&self, owner: &str, repo: &str, tag: &str) -> Result<String, Error> {
        let path = format!(
            "/repos/{}/{}/commits/refs/tags/{}",
            path_segment(owner),
            path_segment(repo),
            path_segment(tag)
        );
        let Some(value) = self.json_or_missing(reqwest::Method::GET, &path, None)? else {
            return Err(Error::unverified(format!(
                "unverified: GitHub {path} returned 404"
            )));
        };
        value
            .get("sha")
            .and_then(Value::as_str)
            .filter(|sha| is_commit_sha(sha))
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::unverified(format!("unverified: GitHub {path} named no commit SHA"))
            })
    }

    /// Publishes one commit on `branch` with parent `parent_oid`, updating the
    /// ref once to the new tip so an open pull request never sees head equal base.
    pub(crate) fn replace_branch_commit(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        parent_oid: &str,
        headline: &str,
        files: FileChanges<'_>,
    ) -> Result<CreatedCommit, Error> {
        let files = files.validate()?;
        let base_tree = self.commit_tree_sha(owner, repo, parent_oid)?;
        let mut tree = Vec::with_capacity(files.additions.len() + files.deletions.len());
        for file in files.additions {
            let path = git_path(&file.path, "addition")?;
            let blob = self.create_blob(owner, repo, &file.contents_base64)?;
            tree.push(json!({
                "path": path,
                "mode": "100644",
                "type": "blob",
                "sha": blob,
            }));
        }
        for file in files.deletions {
            tree.push(json!({
                "path": git_path(&file.path, "deletion")?,
                "mode": "100644",
                "type": "blob",
                "sha": Value::Null,
            }));
        }
        let tree_sha = self.create_tree(owner, repo, &base_tree, &tree)?;
        let commit_sha = self.create_git_commit(owner, repo, &tree_sha, headline, parent_oid)?;
        match self.branch_head(owner, repo, branch)? {
            Look::Empty => {
                let payload = json!({
                    "ref": format!("refs/heads/{branch}"),
                    "sha": commit_sha,
                });
                self.json(
                    reqwest::Method::POST,
                    &format!("/repos/{owner}/{repo}/git/refs"),
                    Some(&payload),
                )?;
            }
            Look::Found(_) => {
                let payload = json!({ "sha": commit_sha, "force": true });
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
        Ok(CreatedCommit { oid: commit_sha })
    }

    pub(crate) fn create_release(
        &self,
        owner: &str,
        repo: &str,
        tag_name: &str,
        name: &str,
        body: &str,
        latest: bool,
    ) -> Result<CreatedRelease, Error> {
        // A string, not a bool, in GitHub's API; absent means "true".
        let payload = json!({
            "tag_name": tag_name,
            "name": name,
            "body": body,
            "make_latest": if latest { "true" } else { "false" },
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
            path_segment(tag_name)
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

    /// The tag of the release GitHub shows as Latest; `Empty` when none is.
    pub(crate) fn latest_release_holder(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Look<String>, Error> {
        let path = format!("/repos/{owner}/{repo}/releases/latest");
        match looked(self.json_or_missing(reqwest::Method::GET, &path, None))? {
            None => Ok(Look::Empty),
            Some(value) => value
                .get("tag_name")
                .and_then(Value::as_str)
                .filter(|tag| !tag.is_empty())
                .map(|tag| Look::Found(tag.to_owned()))
                .ok_or_else(|| Error::new("latest release lookup returned no tag_name")),
        }
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
        let (status, headers, value) = self.send(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}/actions/runs?head_sha={head_sha}&per_page=100"),
            None,
            etag,
            false,
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

    /// Empty when GitHub does not have one of the commits.
    pub(crate) fn compare(
        &self,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<Look<Comparison>, Error> {
        #[derive(Deserialize)]
        struct Payload {
            status: Comparison,
        }
        let path = format!(
            "/repos/{owner}/{repo}/compare/{}...{}?per_page=1",
            path_segment(base),
            path_segment(head)
        );
        let Some(value) = looked(self.json_or_missing(reqwest::Method::GET, &path, None))? else {
            return Ok(Look::Empty);
        };
        let payload: Payload = serde_json::from_value(value)
            .map_err(|err| Error::unverified(format!("unverified: compare body: {err}")))?;
        Ok(Look::Found(payload.status))
    }

    pub(crate) fn pulls_for_head(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        state: &str,
    ) -> Result<Look<Vec<PullRequest>>, Error> {
        let head = format!("{owner}:{branch}");
        let path = format!(
            "/repos/{owner}/{repo}/pulls?head={}&state={}&per_page=100",
            path_segment(&head),
            path_segment(state)
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
                    merged: pull.merged_at.is_some(),
                    author: pull.user.map(|user| PullAuthor {
                        login: user.login,
                        kind: user.kind,
                    }),
                })
                .collect(),
        ))
    }

    pub(crate) fn open_pulls_for_head(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Look<Vec<PullRequest>>, Error> {
        self.pulls_for_head(owner, repo, branch, "open")
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
        // Reopen when a prior run left the version PR closed (e.g. before
        // replace_branch_commit stopped the transient auto-close).
        let payload = json!({ "title": title, "body": body, "state": "open" });
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
    ) -> Result<Upserted, Error> {
        let mut ours = owned_plan_comments(self.issue_comments(owner, repo, number)?, marker);
        // The plan goes up before the extras go, so a failed delete leaves the
        // current plan rather than a stale one or none.
        match ours.pop() {
            Some(comment) => self.update_issue_comment(owner, repo, comment.id, body)?,
            None => {
                self.create_issue_comment(owner, repo, number, body)?;
            }
        }
        let cleanup = ours
            .into_iter()
            .try_for_each(|stale| self.delete_issue_comment(owner, repo, stale.id));
        Ok(Upserted { cleanup })
    }

    /// The body of the comment [`Self::upsert_plan_comment`] would update.
    pub(crate) fn plan_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        marker: &str,
    ) -> Result<Option<String>, Error> {
        Ok(
            owned_plan_comments(self.issue_comments(owner, repo, number)?, marker)
                .pop()
                .map(|comment| comment.body),
        )
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
        let (status, _, value) = self.send(method, path, body, None, true)?;
        if status == StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let (_, _, value) = self.send(method, path, body, None, false)?;
        Ok(value)
    }

    fn commit_tree_sha(&self, owner: &str, repo: &str, commit_sha: &str) -> Result<String, Error> {
        let path = format!(
            "/repos/{owner}/{repo}/git/commits/{}",
            path_segment(commit_sha)
        );
        let value = self.json(reqwest::Method::GET, &path, None)?;
        value
            .pointer("/tree/sha")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|sha| !sha.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::new(format!("GitHub {path} omitted tree.sha")))
    }

    fn create_blob(&self, owner: &str, repo: &str, contents_base64: &str) -> Result<String, Error> {
        let path = format!("/repos/{owner}/{repo}/git/blobs");
        let payload = json!({
            "content": contents_base64,
            "encoding": "base64",
        });
        let value = self.json(reqwest::Method::POST, &path, Some(&payload))?;
        git_object_sha(&value, &path)
    }

    fn create_tree(
        &self,
        owner: &str,
        repo: &str,
        base_tree: &str,
        tree: &[Value],
    ) -> Result<String, Error> {
        let path = format!("/repos/{owner}/{repo}/git/trees");
        let payload = json!({ "base_tree": base_tree, "tree": tree });
        let value = self.json(reqwest::Method::POST, &path, Some(&payload))?;
        git_object_sha(&value, &path)
    }

    fn create_git_commit(
        &self,
        owner: &str,
        repo: &str,
        tree_sha: &str,
        headline: &str,
        parent_oid: &str,
    ) -> Result<String, Error> {
        let path = format!("/repos/{owner}/{repo}/git/commits");
        let payload = json!({
            "message": headline,
            "tree": tree_sha,
            "parents": [parent_oid],
        });
        let value = self.json(reqwest::Method::POST, &path, Some(&payload))?;
        git_object_sha(&value, &path)
    }

    /// `missing_ok` reads a 404 as an answer (`Value::Null`) rather than an error.
    fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
        missing_ok: bool,
    ) -> Result<(StatusCode, HeaderMap, Value), Error> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.api))
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
                "unverified: GitHub request to {path} failed: {}",
                with_causes(&err)
            ))
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let read_body = || response.text().map_err(|err| with_causes(&err));
        let value = classify(status, &headers, read_body, missing_ok, path)?;
        Ok((status, headers, value))
    }
}

#[derive(Deserialize)]
struct PullJson {
    number: u64,
    html_url: String,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    user: Option<PullUserJson>,
}

#[derive(Deserialize)]
struct PullUserJson {
    #[serde(default)]
    login: Option<String>,
    /// Absent in a response that sends `user` without `type`. Kept optional so
    /// the gap reaches the reader rather than defaulting into a verdict.
    #[serde(default, rename = "type")]
    kind: Option<String>,
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
            // oakum writes the marker as the first line; a comment that only
            // quotes it, even a bot's, is someone else's.
            .filter(|comment| {
                is_owned_plan_author(&comment.user) && comment.body.starts_with(marker)
            })
            .collect(),
    };
    ours.sort_by_key(|comment| comment.id);
    ours
}

fn git_object_sha(value: &Value, path: &str) -> Result<String, Error> {
    value
        .get("sha")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::new(format!("GitHub {path} omitted sha")))
}

fn pull_from_value(value: Value) -> Result<PullRequest, Error> {
    let pull: PullJson =
        serde_json::from_value(value).map_err(|err| Error::new(format!("pull body: {err}")))?;
    Ok(PullRequest {
        number: pull.number,
        html_url: pull.html_url,
        merged: pull.merged_at.is_some(),
        author: pull.user.map(|user| PullAuthor {
            login: user.login,
            kind: user.kind,
        }),
    })
}

fn env_url(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
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

/// `err` and each cause under it. reqwest's own message stops at "error
/// sending request"; why (refused, timed out) is only in the chain.
fn with_causes(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let said = cause.to_string();
        if !text.ends_with(&said) {
            text.push_str(": ");
            text.push_str(&said);
        }
        source = cause.source();
    }
    text
}

/// What a response from `path` means; `Value::Null` is no content. The body is
/// read only where a rule needs it, so a status that settles the answer does
/// not wait on its body.
fn classify(
    status: StatusCode,
    headers: &HeaderMap,
    read_body: impl FnOnce() -> Result<String, String>,
    missing_ok: bool,
    path: &str,
) -> Result<Value, Error> {
    if status == StatusCode::NOT_MODIFIED || (missing_ok && status == StatusCode::NOT_FOUND) {
        return Ok(Value::Null);
    }
    let unverified = || Error::unverified(format!("unverified: GitHub {path} returned {status}"));
    if status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN && rate_limited(headers))
    {
        return Err(unverified());
    }
    if status == StatusCode::FORBIDDEN {
        return Err(
            if secondary_rate_limited(&read_body().unwrap_or_default()) {
                unverified()
            } else {
                Error::forbidden(path)
            },
        );
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(Error::Unauthorized {
            path: path.to_owned(),
        });
    }
    if !status.is_success() {
        let body = read_body().unwrap_or_default();
        return Err(Error::new(format!(
            "GitHub {path} returned {status}: {body}"
        )));
    }
    let text = read_body()
        .map_err(|err| Error::unverified(format!("unverified: GitHub {path} body: {err}")))?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text)
        .map_err(|err| Error::unverified(format!("unverified: GitHub {path} JSON: {err}")))
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
        Client, Comparison, CreatedCommit, CreatedRelease, FileAddition, FileChanges, FileDeletion,
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

    fn compare_answering(
        server: &MockServer,
        status: u16,
        body: serde_json::Value,
    ) -> httpmock::Mock<'_> {
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/compare/aaa...bbb")
                .query_param("per_page", "1");
            then.status(status).json_body(body);
        })
    }

    #[test]
    fn compare_reads_each_status() {
        for (status, comparison) in [
            ("ahead", Comparison::Ahead),
            ("behind", Comparison::Behind),
            ("diverged", Comparison::Diverged),
            ("identical", Comparison::Identical),
        ] {
            let server = MockServer::start();
            compare_answering(&server, 200, json!({ "status": status }));
            let look = client(&server)
                .compare("oakoss", "oakum", "aaa", "bbb")
                .expect("compare");
            assert_eq!(look, Look::Found(comparison), "{status}");
        }
    }

    #[test]
    fn compare_of_a_commit_github_lacks_is_empty() {
        let server = MockServer::start();
        let mock = compare_answering(&server, 404, json!({ "message": "Not Found" }));
        let look = client(&server)
            .compare("oakoss", "oakum", "aaa", "bbb")
            .expect("compare");
        mock.assert();
        assert_eq!(look, Look::Empty);
    }

    #[test]
    fn a_compare_answer_that_cannot_be_read_is_unverified() {
        for (status, body) in [
            (200, json!({})),
            (200, json!({ "status": "sideways" })),
            (401, json!({ "message": "Bad credentials" })),
            (403, json!({ "message": "Forbidden" })),
        ] {
            let server = MockServer::start();
            compare_answering(&server, status, body);
            let err = client(&server)
                .compare("oakoss", "oakum", "aaa", "bbb")
                .expect_err("not an answer");
            assert!(
                matches!(err, super::Error::Unverified { .. }),
                "{status}: {err:?}"
            );
        }
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

    /// The commits endpoint is the one that peels an annotated tag: measured
    /// 2026-11-09, `pnpm/action-setup`'s annotated `v6.0.10` answers there with
    /// `0977fd99…`, its `refs/tags/v6.0.10^{}` in `git ls-remote`.
    #[test]
    fn tag_commit_reads_the_commits_endpoint_under_refs_tags() {
        let server = MockServer::start();
        let sha = "0977fd99725f1db4007ccb2928dbb4e90d06cc86";
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/pnpm/action-setup/commits/refs/tags/v6.0.10");
            then.status(200).json_body(json!({ "sha": sha }));
        });
        let commit = client(&server)
            .tag_commit("pnpm", "action-setup", "v6.0.10")
            .expect("commit");
        mock.assert();
        assert_eq!(commit, sha);
    }

    #[test]
    fn a_tag_commit_that_is_not_a_full_sha_is_unverified() {
        for (status, body) in [
            (200, json!({})),
            (200, json!({ "sha": "0977fd9" })),
            (
                200,
                json!({ "sha": "0977FD99725F1DB4007CCB2928DBB4E90D06CC86" }),
            ),
            (
                404,
                json!({ "message": "No commit found for SHA: v6.0.10" }),
            ),
            (500, json!({})),
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(GET)
                    .path("/repos/pnpm/action-setup/commits/refs/tags/v6.0.10");
                then.status(status).json_body(body.clone());
            });
            let err = client(&server)
                .tag_commit("pnpm", "action-setup", "v6.0.10")
                .expect_err("not a commit");
            assert!(
                matches!(err, super::Error::Unverified { .. }),
                "{status} {body}: {err:?}"
            );
        }
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
    fn replace_branch_commit_publishes_one_commit_without_resetting_to_parent() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/commits/deadbeef");
            then.status(200)
                .json_body(json!({ "tree": { "sha": "basetree" } }));
        });
        let blob = server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({ "sha": "blob1" }));
        });
        let tree = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/git/trees")
                .body_includes("basetree")
                .body_includes("CHANGELOG.md")
                .body_includes(r#""path":".changeset/one.md""#)
                .body_includes(r#""sha":null"#);
            then.status(201).json_body(json!({ "sha": "newtree" }));
        });
        let commit = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/git/commits")
                .body_includes("newtree")
                .body_includes("deadbeef");
            then.status(201).json_body(json!({ "sha": "abc123" }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(200)
                .json_body(json!({ "object": { "sha": "old" } }));
        });
        let updated = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages")
                .body_includes("abc123")
                .body_includes("\"force\":true");
            then.status(200)
                .json_body(json!({ "object": { "sha": "abc123" } }));
        });
        let reset = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages")
                .body_includes("deadbeef");
            then.status(200)
                .json_body(json!({ "object": { "sha": "deadbeef" } }));
        });

        let created = client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "chore(release): version packages",
                FileChanges {
                    additions: &[FileAddition {
                        path: String::from("CHANGELOG.md"),
                        contents_base64: String::from("bm90ZXM="),
                    }],
                    deletions: &[FileDeletion::new(".changeset/one.md").expect("path")],
                },
            )
            .expect("commit");

        blob.assert();
        tree.assert();
        commit.assert();
        updated.assert();
        reset.assert_calls(0);
        assert_eq!(
            created,
            CreatedCommit {
                oid: String::from("abc123")
            }
        );
    }

    #[test]
    fn replace_branch_commit_deletion_tree_includes_mode_and_type() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/commits/deadbeef");
            then.status(200)
                .json_body(json!({ "tree": { "sha": "basetree" } }));
        });
        let tree = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/git/trees")
                .body_includes(r#""mode":"100644""#)
                .body_includes(r#""type":"blob""#)
                .body_includes(r#""sha":null"#);
            then.status(201).json_body(json!({ "sha": "newtree" }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/commits");
            then.status(201).json_body(json!({ "sha": "abc123" }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(404).body("missing");
        });
        let created_ref = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/git/refs")
                .body_includes("refs/heads/oakum/version-packages")
                .body_includes("abc123");
            then.status(201).json_body(json!({
                "ref": "refs/heads/oakum/version-packages",
                "object": { "sha": "abc123" }
            }));
        });
        let blob = server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({ "sha": "blob1" }));
        });

        client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "chore(release): version packages",
                FileChanges {
                    additions: &[],
                    deletions: &[FileDeletion::new(".changeset/one.md").expect("path")],
                },
            )
            .expect("commit");

        tree.assert();
        blob.assert_calls(0);
        created_ref.assert();
    }

    fn sample_replace_files() -> (FileAddition, FileDeletion) {
        (
            FileAddition {
                path: String::from("CHANGELOG.md"),
                contents_base64: String::from("bm90ZXM="),
            },
            FileDeletion::new(".changeset/one.md").expect("path"),
        )
    }

    #[test]
    fn replace_branch_commit_fails_when_parent_commit_omits_tree_sha() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/commits/deadbeef");
            then.status(200).json_body(json!({ "tree": {} }));
        });
        let blob = server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({ "sha": "blob1" }));
        });
        let (addition, deletion) = sample_replace_files();
        let err = client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "chore(release): version packages",
                FileChanges {
                    additions: &[addition],
                    deletions: &[deletion],
                },
            )
            .expect_err("tree.sha");
        blob.assert_calls(0);
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
        assert!(err.to_string().contains("omitted tree.sha"), "{err}");
    }

    #[test]
    fn replace_branch_commit_fails_when_blob_response_omits_sha() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/commits/deadbeef");
            then.status(200)
                .json_body(json!({ "tree": { "sha": "basetree" } }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({}));
        });
        let (addition, deletion) = sample_replace_files();
        let err = client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "chore(release): version packages",
                FileChanges {
                    additions: &[addition],
                    deletions: &[deletion],
                },
            )
            .expect_err("blob sha");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
        assert!(err.to_string().contains("omitted sha"), "{err}");
    }

    #[test]
    fn replace_branch_commit_fails_when_ref_update_is_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/commits/deadbeef");
            then.status(200)
                .json_body(json!({ "tree": { "sha": "basetree" } }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({ "sha": "blob1" }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/trees");
            then.status(201).json_body(json!({ "sha": "newtree" }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/commits");
            then.status(201).json_body(json!({ "sha": "abc123" }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(200)
                .json_body(json!({ "object": { "sha": "old" } }));
        });
        server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages");
            then.status(502);
        });
        let (addition, deletion) = sample_replace_files();
        let err = client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "chore(release): version packages",
                FileChanges {
                    additions: &[addition],
                    deletions: &[deletion],
                },
            )
            .expect_err("ref update");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("unverified"), "{err}");
    }

    #[test]
    fn a_request_carries_the_token_and_user_agent() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum")
                .header("authorization", "Bearer token")
                .header("user-agent", "oakum");
            then.status(200)
                .json_body(json!({ "default_branch": "main" }));
        });

        let branch = client(&server)
            .default_branch("oakoss", "oakum")
            .expect("branch");
        mock.assert();
        assert_eq!(branch, "main");
    }

    #[test]
    fn path_segment_percent_encodes_slash_and_at() {
        assert_eq!(super::path_segment("oakum/v0.1.0"), "oakum%2Fv0.1.0");
        assert_eq!(super::path_segment("demo@1.0.0"), "demo%401.0.0");
        assert_eq!(super::path_segment("v0.1.0"), "v0.1.0");
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
                .body_includes("\"body\":\"notes\"")
                .body_includes("\"make_latest\":\"false\"");
            then.status(201).json_body(json!({
                "html_url": "https://github.com/oakoss/oakum/releases/tag/oakum-v0.1.0",
                "tag_name": "oakum-v0.1.0"
            }));
        });

        let release = client(&server)
            .create_release(
                "oakoss",
                "oakum",
                "oakum-v0.1.0",
                "oakum 0.1.0",
                "notes",
                false,
            )
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

    fn classified(
        status: u16,
        headers: &[(&'static str, &str)],
        body: Result<&str, &str>,
        missing_ok: bool,
    ) -> Result<serde_json::Value, super::Error> {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            map.insert(*name, value.parse().expect("header value"));
        }
        super::classify(
            reqwest::StatusCode::from_u16(status).expect("status"),
            &map,
            || body.map(str::to_owned).map_err(str::to_owned),
            missing_ok,
            "/repos/oakoss/oakum",
        )
    }

    #[test]
    fn a_response_that_cannot_be_trusted_is_unverified() {
        for (status, headers, body) in [
            (502, vec![], Ok("bad gateway")),
            (429, vec![], Ok("slow down")),
            (
                403,
                vec![("x-ratelimit-remaining", "0")],
                Ok("rate limited"),
            ),
            (403, vec![("retry-after", "1")], Ok("slow down")),
            (
                403,
                vec![],
                Ok("You have exceeded a secondary rate limit. Please wait."),
            ),
            (200, vec![], Ok("not-json")),
            (200, vec![], Err("connection reset")),
        ] {
            let err = classified(status, &headers, body, false).expect_err("unverified");
            assert!(
                matches!(err, super::Error::Unverified { .. }),
                "{status} {body:?}: {err:?}"
            );
        }
    }

    #[test]
    fn a_refusal_github_read_keeps_its_class() {
        let forbidden = classified(403, &[], Ok("no access"), false).expect_err("403");
        assert!(forbidden.is_forbidden(), "{forbidden:?}");

        let unauthorized = classified(
            401,
            &[],
            Ok("GitHub /repos/oakoss/oakum returned 403"),
            false,
        )
        .expect_err("401");
        assert!(
            matches!(unauthorized, super::Error::Unauthorized { .. }),
            "{unauthorized:?}"
        );

        let missing = classified(404, &[], Ok("Not Found"), false).expect_err("404");
        assert!(matches!(missing, super::Error::Other(_)), "{missing:?}");
        assert!(missing.to_string().contains("Not Found"), "{missing}");

        let unread = classified(404, &[], Err("connection reset"), false).expect_err("404");
        assert!(matches!(unread, super::Error::Other(_)), "{unread:?}");
    }

    #[test]
    fn a_rule_that_needs_no_body_does_not_read_it() {
        let headers = |pairs: &[(&'static str, &str)]| {
            let mut map = reqwest::header::HeaderMap::new();
            for (name, value) in pairs {
                map.insert(*name, value.parse().expect("header value"));
            }
            map
        };
        for (status, pairs, missing_ok) in [
            (304, vec![], false),
            (404, vec![], true),
            (503, vec![], false),
            (429, vec![], false),
            (403, vec![("retry-after", "1")], false),
            (401, vec![], false),
        ] {
            let _ = super::classify(
                reqwest::StatusCode::from_u16(status).expect("status"),
                &headers(&pairs),
                || panic!("{status} read its body"),
                missing_ok,
                "/repos/oakoss/oakum",
            );
        }
    }

    #[test]
    fn no_content_is_null_and_a_body_is_its_json() {
        for (status, body, missing_ok) in [
            (304, Ok(""), false),
            (404, Ok("Not Found"), true),
            (200, Ok("  \n"), false),
            (204, Ok(""), false),
        ] {
            assert_eq!(
                classified(status, &[], body, missing_ok).expect("no content"),
                serde_json::Value::Null,
                "{status}"
            );
        }
        assert_eq!(
            classified(200, &[], Ok(r#"{"default_branch":"main"}"#), false).expect("json"),
            json!({ "default_branch": "main" })
        );
    }

    /// reqwest's own message stops at "error sending request for url"; the
    /// reason (refused, timed out) is in its source chain.
    #[test]
    fn a_transport_failure_is_unverified_and_says_why() {
        let err = Client::at("http://127.0.0.1:1", "token")
            .expect("client")
            .default_branch("oakoss", "oakum")
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("os error"), "{err}");
    }

    #[derive(Debug)]
    struct Chain(&'static str, Option<Box<Chain>>);

    impl std::fmt::Display for Chain {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Chain {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|cause| cause as _)
        }
    }

    /// A cause that repeats the message above it is said once; one that only
    /// appears earlier in the text, as part of a URL, is still said.
    #[test]
    fn with_causes_says_each_cause_once() {
        let inner = Chain("timed out", None);
        let repeat = Chain("timed out", Some(Box::new(inner)));
        let outer = Chain("sending to /timed out/x failed", Some(Box::new(repeat)));
        assert_eq!(
            super::with_causes(&outer),
            "sending to /timed out/x failed: timed out"
        );
    }

    /// End to end: a rule that needs the headers or the body reaches them
    /// through `send`, and the message names the path asked for.
    #[test]
    fn a_rate_limit_over_http_is_unverified() {
        for (headers, body) in [
            (vec![("x-ratelimit-remaining", "0")], "rate limited"),
            (vec![("retry-after", "1")], "slow down"),
            (
                vec![],
                "You have exceeded a secondary rate limit. Please wait.",
            ),
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(GET).path("/repos/oakoss/oakum");
                let mut then = then.status(403).body(body);
                for (name, value) in &headers {
                    then = then.header(*name, *value);
                }
            });
            let err = client(&server)
                .default_branch("oakoss", "oakum")
                .expect_err("unverified");
            assert!(
                matches!(err, super::Error::Unverified { .. }),
                "{headers:?}: {err:?}"
            );
            assert!(err.to_string().contains("/repos/oakoss/oakum"), "{err}");
        }
    }

    #[test]
    fn a_forbidden_repository_is_not_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/oakoss/oakum");
            then.status(403).body("no access");
        });

        let err = client(&server)
            .default_branch("oakoss", "oakum")
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
    fn latest_release_holder_reads_the_holder_and_a_404_as_none() {
        let server = MockServer::start();
        let mut held = server.mock(|when, then| {
            when.method(GET).path("/repos/oakoss/oakum/releases/latest");
            then.status(200)
                .json_body(json!({ "tag_name": "linesmith/v0.4.1" }));
        });
        let found = client(&server)
            .latest_release_holder("oakoss", "oakum")
            .expect("holder");
        assert!(
            matches!(&found, Look::Found(tag) if tag == "linesmith/v0.4.1"),
            "{found:?}"
        );
        held.delete();
        server.mock(|when, then| {
            when.method(GET).path("/repos/oakoss/oakum/releases/latest");
            then.status(404).body("Not Found");
        });
        let none = client(&server)
            .latest_release_holder("oakoss", "oakum")
            .expect("no holder");
        assert!(matches!(none, Look::Empty), "{none:?}");
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
            .create_release(
                "oakoss",
                "oakum",
                "oakum-v0.1.0",
                "oakum 0.1.0",
                "notes",
                true,
            )
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
            .create_release(
                "oakoss",
                "oakum",
                "oakum-v0.1.0",
                "oakum 0.1.0",
                "notes",
                true,
            )
            .expect_err("unverified");
        assert!(matches!(err, super::Error::Unverified { .. }), "{err:?}");
        assert!(err.to_string().contains("html_url"), "{err}");
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
    fn replace_branch_commit_rejects_overlap_and_empty_paths() {
        let server = MockServer::start();
        let overlap = client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "fixture headline",
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
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "fixture headline",
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
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "fixture headline",
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
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "fixture headline",
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
    fn replace_branch_commit_sends_normalized_paths() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/commits/deadbeef");
            then.status(200)
                .json_body(json!({ "tree": { "sha": "basetree" } }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({ "sha": "blob1" }));
        });
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let writer = captured.clone();
        let tree = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/git/trees")
                .is_true(move |req| {
                    *writer.lock().expect("body") = req.body_string();
                    true
                });
            then.status(201).json_body(json!({ "sha": "newtree" }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/commits");
            then.status(201).json_body(json!({ "sha": "abc123" }));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
            then.status(200)
                .json_body(json!({ "object": { "sha": "old" } }));
        });
        server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages");
            then.status(200)
                .json_body(json!({ "object": { "sha": "abc123" } }));
        });

        client(&server)
            .replace_branch_commit(
                "oakoss",
                "oakum",
                "oakum/version-packages",
                "deadbeef",
                "fixture headline",
                FileChanges {
                    additions: &[FileAddition::from_text("./same.md", "notes")],
                    deletions: &[FileDeletion::new("./gone.md").expect("path")],
                },
            )
            .expect("commit");
        tree.assert();

        let body: serde_json::Value =
            serde_json::from_str(&captured.lock().expect("body")).expect("tree json");
        let paths: Vec<&str> = body["tree"]
            .as_array()
            .expect("tree entries")
            .iter()
            .map(|entry| entry["path"].as_str().expect("path"))
            .collect();
        assert_eq!(paths, ["same.md", "gone.md"]);
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
    fn update_pull_returns_number_and_url() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/pulls/7")
                .body_includes("\"title\":\"Fixture Title\"")
                .body_includes("Generated by oakum 0.0.0.")
                .body_includes("\"state\":\"open\"");
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
                "Fixture Title",
                "Generated by oakum 0.0.0.",
            )
            .expect("update");
        mock.assert();
        assert_eq!(
            pull,
            super::PullRequest {
                number: 7,
                html_url: String::from("https://github.com/oakoss/oakum/pull/7"),
                merged: false,
                author: None,
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
    fn create_pull_returns_number_and_url() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/oakoss/oakum/pulls")
                .body_includes("\"head\":\"oakum/version-packages\"")
                .body_includes("\"base\":\"main\"")
                .body_includes("\"title\":\"Fixture Title\"");
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
                "Fixture Title",
                "Generated by oakum 0.0.0.",
            )
            .expect("create");
        mock.assert();
        assert_eq!(
            pull,
            super::PullRequest {
                number: 12,
                html_url: String::from("https://github.com/oakoss/oakum/pull/12"),
                merged: false,
                author: None,
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
        assert!(id.cleanup.is_ok(), "{:?}", id.cleanup);
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
        assert!(id.cleanup.is_ok(), "{:?}", id.cleanup);
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
        assert!(id.cleanup.is_ok(), "{:?}", id.cleanup);
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
        assert!(id.cleanup.is_ok(), "{:?}", id.cleanup);
    }

    #[test]
    fn owned_plan_comments_drops_an_empty_user() {
        let comments = Look::Found(vec![super::IssueComment {
            id: 9,
            body: String::from("<!-- oakum:pr-plan -->\nquoted"),
            user: String::new(),
        }]);
        let owned = super::owned_plan_comments(comments, "<!-- oakum:pr-plan -->");
        assert!(owned.is_empty(), "{owned:?}");
    }

    #[test]
    fn owned_plan_comments_includes_org_app_bot() {
        let comments = Look::Found(vec![super::IssueComment {
            id: 3,
            body: String::from("<!-- oakum:pr-plan -->\nplan"),
            user: String::from("oakoss[bot]"),
        }]);
        assert_eq!(
            super::owned_plan_comments(comments, "<!-- oakum:pr-plan -->").len(),
            1
        );
    }

    #[test]
    fn owned_plan_comments_skips_a_bot_that_quotes_the_marker() {
        let comments = Look::Found(vec![super::IssueComment {
            id: 5,
            body: String::from("Renovate notes:\n<!-- oakum:pr-plan -->\nquoted"),
            user: String::from("renovate[bot]"),
        }]);
        let owned = super::owned_plan_comments(comments, "<!-- oakum:pr-plan -->");
        assert!(owned.is_empty(), "{owned:?}");
    }

    /// A delete that fails after the update leaves the current plan up, not a
    /// stale one or none.
    #[test]
    fn upsert_updates_the_plan_before_deleting_the_extras() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/issues/4/comments");
            then.status(200).json_body(json!([
                { "id": 2, "body": "<!-- oakum:pr-plan -->\nolder", "user": { "login": "oakoss[bot]" } },
                { "id": 3, "body": "<!-- oakum:pr-plan -->\nnewest", "user": { "login": "oakoss[bot]" } }
            ]));
        });
        let patched = server.mock(|when, then| {
            when.method(PATCH)
                .path("/repos/oakoss/oakum/issues/comments/3")
                .body_includes("current");
            then.status(200).json_body(json!({ "id": 3 }));
        });
        let failed_delete = server.mock(|when, then| {
            when.method(DELETE)
                .path("/repos/oakoss/oakum/issues/comments/2");
            then.status(500).body("boom");
        });

        let upserted = client(&server)
            .upsert_plan_comment(
                "oakoss",
                "oakum",
                4,
                "<!-- oakum:pr-plan -->",
                "<!-- oakum:pr-plan -->\ncurrent\n",
            )
            .expect("the plan was updated");
        assert!(upserted.cleanup.is_err(), "{:?}", upserted.cleanup);
        patched.assert();
        failed_delete.assert();
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
