//! The workflow YAML a reader pastes, through the jactionlint `mise run check`
//! runs on `.github/workflows`, under the same `.github/jactionlint.yaml`: every
//! ```` ```yaml ```` block in `docs/guide/`, and the workflow `oakum init`
//! prints. A documented pattern that stops linting fails here, not in the
//! repository that copied it.
//!
//! A guide block is linted as the shape it has. A workflow (`jobs:` at the top)
//! is linted as written, under `on: push` when it names no trigger. A step list
//! runs as one job's steps, a map of jobs goes under `jobs:`, and a lone `env:`
//! sits beside one trivial job. A block whose first line is `# <file>` for a
//! file in [`NOT_WORKFLOWS`] is that file's YAML and is skipped. Any other block
//! fails the test, so a new snippet is linted or skipped on purpose, never by
//! accident.

use crate::support;

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use httpmock::MockServer;
use serde_json::Value;
use support::fixture::{fake_git_dir_repo, mock_release_pin, oakum, plain_repo};

/// Rules a pasted snippet is not held to: the guide explains why the printed
/// workflow has no `concurrency:`, and a job's timeout is the repository's
/// call. Each would otherwise flag every snippet.
const NOT_HELD_TO: [&str; 2] = ["concurrency-limits", "missing-timeout"];

/// Files whose YAML the guides show and jactionlint does not read. A workflow
/// headed `# release.yml` is still linted: only these names are skipped.
const NOT_WORKFLOWS: [&str; 1] = ["pnpm-workspace.yaml"];

const SNIPPET_HEADER: &str = "on: push\npermissions: {}\n";

#[derive(Debug)]
struct Finding {
    file: String,
    line: u64,
    rule: String,
    message: String,
}

/// Every ```` ```yaml ```` block in `docs/guide/`, as `(name, text)`; the name
/// carries the guide and the block's first line.
fn guide_blocks() -> Vec<(String, String)> {
    let guides = support::workspace_root().join("docs/guide");
    let mut paths: Vec<PathBuf> = fs::read_dir(&guides)
        .unwrap_or_else(|err| panic!("{}: {err}", guides.display()))
        .map(|entry| entry.expect("guide entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    let mut blocks = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path).expect("guide readable");
        let stem = path
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .into_owned();
        let mut open: Option<(usize, Vec<&str>)> = None;
        for (index, line) in text.lines().enumerate() {
            match &mut open {
                None if is_yaml_fence(line) => open = Some((index + 2, Vec::new())),
                None => {}
                Some((start, body)) if line.trim() == "```" => {
                    blocks.push((format!("{stem}-L{start}"), body.join("\n") + "\n"));
                    open = None;
                }
                Some((_, body)) => body.push(line),
            }
        }
        assert!(open.is_none(), "{}: unclosed ```yaml block", path.display());
    }
    blocks
}

/// A fence opening YAML, indented in a list or carrying an info string after
/// the language.
fn is_yaml_fence(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("```")
        .and_then(|info| info.split_whitespace().next())
        .is_some_and(|language| matches!(language, "yaml" | "yml"))
}

fn indented(text: &str, by: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::from("\n")
            } else {
                format!("{by}{line}\n")
            }
        })
        .collect()
}

/// The workflow `block` is linted as, or `None` for another file's YAML.
fn as_workflow(name: &str, block: &str) -> Option<String> {
    let first = block.lines().next().unwrap_or_default();
    if first
        .strip_prefix("# ")
        .is_some_and(|file| NOT_WORKFLOWS.contains(&file.trim()))
    {
        return None;
    }
    let value: Value = serde_saphyr::from_str(block)
        .unwrap_or_else(|err| panic!("{name} is not YAML: {err}\n{block}"));
    let one_job = "jobs:\n  snippet:\n    runs-on: ubuntu-latest\n    steps:\n";
    let workflow = match &value {
        Value::Array(_) => format!("{SNIPPET_HEADER}{one_job}{}", indented(block, "      ")),
        Value::Object(map) if map.contains_key("jobs") && map.contains_key("on") => {
            block.to_owned()
        }
        Value::Object(map) if map.contains_key("jobs") => format!("{SNIPPET_HEADER}{block}"),
        Value::Object(map) if map.len() == 1 && map.contains_key("env") => {
            format!("{SNIPPET_HEADER}{block}{one_job}      - run: 'true'\n")
        }
        Value::Object(map)
            if !map.is_empty()
                && map
                    .values()
                    .all(|job| job.get("runs-on").is_some() || job.get("uses").is_some()) =>
        {
            format!("{SNIPPET_HEADER}jobs:\n{}", indented(block, "  "))
        }
        _ => panic!(
            "{name} is neither a workflow, a step list, a job map, nor `env:`; \
             give it one of those shapes, or a first line `# <file>` naming the \
             file it belongs in\n{block}"
        ),
    };
    Some(workflow)
}

