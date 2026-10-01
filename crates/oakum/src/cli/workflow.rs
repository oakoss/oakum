//! The workflow `init` and `migrate` print for the reader to paste. Oakum
//! writes no CI file (ADR-0003), so this text is the deliverable.

use std::path::Path;

use cap_std::fs::Dir;
use oakum::discover::pnpm_version;
use semver::Version;

use super::ci::VERSION_BRANCH;
use super::fs::read_resolved_text;
use super::github;
use super::CliError;

/// Action pins for the printed workflow. JavaScript actions are looked up at
/// print time because a baked-in major goes stale as GitHub retires Node
/// runtimes; [`INSTALL_ACTION_PIN`] is a composite action and ships baked in.
pub(super) struct WorkflowPins {
    checkout: String,
    install: Install,
}

/// How a job gets oakum onto `ubuntu-latest`, which ships npm but neither pnpm
/// nor `cargo-binstall`.
enum Install {
    /// An npm workspace installs through the channel it already uses, after
    /// `pnpm/action-setup`: discovery asks pnpm for the packages.
    Npm(PnpmSetup),
    /// oakum is not in `taiki-e/install-action`'s own manifests, so the action
    /// installs `cargo-binstall` and falls back to it.
    Cargo,
}

/// A composite action, so an older tag keeps running; its releases are
/// immutable. Renovate bumps it (`.github/renovate.json`). Baked in so the
/// Cargo workflow costs no GitHub request beyond `actions/checkout`'s.
const INSTALL_ACTION_PIN: &str = "v2.87.22";

struct PnpmSetup {
    pin: String,
    /// `pnpm/action-setup` refuses to run with neither a `version` input nor a
    /// `packageManager` (or `devEngines.packageManager`) field, and refuses a
    /// `version` input that disagrees with `packageManager`. Set only when
    /// `package.json` declares neither, from the pnpm that ran discovery.
    version: Option<String>,
}

impl WorkflowPins {
    pub(super) fn lookup(dir: &Dir, repo: &Path) -> Result<Self, CliError> {
        let checkout = github::latest_release_tag("actions", "checkout").map_err(CliError::from)?;
        let install = if npm_workspace(repo) {
            let pin = github::latest_release_tag("pnpm", "action-setup").map_err(CliError::from)?;
            let version = if declares_package_manager(dir)? {
                None
            } else {
                Some(pnpm_version(repo).map_err(|err| {
                    CliError::unverified(format!(
                        "unverified: pnpm version for the workflow: {err}; declare `packageManager` (`pnpm@<version>`) in package.json so the workflow needs no version input, or fix pnpm on PATH"
                    ))
                })?)
            };
            Install::Npm(PnpmSetup { pin, version })
        } else {
            Install::Cargo
        };
        Ok(Self { checkout, install })
    }

    /// A pin the reader can add to the repository beside the one the workflow
    /// carries, from the same term as [`Self::install_step`] so the two name
    /// one ecosystem. A Cargo manifest has no entry that installs a binary.
    pub(super) fn repository_pin(&self, binary: &Version) -> Option<String> {
        match self.install {
            Install::Npm(_) => Some(format!("pnpm add -D @oakoss/oakum@{binary}")),
            Install::Cargo => None,
        }
    }

    /// `check` reads either step as the install pin.
    fn install_step(&self, binary: &Version) -> String {
        match &self.install {
            Install::Npm(_) => format!("      - run: npm i -g @oakoss/oakum@{binary}\n"),
            Install::Cargo => format!(
                "      - uses: taiki-e/install-action@{INSTALL_ACTION_PIN}\n        with:\n          tool: oakum@{binary}\n"
            ),
        }
    }

    fn setup_steps(&self) -> String {
        match &self.install {
            Install::Npm(PnpmSetup { pin, version: None }) => {
                format!("      - uses: pnpm/action-setup@{pin}\n")
            }
            Install::Npm(PnpmSetup {
                pin,
                version: Some(version),
            }) => format!(
                "      - uses: pnpm/action-setup@{pin}\n        with:\n          version: {version}\n"
            ),
            Install::Cargo => String::new(),
        }
    }
}

pub(super) fn npm_workspace(repo: &Path) -> bool {
    repo.join("package.json").is_file() || repo.join("pnpm-workspace.yaml").is_file()
}

/// Whether the root `package.json` declares a pnpm version the way
/// `pnpm/action-setup` reads one; anything else makes the action demand its
/// `version` input. A workspace declared only by `pnpm-workspace.yaml` has no
/// manifest to read.
fn declares_package_manager(dir: &Dir) -> Result<bool, CliError> {
    let Some(text) = read_resolved_text(dir, Path::new("package.json"))? else {
        return Ok(false);
    };
    let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        CliError::unverified(format!(
            "unverified: `package.json` is not valid JSON: {err}"
        ))
    })?;
    Ok(declares_pnpm(&manifest))
}

fn declares_pnpm(manifest: &serde_json::Value) -> bool {
    let top_level = manifest
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
        .and_then(|spec| spec.strip_prefix("pnpm@"))
        .is_some_and(|version| !version.split('+').next().unwrap_or("").is_empty());
    let dev_engines = manifest
        .get("devEngines")
        .and_then(|engines| engines.get("packageManager"))
        .is_some_and(|pm| {
            pm.get("name").and_then(serde_json::Value::as_str) == Some("pnpm")
                && pm
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|version| !version.is_empty())
        });
    top_level || dev_engines
}

