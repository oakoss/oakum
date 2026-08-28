//! Downstream workflow handoff after a tag push (okm-h7d, ADR-0011).
//!
//! Three outcomes: a run exists, we looked and the tag cannot start one, or
//! the look did not finish. An empty `.github/workflows` is a completed look,
//! not a skip.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use super::git::{Commit, Git, Op};
use super::github::{self, Look, Refresh, WorkflowRun};
use super::CliError;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Downstream {
    None,
    PushTags { paths: Vec<String> },
    DispatchOnly { paths: Vec<String> },
}

/// What listens at one commit. GitHub evaluates workflows at the ref that
/// triggered the run, so the listener set is read from the tree the tag
/// delivers, never from the worktree — the two diverge whenever a resumed tag
/// sits behind HEAD.
fn discover_at(git: &Git, commit: &Commit) -> Result<Downstream, CliError> {
    classify_workflows(workflows_at(git, commit)?)
}

/// The workflow files GitHub would evaluate at `commit`: yml/yaml files
/// directly under `.github/workflows`, with their contents. GitHub reads no
/// subdirectories, so the recursive listing is filtered back to direct
/// children.
fn workflows_at(git: &Git, commit: &Commit) -> Result<Vec<(String, String)>, CliError> {
    let mut files = Vec::new();
    for path in git.paths(Op::WorkflowTree { commit })? {
        let name = path
            .strip_prefix(".github/workflows/")
            .unwrap_or(path.as_str());
        let is_yaml = !name.contains('/')
            && Path::new(name).extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml")
            });
        if !is_yaml {
            continue;
        }
        let text = git.text(Op::WorkflowText {
            commit,
            path: &path,
        })?;
        files.push((path, text));
    }
    Ok(files)
}

fn classify_workflows(files: Vec<(String, String)>) -> Result<Downstream, CliError> {
    let mut tag_paths = Vec::new();
    let mut dispatch_paths = Vec::new();
    for (path, text) in files {
        let trigger = classify_on(&text).map_err(|err| {
            CliError::unverified(format!(
                "unverified: `{path}` on: block is not readable: {err}"
            ))
        })?;
        if trigger.push_tags {
            tag_paths.push(path);
        } else if trigger.dispatch_only {
            dispatch_paths.push(path);
        }
    }
    if !tag_paths.is_empty() {
        tag_paths.sort();
        return Ok(Downstream::PushTags { paths: tag_paths });
    }
    if !dispatch_paths.is_empty() {
        dispatch_paths.sort();
        return Ok(Downstream::DispatchOnly {
            paths: dispatch_paths,
        });
    }
    Ok(Downstream::None)
}

/// Only [`Handoff::open`] constructs this, so leftover ids cannot confirm a
/// later tag.
struct SeenRuns(BTreeSet<u64>);

/// What one commit owes the handoff: the workflows listening there, and the
/// runs that already existed before any tag was pushed.
struct Listener {
    paths: Vec<String>,
    seen: SeenRuns,
}

/// The downstream side of one release: what listens at each tagged commit,
/// and what runs existed there before any tag was pushed. Opened before
/// anything is written, so a failed look cannot strand a pushed tag outside
/// the partial-failure report, and a dispatch-only downstream refuses before
/// a tag it would never answer is pushed.
pub(crate) struct Handoff<'a> {
    client: &'a github::Client,
    owner: &'a str,
    repo: &'a str,
    /// `None` for a commit where nothing listens for tags: a completed look,
    /// so its confirmations are `Ok(None)` rather than a missed handoff.
    listeners: BTreeMap<Commit, Option<Listener>>,
}