/// jactionlint over `files`, named relative to `dir`, minus [`NOT_HELD_TO`].
fn lint(dir: &Path, files: &[String]) -> Vec<Finding> {
    let config = support::workspace_root().join(".github/jactionlint.yaml");
    let mut command = support::command_on_path("jactionlint");
    command
        .current_dir(dir)
        .args([
            "--no-online",
            "--no-color",
            "--format",
            "json",
            "--config-file",
        ])
        .arg(&config);
    for rule in NOT_HELD_TO {
        command.args(["--ignore", rule]);
    }
    let output = command
        .arg("--")
        .args(files)
        .output()
        .unwrap_or_else(|err| {
            panic!("jactionlint did not run ({err}); run the suite through `mise run test`")
        });
    // 0 is clean and 1 is findings; anything else is a run that did not lint.
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "jactionlint {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "jactionlint printed no JSON report ({err}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(
        output.status.code() == Some(1),
        !report.is_empty(),
        "jactionlint's exit code disagrees with its report: {report:?}"
    );
    report
        .iter()
        .map(|finding| Finding {
            file: finding["filepath"].as_str().unwrap_or_default().to_owned(),
            line: finding["line"].as_u64().unwrap_or_default(),
            rule: finding["id"].as_str().unwrap_or_default().to_owned(),
            message: finding["message"].as_str().unwrap_or_default().to_owned(),
        })
        .collect()
}

fn listing(findings: &[Finding]) -> String {
    let mut text = String::new();
    for f in findings {
        let _ = writeln!(text, "  {}:{} [{}] {}", f.file, f.line, f.rule, f.message);
    }
    text
}

#[test]
fn every_workflow_snippet_in_the_guides_passes_jactionlint() {
    let dir = plain_repo("workflow-snippets", "guides");
    let mut files = Vec::new();
    for (name, block) in guide_blocks() {
        if let Some(workflow) = as_workflow(&name, &block) {
            let file = format!("{name}.yml");
            fs::write(dir.join(&file), workflow).expect("write snippet");
            files.push(file);
        }
    }
    assert!(
        files.len() >= 10,
        "only {} guide snippets were linted; did the extraction break?",
        files.len()
    );
    let findings = lint(&dir, &files);
    assert!(
        findings.is_empty(),
        "guide snippets fail jactionlint (files are `<guide>-L<first line of the block>.yml`):\n{}",
        listing(&findings)
    );
}

/// Every guide snippet passes, so a wrapper that dropped its block would pass
/// too: one broken block of each shape must reach jactionlint and fail it.
#[test]
fn a_broken_snippet_of_each_shape_fails() {
    let unpinned = "- uses: actions/checkout@v4\n";
    let shapes = [
        ("steps", unpinned.to_owned()),
        (
            "job-map",
            format!("build:\n  runs-on: ubuntu-latest\n  steps:\n{}", indented(unpinned, "    ")),
        ),
        (
            "jobs",
            format!(
                "jobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n{}",
                indented(unpinned, "      ")
            ),
        ),
        (
            "workflow",
            format!(
                "# release.yml\non: push\npermissions: {{}}\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n{}",
                indented(unpinned, "      ")
            ),
        ),
        ("env", String::from("env:\n  X: ${{ github.no_such_property }}\n")),
    ];
    let dir = plain_repo("workflow-snippets", "broken");
    let mut files = Vec::new();
    for (shape, block) in &shapes {
        let workflow = as_workflow(shape, block)
            .unwrap_or_else(|| panic!("{shape}: a broken snippet was skipped, not linted"));
        let file = format!("{shape}.yml");
        fs::write(dir.join(&file), workflow).expect("write snippet");
        files.push(file);
    }
    let findings = lint(&dir, &files);
    for file in &files {
        assert!(
            findings
                .iter()
                .any(|finding| finding.file.ends_with(file.as_str())),
            "{file} reached jactionlint broken and passed:\n{}",
            listing(&findings)
        );
    }
}

#[test]
#[should_panic(expected = "is neither a workflow")]
fn a_block_of_no_known_shape_fails() {
    let _ = as_workflow("unknown", "foo: 1\n");
}

#[test]
fn a_fence_with_an_info_string_or_indent_opens_yaml() {
    for fence in [
        "```yaml",
        "```yml",
        "```yaml title=release.yml",
        "   ```yaml",
    ] {
        assert!(is_yaml_fence(fence), "{fence}");
    }
    for fence in ["```", "```toml", "```yamlish"] {
        assert!(!is_yaml_fence(fence), "{fence}");
    }
}

/// The tags and commits GitHub answered with for these actions on 2026-11-09.
fn production_shaped_pins() -> MockServer {
    let server = MockServer::start();
    for (repo, pin) in [
        (
            "actions/checkout",
            "3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1",
        ),
        (
            "pnpm/action-setup",
            "73e46992e05b19abd31f2f3c20e0e7370d762115 # v6.1.1",
        ),
    ] {
        mock_release_pin(&server, repo, pin);
    }
    server
}

#[test]
fn the_workflow_init_prints_passes_jactionlint() {
    let server = production_shaped_pins();
    let dir = plain_repo("workflow-snippets", "init");
    for label in ["cargo", "npm"] {
        let root = fake_git_dir_repo("workflow-snippets", label);
        if label == "npm" {
            fs::write(
                root.join("package.json"),
                "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
            )
            .expect("package.json");
        }
        let output = oakum(&root)
            .arg("init")
            .env("GITHUB_API_URL", server.base_url())
            .output()
            .expect("oakum init");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{label}: {}\n{stdout}",
            String::from_utf8_lossy(&output.stderr)
        );
        let file = format!("init-{label}.yml");
        fs::write(dir.join(&file), support::scaffolded_workflow(&stdout)).expect("write workflow");
        let findings = lint(&dir, &[file]);
        assert!(
            findings.is_empty(),
            "the {label} workflow `init` prints fails jactionlint:\n{}",
            listing(&findings)
        );
    }
}