/// The workflow YAML, with `binary` as every job's install pin.
pub(super) fn workflow_text(binary: &Version, pins: &WorkflowPins) -> String {
    let checkout = &pins.checkout;
    let setup = pins.setup_steps();
    let install = pins.install_step(binary);
    format!(
        "\
name: oakum
on:
  pull_request:
  push:
jobs:
  check:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{setup}{install}      # Identity, not a branch name: a fork, a person, and a push to the bot's
      # branch each fail one of these terms and get checked. The skip reaches
      # the real version pull request only once its author is a bot whose push
      # retriggers CI — secrets.GITHUB_TOKEN raises no run for it to skip.
      - run: oakum check --strict
        if: >-
          github.head_ref != '{VERSION_BRANCH}'
          || github.event.pull_request.head.repo.full_name != github.repository
          || github.event.pull_request.user.type != 'Bot'
          || github.event.sender.type != 'Bot'
      - run: oakum ci pr-status
        id: pr-status
        if: success() || failure()
        continue-on-error: true
        env:
          GITHUB_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}
      # A step that failed must not read as one that reported; a post that
      # fell back to the job summary is not this, and is by design.
      - run: echo \"::warning title=oakum ci pr-status::the step failed, so its report may not have reached the pull request or the job summary; the check above still decides\"
        if: (success() || failure()) && steps.pr-status.outcome == 'failure'
  version:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{setup}{install}      - run: oakum ci version-pr
        env:
          GITHUB_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}
  release:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{setup}{install}      # Tags oakum pushes carry this as their tagger; it matches this job's
      # secrets.GITHUB_TOKEN. Swap the token and swap this too. The version
      # commit is written through the GitHub API and carries the token's own
      # account, which no git config here can change.
      - run: |
          git config user.name \"github-actions[bot]\"
          git config user.email \"41898282+github-actions[bot]@users.noreply.github.com\"
      - run: oakum release
        env:
          GITHUB_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}"
    )
}

#[cfg(test)]
mod printed_pin {
    use semver::Version;

    use super::{workflow_text, Install, PnpmSetup, WorkflowPins};
    use crate::cli::install_pin::versions_in_workflow;

    /// A pasted workflow passes `check` only if every job's install step is one
    /// `check` reads as the pin, whichever way the repository installs.
    #[test]
    fn check_reads_each_printed_install_step_as_the_pin() {
        let binary = Version::new(1, 2, 3);
        let setup = |version: Option<&str>| PnpmSetup {
            pin: String::from("v8.8.8"),
            version: version.map(String::from),
        };
        for install in [
            Install::Cargo,
            Install::Npm(setup(None)),
            Install::Npm(setup(Some("12.5.1"))),
        ] {
            let pins = WorkflowPins {
                checkout: String::from("v9.9.9"),
                install,
            };
            let workflow = workflow_text(&binary, &pins);
            assert_eq!(
                versions_in_workflow(&workflow),
                Ok(vec![binary.clone(); 3]),
                "{workflow}"
            );
        }
    }
}

#[cfg(test)]
mod install_action_pin {
    /// Renovate bumps the constant, so the step has to print it, in every job.
    #[test]
    fn every_cargo_job_installs_with_the_baked_pin() {
        let pins = super::WorkflowPins {
            checkout: String::from("v9.9.9"),
            install: super::Install::Cargo,
        };
        let workflow = super::workflow_text(&semver::Version::new(1, 2, 3), &pins);
        let step = format!(
            "      - uses: taiki-e/install-action@{}\n",
            super::INSTALL_ACTION_PIN
        );
        assert_eq!(workflow.matches(&step).count(), 3, "{workflow}");
    }
}

#[cfg(test)]
mod package_manager_declaration {
    use std::fs;

    use cap_std::ambient_authority;
    use cap_std::fs::Dir;

    use crate::test_fixture::Fixture;

    /// Detection refuses this shape first, so only a `package.json` swapped
    /// after it (at `--interactive`'s prompt) reaches here; a FIFO swapped in
    /// there hung `pnpm --version`.
    #[test]
    fn a_package_json_that_is_not_a_regular_file_is_unverified() {
        let root = Fixture::new("init", "pm-not-regular");
        fs::create_dir(root.join("package.json")).expect("directory package.json");
        let dir = Dir::open_ambient_dir(&*root, ambient_authority()).expect("dir");
        let err = super::declares_package_manager(&dir).expect_err("refused");
        assert!(err.to_string().contains("is not a regular file"), "{err}");
    }
}

#[cfg(test)]
mod declaration_shapes {
    #[test]
    fn declares_pnpm_mirrors_action_setup() {
        use serde_json::json;
        for declared in [
            json!({ "packageManager": "pnpm@10.0.0" }),
            json!({ "packageManager": "pnpm@10.0.0+sha512.abc" }),
            json!({ "devEngines": { "packageManager": { "name": "pnpm", "version": "10" } } }),
        ] {
            assert!(super::declares_pnpm(&declared), "{declared}");
        }
        for undeclared in [
            json!({}),
            json!({ "packageManager": "" }),
            json!({ "packageManager": "pnpm" }),
            json!({ "packageManager": "pnpm@" }),
            json!({ "packageManager": "npm@10" }),
            json!({ "packageManager": 42 }),
            json!({ "devEngines": { "packageManager": null } }),
            json!({ "devEngines": { "packageManager": "" } }),
            json!({ "devEngines": { "packageManager": { "name": "pnpm" } } }),
            json!({ "devEngines": { "packageManager": { "version": "10" } } }),
            json!({ "devEngines": { "packageManager": [{ "name": "pnpm", "version": "10" }] } }),
        ] {
            assert!(!super::declares_pnpm(&undeclared), "{undeclared}");
        }
    }
}