impl<'a> Handoff<'a> {
    /// The commits are cloned into the map, so their borrow is independent
    /// of the handoff's own lifetime.
    pub(crate) fn open<'c>(
        client: &'a github::Client,
        owner: &'a str,
        repo: &'a str,
        git: &Git,
        commits: impl IntoIterator<Item = &'c Commit>,
    ) -> Result<Self, CliError> {
        let mut listeners: BTreeMap<Commit, Option<Listener>> = BTreeMap::new();
        for commit in commits {
            if listeners.contains_key(commit) {
                continue;
            }
            let listener = match discover_at(git, commit)? {
                Downstream::PushTags { paths } => Some(Listener {
                    paths,
                    seen: snapshot(client, owner, repo, commit)?,
                }),
                Downstream::DispatchOnly { paths } => {
                    return Err(CliError::new(format!(
                        "downstream {} at `{}` is workflow_dispatch; a tag \
                         push will not start it",
                        paths.join(", "),
                        commit.as_str()
                    )));
                }
                Downstream::None => {
                    eprintln!(
                        "no downstream workflow listens for tags at `{}`",
                        commit.as_str()
                    );
                    None
                }
            };
            listeners.insert(commit.clone(), listener);
        }
        Ok(Self {
            client,
            owner,
            repo,
            listeners,
        })
    }

    /// The run the pushed tag started, or `Ok(None)` when nothing at its
    /// commit listens. An invocation that pushed nothing started nothing, so
    /// it reports that answer at once instead of polling for a run that could
    /// only predate it — which the baseline filter would rightly refuse.
    pub(crate) fn confirm_push(
        &mut self,
        tag: &str,
        commit: &Commit,
        did_push: bool,
    ) -> Result<Option<WorkflowRun>, CliError> {
        let (client, owner, repo) = (self.client, self.owner, self.repo);
        match self.opened(commit)?.as_mut() {
            None => Ok(None),
            Some(_) if !did_push => Err(CliError::unverified(format!(
                "unverified: tag `{tag}` was already on the remote, so this \
                 invocation pushed nothing and started no workflow run; \
                 whether its downstream handoff ever ran was not confirmed — \
                 check the tag's run on the Actions page, or re-run the \
                 workflow by hand"
            ))),
            Some(Listener { paths, seen }) => {
                confirm(client, owner, repo, tag, commit, paths, seen).map(Some)
            }
        }
    }

    /// Absorbs the commit's current runs, the one just confirmed included, so
    /// none of them can also confirm a later tag at the same commit.
    pub(crate) fn absorb_before_next(&mut self, commit: &Commit) -> Result<(), CliError> {
        let (client, owner, repo) = (self.client, self.owner, self.repo);
        if let Some(Listener { seen, .. }) = self.opened(commit)?.as_mut() {
            record_ids(client, owner, repo, commit, seen)?;
        }
        Ok(())
    }

    fn opened(&mut self, commit: &Commit) -> Result<&mut Option<Listener>, CliError> {
        self.listeners.get_mut(commit).ok_or_else(|| {
            CliError::unverified(format!(
                "unverified: no pre-push snapshot for `{}`, so its handoff \
                 cannot be confirmed",
                commit.as_str()
            ))
        })
    }
}

fn snapshot(
    client: &github::Client,
    owner: &str,
    repo: &str,
    commit: &Commit,
) -> Result<SeenRuns, CliError> {
    let mut seen = SeenRuns(BTreeSet::new());
    record_ids(client, owner, repo, commit, &mut seen)?;
    Ok(seen)
}

fn record_ids(
    client: &github::Client,
    owner: &str,
    repo: &str,
    commit: &Commit,
    seen: &mut SeenRuns,
) -> Result<(), CliError> {
    let (refresh, _) = client
        .workflow_runs(owner, repo, commit.as_str(), None)
        .map_err(CliError::from)?;
    match refresh {
        Refresh::Fresh(Look::Found(runs)) => {
            for run in runs {
                seen.0.insert(run.id);
            }
            Ok(())
        }
        Refresh::Fresh(Look::Empty) => Ok(()),
        Refresh::NotModified => Err(CliError::unverified(
            "unverified: workflow-runs look was not fresh",
        )),
    }
}

