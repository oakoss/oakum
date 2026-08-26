//! GitHub HTTP (okm-dlo). Lives in the binary so it does not add a second
//! `src/` I/O marker (ADR-0002). No octocrab.
//!
//! Tag push is git, not this client ([github-release-path.md](../../../../docs/research/github-release-path.md)).

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "okm-dlo client; a later slice wires a command")
)]

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
    token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileAddition {
    pub path: String,
    pub contents_base64: String,
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
pub(crate) struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct WorkflowRun {
    pub id: u64,
    pub head_sha: String,
    pub status: String,
    pub conclusion: Option<String>,
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
    Unverified { detail: String },
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unverified { detail } | Self::Other(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for Error {}

impl Client {
    pub(crate) fn new(token: impl Into<String>) -> Result<Self, Error> {
        Self::at("https://api.github.com", token)
    }

    fn at(api: impl Into<String>, token: impl Into<String>) -> Result<Self, Error> {
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
            token,
        })
    }

    pub(crate) fn create_commit_on_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        expected_head_oid: &str,
        headline: &str,
        additions: &[FileAddition],
    ) -> Result<CreatedCommit, Error> {
        let additions = additions
            .iter()
            .map(|file| {
                json!({
                    "path": file.path,
                    "contents": file.contents_base64,
                })
            })
            .collect::<Vec<_>>();
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
                    "fileChanges": { "additions": additions },
                }
            }
        });
        let value = self.graphql(&body)?;
        let oid = value
            .pointer("/data/createCommitOnBranch/commit/oid")
            .and_then(Value::as_str)
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
        let html_url = value
            .get("html_url")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("create release returned no html_url"))?;
        Ok(CreatedRelease {
            html_url: html_url.to_owned(),
            tag_name: tag_name.to_owned(),
        })
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

    fn graphql(&self, body: &Value) -> Result<Value, Error> {
        let value = self.json(reqwest::Method::POST, "/graphql", Some(body))?;
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

    fn raw_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<(StatusCode, HeaderMap, Value), Error> {
        let url = format!("{}{path}", self.api);
        let mut request = self
            .http
            .request(method, &url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().map_err(|err| {
            Error::unverified(format!(
                "unverified: GitHub request to {path} failed: {err}"
            ))
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        if status == StatusCode::NOT_MODIFIED {
            return Ok((status, headers, Value::Null));
        }
        if status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN && rate_limited(&headers))
        {
            return Err(Error::unverified(format!(
                "unverified: GitHub {path} returned {status}"
            )));
        }
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(Error::new(format!(
                "GitHub {path} returned {status}: {body}"
            )));
        }
        let value = response
            .json::<Value>()
            .map_err(|err| Error::unverified(format!("unverified: GitHub {path} JSON: {err}")))?;
        Ok((status, headers, value))
    }
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
        CheckRun, Client, CreatedCommit, CreatedRelease, FileAddition, Look, Refresh, WorkflowRun,
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
                &[FileAddition {
                    path: String::from("CHANGELOG.md"),
                    contents_base64: String::from("bm90ZXM="),
                }],
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
            .create_commit_on_branch("oakoss", "oakum", "main", "old", "msg", &[])
            .expect_err("graphql");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
        assert!(
            err.to_string().contains("expectedHeadOid mismatch"),
            "{err}"
        );
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
                status: String::from("completed"),
                conclusion: Some(String::from("success")),
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
    fn a_forbidden_ref_is_not_unverified() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/oakoss/oakum/commits/abc/check-runs");
            then.status(403).body("no access");
        });

        let err = client(&server)
            .check_runs("oakoss", "oakum", "abc")
            .expect_err("other");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
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
            .create_commit_on_branch("oakoss", "oakum", "main", "old", "msg", &[])
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
            .expect_err("other");
        assert!(matches!(err, super::Error::Other(_)), "{err:?}");
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
}
