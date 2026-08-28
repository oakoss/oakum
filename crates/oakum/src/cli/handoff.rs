//! Downstream workflow handoff after a tag push (okm-h7d, ADR-0011).
//!
//! Three outcomes: a run exists, we looked and the tag cannot start one, or
//! the look did not finish. An empty `.github/workflows` is a completed look,
//! not a skip.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::Path;
use std::thread;
use std::time::Duration;

use cap_std::fs::Dir;
use serde::Deserialize;
use serde_json::Value;

use super::github::{self, Look, Refresh, WorkflowRun};
use super::release::Commit;
use super::CliError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Downstream {
    None,
    PushTags { paths: Vec<String> },
    DispatchOnly { paths: Vec<String> },
}

pub(crate) fn discover(dir: &Dir) -> Result<Downstream, CliError> {
    let mut tag_paths = Vec::new();
    let mut dispatch_paths = Vec::new();
    let entries = match dir.read_dir(".github/workflows") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Downstream::None),
        Err(err) => {
            return Err(CliError::unverified(format!(
                "unverified: failed to read `.github/workflows`: {err}"
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::unverified(format!(
                "unverified: failed to read `.github/workflows`: {err}"
            ))
        })?;
        let name = entry.file_name();
        let path = Path::new(".github/workflows").join(&name);
        let Some(name) = name.to_str() else {
            return Err(CliError::unverified(format!(
                "unverified: workflow path `{}` is not valid UTF-8",
                path.display()
            )));
        };
        let is_yaml = Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"));
        if !is_yaml {
            continue;
        }
        let file_type = entry.file_type().map_err(|err| {
            CliError::unverified(format!(
                "unverified: failed to inspect `{}`: {err}",
                path.display()
            ))
        })?;
        if !file_type.is_file() {
            return Err(CliError::unverified(format!(
                "unverified: `{}` is not a file",
                path.display()
            )));
        }
        let text = read_text(dir, &path)?;
        let trigger = classify_on(&text).map_err(|err| {
            CliError::unverified(format!(
                "unverified: `{}` on: block is not readable: {err}",
                path.display()
            ))
        })?;
        let listed = path.to_string_lossy().replace('\\', "/");
        if trigger.push_tags {
            tag_paths.push(listed);
        } else if trigger.dispatch_only {
            dispatch_paths.push(listed);
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

/// Only `snapshot` constructs this, so leftover ids cannot confirm a later tag.
pub(crate) struct SeenRuns(BTreeSet<u64>);

pub(crate) fn snapshot(
    client: &github::Client,
    owner: &str,
    repo: &str,
    commit: &Commit,
) -> Result<SeenRuns, CliError> {
    let mut seen = SeenRuns(BTreeSet::new());
    record_ids(client, owner, repo, commit, &mut seen)?;
    Ok(seen)
}

pub(crate) fn absorb(
    client: &github::Client,
    owner: &str,
    repo: &str,
    commit: &Commit,
    seen: &mut SeenRuns,
) -> Result<(), CliError> {
    record_ids(client, owner, repo, commit, seen)
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

pub(crate) fn confirm(
    client: &github::Client,
    owner: &str,
    repo: &str,
    commit: &Commit,
    paths: &[String],
    seen: &mut SeenRuns,
    require_new: bool,
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
                    let allowed = matches_listener(&run, paths)
                        && (!require_new || !seen.0.contains(&run.id));
                    seen.0.insert(run.id);
                    if chosen.is_none() && allowed {
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

fn matches_listener(run: &WorkflowRun, paths: &[String]) -> bool {
    run.path.as_ref().is_some_and(|path| paths.contains(path))
        && run
            .event
            .as_deref()
            .is_some_and(|event| event == "push" || event == "create")
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
            .unwrap_or(1),
    )
}

fn backoff(index: u32) -> Duration {
    Duration::from_secs(1 << index.saturating_sub(1).min(4))
}

fn read_text(dir: &Dir, path: &Path) -> Result<String, CliError> {
    let mut file = dir.open(path).map_err(|err| {
        CliError::unverified(format!(
            "unverified: failed to read `{}`: {err}",
            path.display()
        ))
    })?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|err| {
        CliError::unverified(format!(
            "unverified: failed to read `{}`: {err}",
            path.display()
        ))
    })?;
    Ok(text)
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
    use super::{classify_on, trigger_of, YamlOn};

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