fn confirm(
    client: &github::Client,
    owner: &str,
    repo: &str,
    tag: &str,
    commit: &Commit,
    paths: &[String],
    seen: &mut SeenRuns,
) -> Result<WorkflowRun, CliError> {
    let looks = look_count();
    let skip_sleep = fast_looks().is_some();
    let mut etag = None;
    for index in 0..looks {
        if index > 0 && !skip_sleep {
            thread::sleep(backoff(index));
        }
        let (refresh, next) = client
            .workflow_runs(owner, repo, commit.as_str(), etag.as_deref())
            .map_err(CliError::from)?;
        if let Some(next) = next {
            etag = Some(next);
        }
        let matched = match refresh {
            Refresh::Fresh(Look::Found(runs)) => {
                let mut chosen = None;
                for run in runs {
                    // A run that predates the invocation is in the baseline
                    // and cannot be this tag's handoff, however well it
                    // matches: the workflow may have been disabled or deleted
                    // since it ran. Only the chosen run is absorbed here — a
                    // fresh run first seen incomplete must stay eligible for
                    // a later look, and `absorb_before_next` re-reads before
                    // any same-commit successor.
                    let allowed = matches_listener(&run, paths, tag) && !seen.0.contains(&run.id);
                    if chosen.is_none() && allowed {
                        seen.0.insert(run.id);
                        chosen = Some(run);
                    }
                }
                chosen
            }
            Refresh::NotModified | Refresh::Fresh(Look::Empty) => None,
        };
        if let Some(run) = matched {
            return Ok(run);
        }
    }
    Err(CliError::unverified(format!(
        "unverified: no workflow run for `{}` on {} after {looks} looks",
        commit.as_str(),
        paths.join(", ")
    )))
}

fn matches_listener(run: &WorkflowRun, paths: &[String], tag: &str) -> bool {
    let listens = run.path.as_ref().is_some_and(|path| paths.contains(path));
    let for_this_tag = match run.event.as_deref() {
        // A push run reports the pushed ref's short name as head_branch
        // (measured, okm-e9e.17), so a branch push at the same commit of a
        // workflow listening to both is told apart from this tag's push.
        Some("push") => run.head_branch.as_deref() == Some(tag),
        // What a create-event run reports there is unmeasured: a named ref
        // must still be this tag's, while an absent one is accepted rather
        // than making every create handoff unverifiable.
        Some("create") => run.head_branch.as_deref().is_none_or(|named| named == tag),
        _ => false,
    };
    listens && for_this_tag
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Trigger {
    push_tags: bool,
    dispatch_only: bool,
}

fn classify_on(text: &str) -> Result<Trigger, String> {
    let parsed: WorkflowFile = serde_saphyr::from_str(text).map_err(|err| err.to_string())?;
    Ok(trigger_of(parsed.on.as_ref()))
}

fn trigger_of(on: Option<&YamlOn>) -> Trigger {
    let Some(on) = on else {
        return Trigger::default();
    };
    match on {
        YamlOn::Name(name) => Trigger {
            push_tags: name == "create",
            dispatch_only: name == "workflow_dispatch",
        },
        YamlOn::Names(names) => Trigger {
            push_tags: names.iter().any(|name| name == "create"),
            dispatch_only: dispatch_only_events(names.iter().map(String::as_str)),
        },
        YamlOn::Map(map) => Trigger {
            push_tags: map.contains_key("create") || push_has_tags(map.get("push")),
            dispatch_only: dispatch_only_events(map.keys().map(String::as_str)),
        },
    }
}

fn dispatch_only_events<'a>(events: impl IntoIterator<Item = &'a str>) -> bool {
    let mut only = false;
    for event in events {
        if event != "workflow_dispatch" {
            return false;
        }
        only = true;
    }
    only
}

fn push_has_tags(push: Option<&Value>) -> bool {
    match push {
        Some(Value::Object(map)) => {
            nonempty_list(map.get("tags")) || nonempty_list(map.get("tags-ignore"))
        }
        _ => false,
    }
}

fn nonempty_list(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::String(text)) => !text.is_empty(),
        _ => false,
    }
}

fn look_count() -> u32 {
    fast_looks().unwrap_or(6)
}

