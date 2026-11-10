//! `oakum workflow` (`okm-bryc`): the workflow `init` prints, alone on stdout,
//! with nothing written.

use crate::support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::fixture::{cargo_package, mock_action_pins, oakum, Fixture};
use support::repo_files::{write_install_pin, BINARY_VERSION};

fn temp_repo(label: &str) -> Fixture {
    support::fixture::fake_git_dir_repo("workflow", label)
}

fn run(root: &Path, command: &str, args: &[&str]) -> Output {
    let server = mock_action_pins();
    oakum(root)
        .arg(command)
        .args(args)
        .env("GITHUB_API_URL", server.base_url())
        .output()
        .unwrap_or_else(|err| panic!("oakum {command}: {err}"))
}

/// Every path under `root` with its bytes, so a run that writes, renames, or
/// removes anything reads differently.
fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let rel = path.strip_prefix(root).expect("under root").to_path_buf();
            if path.is_dir() {
                out.push((rel, Vec::new()));
                walk(root, &path, out);
            } else {
                out.push((rel, fs::read(&path).expect("read file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn prints_the_init_workflow_alone_and_writes_nothing() {
    let reference = temp_repo("reference");
    cargo_package(&reference, "demo", "0.1.0");
    let init = run(&reference, "init", &[]);
    assert_eq!(init.status.code(), Some(0), "{}", text(&init.stderr));
    let printed_by_init = text(&init.stdout);
    let expected = support::scaffolded_workflow(&printed_by_init);

    let fresh = temp_repo("fresh");
    cargo_package(&fresh, "demo", "0.1.0");
    let initialized = temp_repo("initialized");
    cargo_package(&initialized, "demo", "0.1.0");
    let setup = run(&initialized, "init", &[]);
    assert_eq!(setup.status.code(), Some(0), "{}", text(&setup.stderr));

    for root in [&fresh, &initialized] {
        let before = tree(root);
        let output = run(root, "workflow", &[]);
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert_eq!(text(&output.stdout), expected, "{stderr}");
        assert_eq!(tree(root), before, "`oakum workflow` wrote: {stderr}");
        assert_eq!(
            stderr,
            format!(
                "next: no oakum install pin yet; `tool: oakum@{BINARY_VERSION}` in the printed \
                 workflow is the pin: commit the workflow under `.github/workflows/`\n"
            )
        );
    }
}

/// An npm repository with no pnpm declared asks pnpm its version, the one
/// child process this command starts; it must leave the tree as it was.
#[test]
fn the_pnpm_probe_writes_nothing() {
    let root = temp_repo("npm-probe");
    fs::write(
        root.join("package.json"),
        "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    let before = tree(&root);
    let output = run(&root, "workflow", &[]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(tree(&root), before, "{stderr}");
    let stdout = text(&output.stdout);
    assert!(stdout.contains("          version: "), "{stdout}");

    let twin = temp_repo("npm-probe-init");
    fs::write(
        twin.join("package.json"),
        "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    let init = run(&twin, "init", &[]);
    assert_eq!(init.status.code(), Some(0), "{}", text(&init.stderr));
    assert_eq!(
        stdout,
        support::scaffolded_workflow(&text(&init.stdout)),
        "{stderr}"
    );
    assert!(
        stderr.contains("which needs `@oakoss/oakum` in `package.json`"),
        "the dependency step goes to stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn a_dev_engines_range_is_refused_without_asking_pnpm() {
    for (label, spec) in [("range", ">=10"), ("empty", "")] {
        let root = temp_repo(&format!("npm-{label}"));
        fs::write(
            root.join("package.json"),
            format!(
                "{{\"name\": \"demo\", \"version\": \"0.1.0\", \"devEngines\": {{\"packageManager\": [{{\"name\": \"pnpm\", \"version\": \"{spec}\"}}]}}}}\n"
            ),
        )
        .expect("package.json");
        let asked = root.with_extension("asked");
        let shim = support::tools::path_shim(
            &root,
            "pnpm",
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", asked.display()),
        );
        let before = tree(&root);
        let server = mock_action_pins();
        let output = oakum(&root)
            .arg("workflow")
            .env("PATH", support::tools::path_prefixed_by(&shim))
            .env("GITHUB_API_URL", server.base_url())
            .output()
            .expect("oakum workflow");
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{spec}: {stderr}");
        assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
        assert!(
            stderr.contains(&format!(
                "`devEngines.packageManager` in package.json gives pnpm the version `\"{spec}\"`"
            )) && stderr.contains("declare an exact version there"),
            "{stderr}"
        );
        assert!(!asked.exists(), "{spec}: pnpm was asked");
        assert_eq!(tree(&root), before, "{stderr}");
    }
}

/// A redirect must never hold half a workflow.
#[test]
fn a_failed_pin_lookup_prints_nothing_on_stdout() {
    let root = temp_repo("offline");
    cargo_package(&root, "demo", "0.1.0");
    let output = oakum(&root)
        .arg("workflow")
        .env("GITHUB_API_URL", "http://127.0.0.1:9")
        .output()
        .expect("oakum workflow");
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    assert!(stderr.contains("pins `actions/checkout`"), "{stderr}");
}

#[test]
fn a_pin_naming_another_version_refuses_before_printing() {
    let root = temp_repo("mismatch");
    write_install_pin(&root, "0.0.1");
    let output = run(&root, "workflow", &[]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    assert!(
        stderr.contains(&format!(
            "this binary is `{BINARY_VERSION}` but the install pin is `0.0.1`"
        )),
        "{stderr}"
    );
}

/// Pins that disagree or cannot be read are a look that failed: the workflow
/// still prints, and the run does not exit like one that found nothing to say.
#[test]
fn unsettled_pins_print_the_workflow_and_exit_unverified() {
    let disagree = temp_repo("disagree");
    write_install_pin(&disagree, BINARY_VERSION);
    fs::write(
        disagree.join(".mise.toml"),
        "[tools]\n\"github:oakoss/oakum\" = \"0.0.1\"\n",
    )
    .expect("mise pin");
    let unreadable = temp_repo("unreadable");
    fs::create_dir_all(unreadable.join(".github/workflows")).expect("workflows");
    fs::write(
        unreadable.join(".github/workflows/release.yml"),
        "jobs: [\n",
    )
    .expect("bad yaml");
    for (root, said) in [
        (&disagree, "install pins disagree"),
        (
            &unreadable,
            "`.github/workflows/release.yml` is not valid YAML",
        ),
    ] {
        let output = run(root, "workflow", &[]);
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{said}: {stderr}");
        assert!(
            text(&output.stdout).starts_with("name: oakum\n"),
            "{stderr}"
        );
        assert!(stderr.contains(said), "{stderr}");
    }
}

#[test]
fn a_workflow_nobody_can_receive_is_not_success() {
    let root = temp_repo("dead-stdout");
    let server = mock_action_pins();
    let output = oakum(&root)
        .arg("workflow")
        .env("GITHUB_API_URL", server.base_url())
        .stdout(support::dead_stdout())
        .output()
        .expect("oakum workflow");
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("the workflow could not be delivered to stdout"),
        "{stderr}"
    );
}

#[test]
fn the_token_flags_shape_the_workflow_in_a_managed_repository() {
    let root = temp_repo("token");
    let setup = run(&root, "init", &[]);
    assert_eq!(setup.status.code(), Some(0), "{}", text(&setup.stderr));

    let named = run(&root, "workflow", &["--token", "RELEASE_PAT"]);
    let stderr = text(&named.stderr);
    assert_eq!(named.status.code(), Some(0), "{stderr}");
    assert!(
        text(&named.stdout).contains("GITHUB_TOKEN: ${{ secrets.RELEASE_PAT || github.token }}"),
        "{stderr}"
    );
    assert!(
        stderr.contains("the printed workflow reads the `RELEASE_PAT` secret"),
        "{stderr}"
    );

    let app = run(&root, "workflow", &["--app"]);
    let stderr = text(&app.stderr);
    assert_eq!(app.status.code(), Some(0), "{stderr}");
    assert!(
        text(&app.stdout).contains("actions/create-github-app-token@"),
        "{stderr}"
    );
    assert!(stderr.contains("`APP_CLIENT_ID`"), "{stderr}");
}