fn fast_looks() -> Option<u32> {
    // Parsed as a look count; default empty-look wait is ~30s.
    let value = std::env::var_os("OAKUM_HANDOFF_FAST")?;
    Some(
        value
            .to_str()
            .and_then(|text| text.parse().ok())
            .unwrap_or_else(|| {
                // Said once: `look_count` and the sleep gate both ask per look.
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    eprintln!(
                        "warning: OAKUM_HANDOFF_FAST is set but not a number; \
                 verifying with 1 look"
                    );
                });
                1
            }),
    )
}

fn backoff(index: u32) -> Duration {
    Duration::from_secs(1 << index.saturating_sub(1).min(4))
}

#[derive(Deserialize)]
struct WorkflowFile {
    #[serde(default, rename = "on")]
    on: Option<YamlOn>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum YamlOn {
    Name(String),
    Names(Vec<String>),
    Map(BTreeMap<String, Value>),
}

#[cfg(test)]
mod tests {
    use super::super::git::{Commit, Git, Reply};
    use super::super::github;
    use super::{classify_on, trigger_of, Handoff, YamlOn};
    use httpmock::prelude::*;
    use serde_json::json;

    fn minted(shas: [&'static str; 2]) -> [Commit; 2] {
        let git = Git::answering([
            ("rev-parse HEAD", Reply::said(shas[0])),
            ("rev-parse HEAD", Reply::said(shas[1])),
        ]);
        [git.head().expect("minted"), git.head().expect("minted")]
    }

    fn runs_for<'a>(server: &'a MockServer, sha: &str, ids: &[u64]) -> httpmock::Mock<'a> {
        let runs: Vec<_> = ids
            .iter()
            .map(|id| {
                json!({
                    "id": id,
                    "head_sha": sha,
                    "head_branch": "v1.0.0",
                    "status": "completed",
                    "path": ".github/workflows/release.yml",
                    "event": "push",
                    "html_url": format!("https://example.test/runs/{id}"),
                })
            })
            .collect();
        let sha = sha.to_owned();
        server.mock(move |when, then| {
            when.method(GET)
                .path("/repos/o/r/actions/runs")
                .query_param("head_sha", sha);
            then.status(200)
                .json_body(json!({ "total_count": runs.len(), "workflow_runs": runs }));
        })
    }

    const TREE_OP: &str = "ls-tree";
    const TEXT_OP: &str = "cat-file blob";
    const LISTENER: &str = "on:\n  push:\n    tags: ['v*']\n";

    /// A `Git` whose tree at every asked commit holds one tag-listening
    /// workflow. One (listing, read) pair per expected ask.
    fn listening_tree(reads: usize) -> Git {
        Git::answering((0..reads).flat_map(|_| {
            [
                (TREE_OP, Reply::said(".github/workflows/release.yml\0")),
                (TEXT_OP, Reply::said(LISTENER)),
            ]
        }))
    }

    /// The baselines are per commit: a run seen before any push at one commit
    /// must not stop the same run id from confirming a different commit's tag.
    /// Shared baselines would filter run 1 here and poll for ~30s instead.
    #[test]
    fn baselines_are_taken_per_commit_before_any_write() {
        let server = MockServer::start();
        let [aaa, bbb] = minted(["aaa", "bbb"]);
        let at_aaa = runs_for(&server, "aaa", &[1]);
        let mut at_bbb = runs_for(&server, "bbb", &[]);
        let client = github::Client::at(server.base_url(), "token").expect("client");
        let git = listening_tree(2);

        let mut handoff = Handoff::open(&client, "o", "r", &git, [&aaa, &aaa, &bbb]).expect("open");
        assert_eq!(at_aaa.calls(), 1, "one snapshot per distinct commit");
        assert_eq!(at_bbb.calls(), 1);
        assert_eq!(git.asked().len(), 4, "one tree read per distinct commit");

        at_bbb.delete();
        runs_for(&server, "bbb", &[1]);
        let run = handoff
            .confirm_push("v1.0.0", &bbb, true)
            .expect("run 1 is new at bbb")
            .expect("a listener is declared");
        assert_eq!(run.id, 1);
    }

    /// A commit whose tree holds no workflows is a completed look: nothing is
    /// snapshotted, and confirmation is `Ok(None)`, not a missed handoff.
    #[test]
    fn a_commit_without_tag_listeners_neither_looks_nor_confirms() {
        let server = MockServer::start();
        let [aaa, _] = minted(["aaa", "bbb"]);
        let client = github::Client::at(server.base_url(), "token").expect("client");
        let git = Git::answering([(TREE_OP, Reply::said(""))]);

        let mut handoff = Handoff::open(&client, "o", "r", &git, [&aaa]).expect("open");
        let confirmed = handoff
            .confirm_push("v1.0.0", &aaa, true)
            .expect("nothing to confirm");
        assert_eq!(confirmed, None);
        handoff.absorb_before_next(&aaa).expect("nothing to absorb");
    }

    /// A dispatch-only downstream at a tagged commit refuses inside `open`,
    /// before any tag is pushed, naming the commit it was read from.
    #[test]
    fn a_dispatch_only_commit_refuses_before_any_write() {
        let server = MockServer::start();
        let [aaa, _] = minted(["aaa", "bbb"]);
        let client = github::Client::at(server.base_url(), "token").expect("client");
        let git = Git::answering([
            (TREE_OP, Reply::said(".github/workflows/publish.yml\0")),
            (
                TEXT_OP,
                Reply::said("on:\n  workflow_dispatch:\n    inputs: {}\n"),
            ),
        ]);

        let Err(err) = Handoff::open(&client, "o", "r", &git, [&aaa]) else {
            panic!("a dispatch-only downstream must refuse")
        };
        assert!(err.to_string().contains("workflow_dispatch"), "{err}");
        assert!(err.to_string().contains("`aaa`"), "{err}");
    }

    /// GitHub reads only yml/yaml files directly under `.github/workflows`,
    /// so nothing else in the recursive listing is opened or classified.
    #[test]
    fn only_direct_yaml_children_are_read() {
        let [aaa, _] = minted(["aaa", "bbb"]);
        let git = Git::answering([
            (
                TREE_OP,
                Reply::said(
                    ".github/workflows/README.md\0.github/workflows/sub/nested.yml\0\
                     .github/workflows/release.YAML\0",
                ),
            ),
            (TEXT_OP, Reply::said(LISTENER)),
        ]);

        let files = super::workflows_at(&git, &aaa).expect("read");
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].0, ".github/workflows/release.YAML");
        assert_eq!(git.asked(), [TREE_OP, TEXT_OP], "one read per yaml child");
    }

    /// The event decides what `head_branch` must prove: a push run carries
    /// the pushed ref's short name (measured, okm-e9e.17), so only the run
    /// this tag's push started counts for it.
    #[test]
    fn a_push_run_counts_only_for_its_own_ref() {
        let path = String::from(".github/workflows/release.yml");
        let run = |event: Option<&str>, head_branch: Option<&str>| super::github::WorkflowRun {
            id: 1,
            head_sha: String::from("aaa"),
            head_branch: head_branch.map(String::from),
            status: String::from("completed"),
            conclusion: None,
            path: Some(path.clone()),
            event: event.map(String::from),
            html_url: String::new(),
        };
        let paths = [path.clone()];

        assert!(super::matches_listener(
            &run(Some("push"), Some("v1.0.0")),
            &paths,
            "v1.0.0"
        ));
        let branch_push = run(Some("push"), Some("main"));
        assert!(!super::matches_listener(&branch_push, &paths, "v1.0.0"));
        let unattributed = run(Some("push"), None);
        assert!(!super::matches_listener(&unattributed, &paths, "v1.0.0"));
        // What a create-event run reports in head_branch is unmeasured: an
        // absent ref is accepted, a named one must still be this tag's.
        assert!(super::matches_listener(
            &run(Some("create"), None),
            &paths,
            "v1.0.0"
        ));
        assert!(super::matches_listener(
            &run(Some("create"), Some("v1.0.0")),
            &paths,
            "v1.0.0"
        ));
        assert!(!super::matches_listener(
            &run(Some("create"), Some("main")),
            &paths,
            "v1.0.0"
        ));
        assert!(!super::matches_listener(&run(None, None), &paths, "v1.0.0"));
    }

    #[test]
    fn a_commit_that_was_never_opened_cannot_be_confirmed() {
        let server = MockServer::start();
        let [aaa, _] = minted(["aaa", "bbb"]);
        let client = github::Client::at(server.base_url(), "token").expect("client");
        let git = Git::answering([]);

        let mut handoff = Handoff::open(&client, "o", "r", &git, []).expect("open");
        let err = handoff
            .confirm_push("v1.0.0", &aaa, true)
            .expect_err("no baseline was taken");
        assert!(err.to_string().starts_with("unverified:"), "{err}");
        assert!(err.to_string().contains("no pre-push snapshot"), "{err}");
    }

    #[test]
    fn branch_push_is_not_downstream() {
        let trigger = classify_on("on: push\n").expect("parse");
        assert!(!trigger.push_tags);
        assert!(!trigger.dispatch_only);
    }

    #[test]
    fn push_tags_is_downstream() {
        let trigger = classify_on("on:\n  push:\n    tags:\n      - '*'\n").expect("parse");
        assert!(trigger.push_tags);
        assert!(!trigger.dispatch_only);
    }

    #[test]
    fn create_event_is_downstream() {
        let trigger = classify_on("on: create\n").expect("parse");
        assert!(trigger.push_tags);
    }

    #[test]
    fn dispatch_only_is_not_a_tag_push() {
        let trigger = classify_on("on:\n  workflow_dispatch:\n    inputs: {}\n").expect("parse");
        assert!(!trigger.push_tags);
        assert!(trigger.dispatch_only);
    }

    #[test]
    fn both_tag_and_dispatch_prefers_tags_for_polling() {
        let trigger =
            classify_on("on:\n  push:\n    tags: ['v*']\n  workflow_dispatch:\n    inputs: {}\n")
                .expect("parse");
        assert!(trigger.push_tags);
        assert!(!trigger.dispatch_only);
    }

    #[test]
    fn ci_dispatch_button_is_not_downstream() {
        let trigger = classify_on(
            "on:\n  pull_request:\n  push:\n    branches: [main]\n  workflow_dispatch:\n",
        )
        .expect("parse");
        assert!(!trigger.push_tags);
        assert!(!trigger.dispatch_only);
    }

    #[test]
    fn dispatch_plus_workflow_call_is_not_dispatch_only() {
        let trigger = classify_on("on: [workflow_call, workflow_dispatch]\n").expect("parse");
        assert!(!trigger.dispatch_only);
    }

    #[test]
    fn tags_ignore_is_downstream() {
        let trigger =
            classify_on("on:\n  push:\n    tags-ignore:\n      - '*-dev'\n").expect("parse");
        assert!(trigger.push_tags);
    }

    #[test]
    fn scalar_tags_is_downstream() {
        let trigger = classify_on("on:\n  push:\n    tags: 'v*'\n").expect("parse");
        assert!(trigger.push_tags);
    }

    #[test]
    fn scalar_tags_ignore_is_downstream() {
        let trigger = classify_on("on:\n  push:\n    tags-ignore: '*-dev'\n").expect("parse");
        assert!(trigger.push_tags);
    }

    #[test]
    fn unreadable_on_is_an_error() {
        assert!(classify_on("on: [\n").is_err());
    }

    #[test]
    fn missing_on_is_not_downstream() {
        assert_eq!(trigger_of(None::<&YamlOn>), super::Trigger::default());
    }

    #[test]
    fn empty_tags_list_is_not_downstream() {
        let trigger = classify_on("on:\n  push:\n    tags: []\n").expect("parse");
        assert!(!trigger.push_tags);
    }

    #[test]
    fn list_form_push_is_not_downstream() {
        let trigger = classify_on("on: [push]\n").expect("parse");
        assert!(!trigger.push_tags);
    }

    #[test]
    fn list_form_create_is_downstream() {
        let trigger = classify_on("on: [create]\n").expect("parse");
        assert!(trigger.push_tags);
    }
}
