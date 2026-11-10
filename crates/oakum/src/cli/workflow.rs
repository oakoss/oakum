//! The workflow `init` and `migrate` print for the reader to paste. Oakum
//! writes no CI file (ADR-0003), so this text is the deliverable.

use std::path::Path;

use cap_std::fs::Dir;
use clap::Args;
use oakum::discover::pnpm_version;
use semver::Version;

use super::ci::{VERSION_BRANCH, VERSION_PR_AUTHOR};
use super::fs::read_resolved_text;
use super::github;
use super::install_pin::Owed;
use super::CliError;

/// Each pin is `<commit> # <tag>`, since a tag can move after the paste.
/// JavaScript actions are looked up because a baked-in major goes stale as
/// GitHub retires Node runtimes; [`INSTALL_ACTION_PIN`] is composite.
pub(super) struct WorkflowPins {
    checkout: String,
    upload: String,
    download: String,
    install: Install,
    token: Token,
    /// `actions/create-github-app-token`, looked up only for [`Token::App`].
    app_token: Option<String>,
}

/// The token the printed workflow hands oakum where it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Token {
    /// `secrets.GITHUB_TOKEN`: every run has it, a fork's read-only.
    Default,
    /// A repository secret, with `github.token` behind it: a fork receives
    /// none of the repository's secrets, and an absent one is empty.
    Secret(String),
    /// An App installation token minted in each job that writes.
    App,
}

const APP_CLIENT_ID: &str = "APP_CLIENT_ID";
const APP_PRIVATE_KEY: &str = "APP_PRIVATE_KEY";

/// The token flags `init` and `migrate` share. Two flags, not one with a
/// reserved value, so no secret's name can collide with the App shape.
#[derive(Debug, Args)]
pub(super) struct TokenArgs {
    /// Wire the printed workflow's writing steps to this repository secret,
    /// falling back to the run's own token where it is empty, as on a fork.
    #[arg(long, value_name = "SECRET", value_parser = secret_name, conflicts_with = "app")]
    token: Option<String>,
    /// Print the workflow with a GitHub App token minted in each job that
    /// writes. It needs the `APP_CLIENT_ID` and `APP_PRIVATE_KEY` secrets.
    #[arg(long)]
    app: bool,
}

impl TokenArgs {
    pub(super) fn token(&self) -> Token {
        match (&self.token, self.app) {
            (_, true) => Token::App,
            (Some(name), false) if !name.eq_ignore_ascii_case("GITHUB_TOKEN") => {
                Token::Secret(name.clone())
            }
            (_, false) => Token::Default,
        }
    }

    /// A rerun prints no workflow, so a flag that only shapes it would be
    /// dropped without a word.
    pub(super) fn refuse_without_workflow(&self, command: &str) -> Result<(), CliError> {
        let flag = match (&self.token, self.app) {
            (_, true) => "--app",
            (Some(_), false) => "--token",
            (None, false) => return Ok(()),
        };
        Err(CliError::new(format!(
            "`{flag}` shapes the workflow `{command}` prints, and a repository oakum already manages gets none; run without it"
        )))
    }
}

/// GitHub's rules for a secret's name, which the workflow cannot run without.
fn secret_name(value: &str) -> Result<String, String> {
    let starts = value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !starts
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(String::from(
            "a secret name is letters, digits, and underscores, and does not start with a digit",
        ));
    }
    let upper = value.to_ascii_uppercase();
    if upper.starts_with("GITHUB_") && upper != "GITHUB_TOKEN" {
        return Err(String::from(
            "GitHub reserves secret names that start with `GITHUB_`",
        ));
    }
    Ok(value.to_owned())
}

/// How a job gets oakum onto `ubuntu-latest`, which ships npm but neither pnpm
/// nor `cargo-binstall`.
enum Install {
    /// An npm workspace runs the `@oakoss/oakum` its lockfile pins, after
    /// `pnpm/action-setup`: discovery asks pnpm for the packages.
    Npm(PnpmSetup),
    /// oakum is not in `taiki-e/install-action`'s own manifests, so the action
    /// installs `cargo-binstall` and falls back to it.
    Cargo,
}

/// A composite action, so an older release keeps running. Renovate bumps the
/// commit and the tag together (`.github/renovate.json`). Baked in so the Cargo
/// workflow costs no GitHub request beyond the JavaScript actions'.
const INSTALL_ACTION_PIN: &str = "1919c4a95903d2c33e4a221102edca87309fc14b # v2.89.0";

struct PnpmSetup {
    pin: String,
    /// `pnpm/action-setup` refuses to run with neither a `version` input nor a
    /// `packageManager` (or `devEngines.packageManager`) field, and refuses a
    /// `version` input that disagrees with `packageManager`. Set only when the
    /// action reads no version from `package.json`.
    version: Option<String>,
    /// Whether the root `package.json` lists `@oakoss/oakum`, without which
    /// `pnpm exec oakum` finds no command.
    has_oakum: bool,
}

enum PnpmDeclaration {
    ReadByAction,
    /// A version the action cannot read itself.
    Exact(String),
    Undeclared,
}

/// Why `init` and `migrate` reach GitHub, said when the lookup fails, since
/// nothing else about either command needs the network.
const PIN_LOOKUP_WHY: &str = "the workflow oakum prints pins `actions/checkout`, `actions/upload-artifact`, and `actions/download-artifact` (and `pnpm/action-setup` in a pnpm workspace, `actions/create-github-app-token` under `--app`) to the commits of their latest releases, looked up on GitHub, and nothing is written until it has them; rerun with network access, or set `GITHUB_API_URL` to a GitHub API this machine can reach";

fn action_pin(owner: &str, repo: &str) -> Result<String, CliError> {
    // Any failure leaves the pin unread, so every one is unverified.
    let pin = github::latest_release_pin(owner, repo).map_err(|err| {
        let anonymous = if matches!(err, github::Error::Unauthorized { .. }) {
            "; the lookup sent the token in `GITHUB_TOKEN` or `GH_TOKEN`, and unsetting both makes it anonymous"
        } else {
            ""
        };
        let detail = CliError::from(err).detail();
        CliError::unverified(format!("unverified: {detail}{anonymous}; {PIN_LOOKUP_WHY}"))
    })?;
    Ok(format!("{} # {}", pin.commit, pin.tag))
}

/// What the workflow's install steps need from the repository, read before
/// any prompt: a file swapped while the user answers must not change which
/// workflow prints.
pub(super) enum InstallFacts {
    Npm {
        /// `pnpm/action-setup`'s `version` input, when the action cannot read one.
        version: Option<String>,
        has_oakum: bool,
    },
    Cargo,
}

impl InstallFacts {
    /// `npm` is the caller's discovery decision, so this takes no second look.
    pub(super) fn read(dir: &Dir, repo: &Path, npm: bool) -> Result<Self, CliError> {
        if !npm {
            return Ok(Self::Cargo);
        }
        let manifest = root_manifest(dir)?;
        let declaration = manifest
            .as_ref()
            .map_or(PnpmDeclaration::Undeclared, pnpm_declaration);
        let version = match declaration {
            PnpmDeclaration::ReadByAction => None,
            PnpmDeclaration::Exact(version) => Some(version),
            PnpmDeclaration::Undeclared => Some(pnpm_version(repo).map_err(|err| {
                CliError::unverified(format!(
                    "unverified: pnpm version for the workflow: {err}; declare `packageManager` (`pnpm@<version>`) in package.json so the workflow needs no version input, or fix pnpm on PATH"
                ))
            })?),
        };
        Ok(Self::Npm {
            version,
            has_oakum: manifest.as_ref().is_some_and(lists_oakum),
        })
    }
}

impl WorkflowPins {
    pub(super) fn lookup(facts: InstallFacts, token: Token) -> Result<Self, CliError> {
        let checkout = action_pin("actions", "checkout")?;
        let install = match facts {
            InstallFacts::Npm { version, has_oakum } => {
                let pin = action_pin("pnpm", "action-setup")?;
                Install::Npm(PnpmSetup {
                    pin,
                    version,
                    has_oakum,
                })
            }
            InstallFacts::Cargo => Install::Cargo,
        };
        let upload = action_pin("actions", "upload-artifact")?;
        let download = action_pin("actions", "download-artifact")?;
        let app_token = match token {
            Token::App => Some(action_pin("actions", "create-github-app-token")?),
            Token::Default | Token::Secret(_) => None,
        };
        Ok(Self {
            checkout,
            upload,
            download,
            install,
            token,
            app_token,
        })
    }

    fn token_value(&self) -> String {
        match &self.token {
            Token::Default => String::from("${{ secrets.GITHUB_TOKEN }}"),
            Token::Secret(name) => format!("${{{{ secrets.{name} || github.token }}}}"),
            Token::App => String::from("${{ steps.app-token.outputs.token || github.token }}"),
        }
    }

    /// The App's mint step, asking only for what the job does: with no
    /// `permission-*` input the token carries every permission the App holds.
    fn mint(&self, permissions: &[&str]) -> String {
        let Some(pin) = &self.app_token else {
            return String::new();
        };
        let asked = permissions
            .iter()
            .map(|permission| format!("          permission-{permission}: write"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "      - id: app-token
        uses: actions/create-github-app-token@{pin}
        with:
          client-id: ${{{{ secrets.{APP_CLIENT_ID} }}}}
          private-key: ${{{{ secrets.{APP_PRIVATE_KEY} }}}}
{asked}
"
        )
    }

    /// The release job's push credential: a tag pushed with the run's own
    /// token starts no other workflow.
    fn pushing_checkout(&self) -> String {
        match self.token {
            Token::Default => String::new(),
            Token::Secret(_) | Token::App => format!(
                "          persist-credentials: true\n          token: {}\n",
                self.token_value()
            ),
        }
    }

    fn tagger_step(&self) -> String {
        match &self.token {
            Token::Default => String::from(
                "      # Tags oakum pushes carry this as their tagger; it matches this job's
      # secrets.GITHUB_TOKEN. Swap the token and swap this too. The version
      # commit is written through the GitHub API and carries the token's own
      # account, which no git config here can change.
      - run: |
          git config user.name \"github-actions[bot]\"
          git config user.email \"41898282+github-actions[bot]@users.noreply.github.com\"
",
            ),
            Token::Secret(name) => format!(
                "      # Tags oakum pushes carry this as their tagger: set it to the account
      # behind secrets.{name}. The version commit is written through the GitHub
      # API and carries that account, which no git config here can change.
      - run: |
          git config user.name \"github-actions[bot]\"
          git config user.email \"41898282+github-actions[bot]@users.noreply.github.com\"
"
            ),
            Token::App => String::from(
                "      # Tags oakum pushes carry the App's bot as their tagger.
      - env:
          GH_TOKEN: ${{ steps.app-token.outputs.token }}
          SLUG: ${{ steps.app-token.outputs.app-slug }}
        run: |
          id=\"$(gh api \"/users/${SLUG}%5Bbot%5D\" --jq .id)\"
          git config user.name \"${SLUG}[bot]\"
          git config user.email \"${id}+${SLUG}[bot]@users.noreply.github.com\"
",
            ),
        }
    }

    /// What the token shape needs that `init` cannot set up or check.
    fn token_step(&self) -> Option<String> {
        match &self.token {
            Token::Default => None,
            Token::Secret(name) => Some(format!(
                "the printed workflow reads the `{name}` secret, falling back to the run's own token where it is empty: \
                 store it under that name, and set the release job's `git config` to its account in place of \
                 `{VERSION_PR_AUTHOR}`"
            )),
            Token::App => Some(format!(
                "the printed workflow mints a GitHub App token, which needs the App installed on this repository and \
                 its client ID and private key stored as the `{APP_CLIENT_ID}` and `{APP_PRIVATE_KEY}` secrets; \
                 oakum cannot check either. The `post` job skips Dependabot's pull requests, whose runs get no \
                 Actions secrets; store the two as Dependabot secrets too and you can drop that clause"
            )),
        }
    }

    /// The `check --strict` skip for the version pull request. Only the default
    /// token's login is known here; for a named secret or an App the skip
    /// matches the branch and repository, since a same-repository pusher of
    /// that branch can edit this workflow anyway.
    fn version_pr_skip(&self) -> String {
        let oakum = self.oakum();
        match self.token {
            Token::Default => format!(
                "      # Identity, not a branch name: a fork, a person, another bot, and a push
      # to the bot's branch each fail one of these terms and get checked. The
      # login is the version job's token's account; swap that token for an App's
      # and swap this to the App's `<slug>[bot]`.
      - run: {oakum} check --strict
        if: >-
          github.head_ref != '{VERSION_BRANCH}'
          || github.event.pull_request.head.repo.full_name != github.repository
          || github.event.pull_request.user.login != '{VERSION_PR_AUTHOR}'
          || github.event.sender.login != '{VERSION_PR_AUTHOR}'
"
            ),
            Token::Secret(_) | Token::App => format!(
                "      # The version pull request's branch in this repository: its author is the
      # token's account, which `oakum init` cannot know, and anyone who can push
      # this branch can edit this workflow too.
      - run: {oakum} check --strict
        if: >-
          github.head_ref != '{VERSION_BRANCH}'
          || github.event.pull_request.head.repo.full_name != github.repository
"
            ),
        }
    }

    /// The Cargo step is an install pin `check` reads; the npm workflow's pin is
    /// the `package.json` entry, installed with no lifecycle script run.
    /// `release` checks out with its push credential, so a token is already
    /// held when it installs, and its comment does not claim otherwise.
    fn install_step(&self, binary: &Version, token_held: bool) -> String {
        let when = if token_held {
            ""
        } else {
            ",\n      # before any step holds a token"
        };
        match &self.install {
            Install::Npm(_) => format!(
                "      - run: pnpm install --frozen-lockfile --ignore-scripts
      # oakum's launcher fetches the binary its skipped script would have{when}.
      - run: pnpm exec oakum --version
"
            ),
            Install::Cargo => format!(
                "      - uses: taiki-e/install-action@{INSTALL_ACTION_PIN}\n        with:\n          tool: oakum@{binary}\n"
            ),
        }
    }

    const fn oakum(&self) -> &'static str {
        match self.install {
            Install::Npm(_) => "pnpm exec oakum",
            Install::Cargo => "oakum",
        }
    }

    fn setup_steps(&self) -> String {
        match &self.install {
            Install::Npm(PnpmSetup {
                pin, version: None, ..
            }) => {
                format!("      - uses: pnpm/action-setup@{pin}\n")
            }
            Install::Npm(PnpmSetup {
                pin,
                version: Some(version),
                ..
            }) => format!(
                "      - uses: pnpm/action-setup@{pin}\n        with:\n          version: {version}\n"
            ),
            Install::Cargo => String::new(),
        }
    }

    /// The step an npm repository owes before the printed workflow can run,
    /// whatever else pins oakum: no step of it puts oakum on `PATH`.
    pub(super) fn dependency_step(&self, binary: &Version) -> Option<String> {
        match &self.install {
            Install::Npm(PnpmSetup {
                has_oakum: false, ..
            }) => Some(format!(
                "the printed workflow runs `pnpm exec oakum`, which needs `{NPM_PACKAGE}` in `package.json`: \
                 run `pnpm add -D --allow-build={NPM_PACKAGE} {NPM_PACKAGE}@{binary}` and commit what it changes \
                 (`package.json`, `pnpm-lock.yaml`, and the `allowBuilds` entry in `pnpm-workspace.yaml`); \
                 the workflow's `pnpm install --frozen-lockfile` fails without a committed `pnpm-lock.yaml`"
            )),
            Install::Npm(_) | Install::Cargo => None,
        }
    }

    /// The steps `init` and `migrate` print beside `workflow`. With nothing else
    /// pinning oakum, the missing dependency is the pin step, not a second one.
    pub(super) fn owed_steps(
        &self,
        owed: Option<&Owed>,
        binary: &Version,
        workflow: &str,
    ) -> Vec<String> {
        let dependency = self.dependency_step(binary);
        let pin = owed
            .filter(|owed| dependency.is_none() || owed.refusal().is_some())
            .map(|owed| owed.step(Some(workflow)));
        pin.into_iter()
            .chain(dependency)
            .chain(self.token_step())
            .collect()
    }
}

const NPM_PACKAGE: &str = "@oakoss/oakum";

pub(super) fn npm_workspace(repo: &Path) -> bool {
    repo.join("package.json").is_file() || repo.join("pnpm-workspace.yaml").is_file()
}

/// The root `package.json`; a workspace declared only by `pnpm-workspace.yaml`
/// has none.
fn root_manifest(dir: &Dir) -> Result<Option<serde_json::Value>, CliError> {
    let Some(text) = read_resolved_text(dir, Path::new("package.json"))? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|err| {
        CliError::unverified(format!(
            "unverified: `package.json` is not valid JSON: {err}"
        ))
    })
}

/// Whether `package.json` declares a pnpm version the way `pnpm/action-setup`
/// reads one; anything else makes the action demand its `version` input.
fn pnpm_declaration(manifest: &serde_json::Value) -> PnpmDeclaration {
    if declares_pnpm(manifest) {
        return PnpmDeclaration::ReadByAction;
    }
    array_pnpm_version(manifest).map_or(PnpmDeclaration::Undeclared, PnpmDeclaration::Exact)
}

fn lists_oakum(manifest: &serde_json::Value) -> bool {
    ["dependencies", "devDependencies"].iter().any(|section| {
        manifest
            .get(section)
            .and_then(|deps| deps.get(NPM_PACKAGE))
            .and_then(serde_json::Value::as_str)
            .is_some()
    })
}

/// The action ignores the array form, and asking pnpm can write a lockfile.
/// A range or build metadata is left to the probe.
fn array_pnpm_version(manifest: &serde_json::Value) -> Option<String> {
    let version = manifest
        .get("devEngines")?
        .get("packageManager")?
        .as_array()?
        .iter()
        .find(|pm| pm.get("name").and_then(serde_json::Value::as_str) == Some("pnpm"))?
        .get("version")?
        .as_str()?;
    Version::parse(version)
        .ok()
        .filter(|parsed| parsed.build.is_empty())
        .map(|parsed| parsed.to_string())
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

const COMMENT_DIR: &str = "oakum-comment";

/// The workflow YAML, installing `binary` in a Cargo repository. `check` runs
/// pull-request code without a write token; `post` holds one on the base commit.
pub(super) fn workflow_text(binary: &Version, pins: &WorkflowPins) -> String {
    format!(
        "name: oakum\non:\n  pull_request:\n  push:\njobs:\n{}{}",
        pull_request_jobs(binary, pins),
        default_branch_jobs(binary, pins)
    )
}

fn pull_request_jobs(binary: &Version, pins: &WorkflowPins) -> String {
    let checkout = &pins.checkout;
    let upload = &pins.upload;
    let download = &pins.download;
    let setup = pins.setup_steps();
    let install = pins.install_step(binary, false);
    let oakum = pins.oakum();
    let mint = pins.mint(&["pull-requests"]);
    let token = pins.token_value();
    let skip = pins.version_pr_skip();
    format!(
        "  check:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
          persist-credentials: false
{setup}{install}{skip}      - run: {oakum} ci pr-status --emit-comment {COMMENT_DIR}
        id: pr-status
        if: success() || failure()
        continue-on-error: true
      # A step that failed must not read as one that reported. The `post` job
      # still marks the comment stale from what the step left.
      - run: echo \"::warning title=oakum ci pr-status::the step failed, so its plan may not have reached the job summary or the pull request comment; the check above still decides\"
        if: (success() || failure()) && steps.pr-status.outcome == 'failure'
      - uses: actions/upload-artifact@{upload}
        if: always()
        with:
          name: {COMMENT_DIR}
          path: {COMMENT_DIR}/
  # Holds the write token and runs no pull-request code: it checks out the
  # base commit, and the artifact is data. A fork's token cannot post, nor
  # can a Dependabot run's (read-only, with no Actions secrets), so both skip
  # it and keep the plan in the `check` job's summary.
  post:
    needs: check
    if: >-
      always()
      && github.event_name == 'pull_request'
      && github.event.pull_request.head.repo.full_name == github.repository
      && github.actor != 'dependabot[bot]'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          ref: ${{{{ github.event.pull_request.base.sha }}}}
          persist-credentials: false
{setup}{install}      - uses: actions/download-artifact@{download}
        with:
          name: {COMMENT_DIR}
          path: {COMMENT_DIR}
{mint}      - run: {oakum} ci post-comment {COMMENT_DIR}
        env:
          GITHUB_TOKEN: {token}
"
    )
}

fn default_branch_jobs(binary: &Version, pins: &WorkflowPins) -> String {
    let checkout = &pins.checkout;
    let setup = pins.setup_steps();
    let install = pins.install_step(binary, false);
    let release_install = pins.install_step(binary, true);
    let oakum = pins.oakum();
    let version_mint = pins.mint(&["contents", "pull-requests"]);
    let release_mint = pins.mint(&["contents"]);
    let release_checkout = pins.pushing_checkout();
    let tagger = pins.tagger_step();
    let token = pins.token_value();
    format!(
        "  version:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
          persist-credentials: false
{setup}{install}{version_mint}      - run: {oakum} ci version-pr
        env:
          GITHUB_TOKEN: {token}
  release:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
{release_mint}      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{release_checkout}{setup}{release_install}{tagger}      - run: {oakum} release
        env:
          GITHUB_TOKEN: {token}"
    )
}

#[cfg(test)]
mod printed_pin {
    use semver::Version;

    use super::{workflow_text, Install, PnpmSetup, Token, WorkflowPins};
    use crate::cli::install_pin::versions_in_workflow;

    fn npm(version: Option<&str>, has_oakum: bool) -> Install {
        Install::Npm(PnpmSetup {
            pin: String::from("v8.8.8"),
            version: version.map(String::from),
            has_oakum,
        })
    }

    pub(super) fn pins(install: Install) -> WorkflowPins {
        pins_for(install, Token::Default)
    }

    pub(super) fn pins_for(install: Install, token: Token) -> WorkflowPins {
        let app_token = (token == Token::App)
            .then(|| String::from("5555555555555555555555555555555555555555 # v5.5.5"));
        WorkflowPins {
            checkout: String::from("9999999999999999999999999999999999999999 # v9.9.9"),
            upload: String::from("7777777777777777777777777777777777777777 # v7.7.7"),
            download: String::from("6666666666666666666666666666666666666666 # v6.6.6"),
            install,
            token,
            app_token,
        }
    }

    /// A pasted Cargo workflow passes `check` only if every job's install step
    /// is one `check` reads as the pin.
    #[test]
    fn check_reads_each_printed_cargo_install_step_as_the_pin() {
        let binary = Version::new(1, 2, 3);
        let workflow = workflow_text(&binary, &pins(Install::Cargo));
        assert_eq!(
            versions_in_workflow(&workflow),
            Ok(vec![binary.clone(); 4]),
            "{workflow}"
        );
    }

    fn job<'a>(workflow: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        workflow["jobs"]
            .get(name)
            .unwrap_or_else(|| panic!("no `{name}` job: {workflow}"))
    }

    fn step_running<'a>(job: &'a serde_json::Value, command: &str) -> &'a serde_json::Value {
        job["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|step| {
                step["run"]
                    .as_str()
                    .is_some_and(|run| run.contains(command))
            })
            .unwrap_or_else(|| panic!("no step runs `{command}`: {job}"))
    }

    /// The pull request's code runs where no write token is, and the token is
    /// held only on the base commit, after the install.
    #[test]
    fn the_comment_is_emitted_by_check_and_posted_from_the_base_commit() {
        for install in [Install::Cargo, npm(None, true)] {
            let text = workflow_text(&Version::new(1, 2, 3), &pins(install));
            let workflow: serde_json::Value = serde_saphyr::from_str(&text).expect("YAML");
            let check = job(&workflow, "check");
            assert_eq!(
                check["permissions"]["pull-requests"],
                serde_json::Value::Null,
                "{text}"
            );
            assert_eq!(
                step_running(check, "ci pr-status")["run"]
                    .as_str()
                    .and_then(|run| run.split_once("ci pr-status")),
                Some((run_prefix(&text), " --emit-comment oakum-comment")),
                "{text}"
            );
            assert!(
                !text
                    .split("  post:\n")
                    .next()
                    .unwrap_or_default()
                    .contains("GITHUB_TOKEN"),
                "{text}"
            );

            let post = job(&workflow, "post");
            assert_eq!(post["needs"], "check", "{text}");
            let guard = post["if"].as_str().unwrap_or_default();
            for term in [
                "always()",
                "github.event_name == 'pull_request'",
                "github.event.pull_request.head.repo.full_name == github.repository",
                "github.actor != 'dependabot[bot]'",
            ] {
                assert!(guard.contains(term), "{term}: {guard}");
            }
            assert_eq!(post["permissions"]["pull-requests"], "write", "{text}");
            let steps = post["steps"].as_array().expect("steps");
            assert_eq!(
                steps[0]["with"]["ref"], "${{ github.event.pull_request.base.sha }}",
                "{text}"
            );
            let posting = steps
                .iter()
                .position(|step| {
                    step["run"]
                        .as_str()
                        .is_some_and(|run| run.ends_with("ci post-comment oakum-comment"))
                })
                .expect("a post-comment step");
            assert_eq!(posting, steps.len() - 1, "posting is the last step: {text}");
            assert!(steps[posting]["env"]["GITHUB_TOKEN"].is_string(), "{text}");
        }
    }

    fn run_prefix(text: &str) -> &'static str {
        if text.contains("pnpm exec oakum") {
            "pnpm exec oakum "
        } else {
            "oakum "
        }
    }

    /// The npm workflow names no version, so `package.json` stays the one pin.
    #[test]
    fn the_npm_workflow_runs_the_lockfile_pin_and_names_no_version() {
        let binary = Version::new(1, 2, 3);
        for install in [npm(None, true), npm(Some("12.5.1"), true)] {
            let workflow = workflow_text(&binary, &pins(install));
            assert_eq!(versions_in_workflow(&workflow), Ok(vec![]), "{workflow}");
            assert_eq!(
                workflow
                    .matches("      - run: pnpm install --frozen-lockfile --ignore-scripts\n")
                    .count(),
                4,
                "{workflow}"
            );
            for command in [
                "check --strict",
                "ci pr-status --emit-comment oakum-comment",
                "ci post-comment oakum-comment",
                "ci version-pr",
                "release",
            ] {
                assert!(
                    workflow.contains(&format!("      - run: pnpm exec oakum {command}\n")),
                    "{command}: {workflow}"
                );
            }
            assert!(!workflow.contains("run: oakum "), "{workflow}");
            assert!(!workflow.contains(" npm i"), "{workflow}");
        }
    }

    /// `--ignore-scripts` skips oakum's own download too, and the launcher
    /// fetches on its first run, so a tokenless run must come first.
    #[test]
    fn the_npm_binary_is_fetched_before_any_step_holds_a_token() {
        for token in [
            Token::Default,
            Token::Secret(String::from("RELEASE_PAT")),
            Token::App,
        ] {
            let text = workflow_text(&Version::new(1, 2, 3), &pins_for(npm(None, true), token));
            fetched_before_a_token(&text);
            // `release` checks out with its push credential before anything else.
            let release = text.split("  release:\n").nth(1).expect("release job");
            assert!(!release.contains("before any step holds a token"), "{text}");
        }
    }

    fn fetched_before_a_token(text: &str) {
        let workflow: serde_json::Value = serde_saphyr::from_str(text).expect("YAML");
        for name in ["check", "post", "version"] {
            let steps = workflow["jobs"][name]["steps"].as_array().expect("steps");
            let first_run = steps
                .iter()
                .position(|step| {
                    step["run"]
                        .as_str()
                        .is_some_and(|run| run.starts_with("pnpm exec oakum"))
                })
                .expect("an oakum step");
            assert_eq!(
                steps[first_run]["run"], "pnpm exec oakum --version",
                "{name}"
            );
            assert!(steps[first_run].get("env").is_none(), "{name}: {text}");
            let holds_token = steps
                .iter()
                .position(|step| {
                    step["env"].get("GITHUB_TOKEN").is_some()
                        || step["id"] == "app-token"
                        || step["with"].get("token").is_some()
                })
                .unwrap_or(steps.len());
            assert!(first_run < holds_token, "{name}: {text}");
        }
    }

    #[test]
    fn a_missing_dependency_is_the_pin_step_only_when_nothing_else_pins() {
        use crate::cli::install_pin::Owed;
        use crate::cli::CliError;

        let binary = Version::new(1, 2, 3);
        let missing = Owed::Missing(CliError::unverified("unverified: no oakum install pin"));
        let unsettled = Owed::Unsettled(CliError::unverified("unverified: install pins disagree"));
        let owed =
            |install, owed: Option<&Owed>| pins(install).owed_steps(owed, &binary, "jobs: {}\n");

        let [step] = &owed(npm(None, false), Some(&missing))[..] else {
            panic!("no pin and no dependency is one step");
        };
        assert!(step.contains("pnpm add -D"), "{step}");
        let [refusal, step] = &owed(npm(None, false), Some(&unsettled))[..] else {
            panic!("unsettled pins and no dependency are two steps");
        };
        assert_eq!(refusal, "install pins disagree");
        assert!(step.contains("pnpm add -D"), "{step}");
        assert_eq!(
            owed(npm(None, true), Some(&missing)),
            ["no oakum install pin"]
        );
        assert_eq!(
            owed(Install::Cargo, Some(&missing)),
            ["no oakum install pin"]
        );
        assert_eq!(owed(npm(None, true), None), Vec::<String>::new());
    }

    #[test]
    fn only_an_npm_repository_without_the_dependency_is_told_to_add_it() {
        let binary = Version::new(1, 2, 3);
        let step = pins(npm(None, false))
            .dependency_step(&binary)
            .expect("a missing dependency is a step");
        assert!(
            step.contains("`pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@1.2.3`"),
            "{step}"
        );
        assert_eq!(pins(npm(None, true)).dependency_step(&binary), None);
        assert_eq!(pins(Install::Cargo).dependency_step(&binary), None);
    }

    /// Only `release` pushes; every other job leaves no token behind for git.
    #[test]
    fn only_the_release_checkout_keeps_its_credential() {
        let workflow = workflow_text(&Version::new(1, 2, 3), &pins(Install::Cargo));
        let release = workflow.split("  release:\n").nth(1).expect("release job");
        let before = workflow.split("  release:\n").next().expect("other jobs");
        assert_eq!(
            before.matches("persist-credentials: false").count(),
            3,
            "{workflow}"
        );
        assert!(!release.contains("persist-credentials"), "{workflow}");
    }
}

#[cfg(test)]
mod token_shapes {
    use clap::Parser;
    use semver::Version;
    use serde_json::Value;

    use super::printed_pin::pins_for;
    use super::{workflow_text, Install, Token, TokenArgs};

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        token: TokenArgs,
    }

    fn parse(args: &[&str]) -> Result<Token, clap::Error> {
        Cli::try_parse_from(std::iter::once("init").chain(args.iter().copied()))
            .map(|cli| cli.token.token())
    }

    fn workflow(token: Token) -> (String, Value) {
        let text = workflow_text(&Version::new(1, 2, 3), &pins_for(Install::Cargo, token));
        let value = serde_saphyr::from_str(&text).unwrap_or_else(|err| panic!("{err}\n{text}"));
        (text, value)
    }

    fn steps<'a>(workflow: &'a Value, job: &str) -> &'a [Value] {
        workflow["jobs"][job]["steps"]
            .as_array()
            .map_or(&[], Vec::as_slice)
    }

    fn position(steps: &[Value], matches: impl Fn(&Value) -> bool) -> usize {
        steps.iter().position(matches).expect("a matching step")
    }

    fn runs(step: &Value, command: &str) -> bool {
        step["run"]
            .as_str()
            .is_some_and(|run| run.contains(command))
    }

    const WRITERS: [(&str, &str); 3] = [
        ("post", "ci post-comment"),
        ("version", "ci version-pr"),
        ("release", "oakum release"),
    ];

    #[test]
    fn the_flags_name_a_secret_or_the_app_and_never_both() {
        assert_eq!(parse(&[]).expect("no flag"), Token::Default);
        assert_eq!(
            parse(&["--token", "GITHUB_TOKEN"]).expect("the default's name"),
            Token::Default
        );
        assert_eq!(
            parse(&["--token", "RELEASE_PAT"]).expect("a secret"),
            Token::Secret(String::from("RELEASE_PAT"))
        );
        assert_eq!(parse(&["--app"]).expect("the App"), Token::App);
        let both = parse(&["--token", "RELEASE_PAT", "--app"]).expect_err("conflict");
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
        for name in ["1PAT", "MY-PAT", "MY PAT", "", "GITHUB_PAT", "github_app"] {
            assert!(parse(&["--token", name]).is_err(), "{name:?}");
        }
    }

    #[test]
    fn the_default_reads_the_runs_own_token_at_every_writer() {
        let (text, value) = workflow(Token::Default);
        for (job, command) in WRITERS {
            let step = &steps(&value, job)[position(steps(&value, job), |s| runs(s, command))];
            assert_eq!(
                step["env"]["GITHUB_TOKEN"], "${{ secrets.GITHUB_TOKEN }}",
                "{job}"
            );
        }
        for absent in [
            "github.token",
            "app-token",
            "persist-credentials: true",
            "token: $",
        ] {
            assert!(!text.contains(absent), "{absent}: {text}");
        }
    }

    #[test]
    fn a_named_secret_falls_back_to_the_runs_own_token_and_pushes_the_tags() {
        let (text, value) = workflow(Token::Secret(String::from("RELEASE_PAT")));
        let expression = "${{ secrets.RELEASE_PAT || github.token }}";
        for (job, command) in WRITERS {
            let step = &steps(&value, job)[position(steps(&value, job), |s| runs(s, command))];
            assert_eq!(step["env"]["GITHUB_TOKEN"], expression, "{job}");
        }
        let checkout = &steps(&value, "release")[0];
        assert_eq!(checkout["with"]["token"], expression, "{text}");
        assert_eq!(checkout["with"]["persist-credentials"], true, "{text}");
        assert!(!text.contains("secrets.GITHUB_TOKEN"), "{text}");
        assert!(!text.contains("app-token"), "{text}");
    }

    /// Mint after the install, so no token exists while packages install,
    /// except in `release`, whose checkout needs the token to push with.
    #[test]
    fn the_app_mints_in_each_writing_job_with_only_its_permissions() {
        let (text, value) = workflow(Token::App);
        for (job, command, permissions) in [
            ("post", "ci post-comment", &["pull-requests"][..]),
            (
                "version",
                "ci version-pr",
                &["contents", "pull-requests"][..],
            ),
            ("release", "oakum release", &["contents"][..]),
        ] {
            let steps = steps(&value, job);
            let mint = position(steps, |s| s["id"] == "app-token");
            let writer = position(steps, |s| runs(s, command));
            assert!(mint < writer, "{job}: {text}");
            assert_eq!(
                steps[writer]["env"]["GITHUB_TOKEN"],
                "${{ steps.app-token.outputs.token || github.token }}",
                "{job}"
            );
            let with = steps[mint]["with"].as_object().expect("with");
            assert_eq!(with["client-id"], "${{ secrets.APP_CLIENT_ID }}", "{job}");
            assert_eq!(
                with["private-key"], "${{ secrets.APP_PRIVATE_KEY }}",
                "{job}"
            );
            let mut asked: Vec<&str> = with
                .keys()
                .filter_map(|key| key.strip_prefix("permission-"))
                .collect();
            asked.sort_unstable();
            assert_eq!(asked, permissions, "{job}");
            let install = position(steps, |s| {
                s["uses"]
                    .as_str()
                    .is_some_and(|uses| uses.starts_with("taiki-e/"))
            });
            if job == "release" {
                assert_eq!(mint, 0, "{text}");
                assert_eq!(
                    steps[1]["with"]["token"],
                    "${{ steps.app-token.outputs.token || github.token }}",
                    "{text}"
                );
            } else {
                assert!(install < mint, "{job}: {text}");
            }
        }
        assert!(
            value["jobs"]["post"]["if"]
                .as_str()
                .is_some_and(|guard| guard.contains(
                    "github.event.pull_request.head.repo.full_name == github.repository"
                )),
            "{text}"
        );
        assert!(
            text.contains("git config user.name \"${SLUG}[bot]\""),
            "{text}"
        );
        assert!(!text.contains("secrets.GITHUB_TOKEN"), "{text}");
    }

    #[test]
    fn only_a_flag_owes_a_step_and_the_app_names_its_secrets() {
        let binary = Version::new(1, 2, 3);
        let owed = |token| pins_for(Install::Cargo, token).owed_steps(None, &binary, "");
        assert_eq!(owed(Token::Default), Vec::<String>::new());
        let [secret] = &owed(Token::Secret(String::from("RELEASE_PAT")))[..] else {
            panic!("one step for a named secret");
        };
        assert!(secret.contains("`RELEASE_PAT` secret"), "{secret}");
        let [app] = &owed(Token::App)[..] else {
            panic!("one step for the App");
        };
        assert!(
            app.contains("`APP_CLIENT_ID` and `APP_PRIVATE_KEY` secrets"),
            "{app}"
        );
        for step in [secret, app] {
            assert!(
                !step.contains("check --strict"),
                "nothing to edit there: {step}"
            );
        }
    }

    /// The default token's login is known, so its skip trusts that identity;
    /// a named secret's or an App's is not, so theirs match branch and
    /// repository and run as pasted.
    #[test]
    fn the_version_pr_skip_names_a_login_only_for_the_default_token() {
        for (token, terms) in [
            (Token::Default, 4),
            (Token::Secret(String::from("RELEASE_PAT")), 2),
            (Token::App, 2),
        ] {
            let (text, value) = workflow(token);
            let guard = steps(&value, "check")
                .iter()
                .find(|step| runs(step, "check --strict"))
                .and_then(|step| step["if"].as_str())
                .expect("a guarded check step")
                .to_owned();
            assert_eq!(guard.matches("||").count() + 1, terms, "{guard}");
            assert!(
                guard.starts_with("github.head_ref != 'oakum/version-packages'")
                    && guard.contains(
                        "github.event.pull_request.head.repo.full_name != github.repository"
                    ),
                "{guard}"
            );
            assert_eq!(guard.contains("github-actions[bot]"), terms == 4, "{text}");
        }
    }
}

#[cfg(test)]
mod oakum_dependency {
    use serde_json::json;

    #[test]
    fn a_dependency_or_dev_dependency_on_the_scoped_package_counts() {
        for listed in [
            json!({ "devDependencies": { "@oakoss/oakum": "1.2.3" } }),
            json!({ "dependencies": { "@oakoss/oakum": "^1" } }),
        ] {
            assert!(super::lists_oakum(&listed), "{listed}");
        }
        for unlisted in [
            json!({}),
            json!({ "devDependencies": { "oakum": "1.2.3" } }),
            json!({ "peerDependencies": { "@oakoss/oakum": "1.2.3" } }),
            json!({ "devDependencies": { "@oakoss/oakum": null } }),
            json!({ "dependencies": { "@oakoss/oakum": false } }),
        ] {
            assert!(!super::lists_oakum(&unlisted), "{unlisted}");
        }
    }
}

#[cfg(test)]
mod install_action_pin {
    /// Renovate bumps the constant, so the step has to print it, in every job.
    #[test]
    fn every_cargo_job_installs_with_the_baked_pin() {
        let pins = super::printed_pin::pins(super::Install::Cargo);
        let workflow = super::workflow_text(&semver::Version::new(1, 2, 3), &pins);
        let step = format!(
            "      - uses: taiki-e/install-action@{}\n",
            super::INSTALL_ACTION_PIN
        );
        assert_eq!(workflow.matches(&step).count(), 4, "{workflow}");
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
        let Err(err) = super::root_manifest(&dir) else {
            panic!("a directory package.json was read");
        };
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

    #[test]
    fn only_an_exact_pnpm_version_in_an_array_is_read() {
        use serde_json::json;
        let array =
            |entries: serde_json::Value| json!({ "devEngines": { "packageManager": entries } });
        for (manifest, read) in [
            (
                array(json!([{ "name": "pnpm", "version": "12.15.0" }])),
                Some("12.15.0"),
            ),
            (
                array(
                    json!([{ "name": "npm", "version": "10.0.0" }, { "name": "pnpm", "version": "11.0.0-rc.1" }]),
                ),
                Some("11.0.0-rc.1"),
            ),
            (array(json!([{ "name": "pnpm", "version": "^12" }])), None),
            (array(json!([{ "name": "pnpm", "version": "12" }])), None),
            (
                array(json!([{ "name": "pnpm", "version": "12.15.0+sha512.abc" }])),
                None,
            ),
            (array(json!([{ "name": "pnpm" }])), None),
            (array(json!([{ "name": "npm", "version": "10.0.0" }])), None),
            (
                json!({ "devEngines": { "packageManager": { "name": "pnpm", "version": "12.15.0" } } }),
                None,
            ),
            (json!({}), None),
        ] {
            assert_eq!(
                super::array_pnpm_version(&manifest).as_deref(),
                read,
                "{manifest}"
            );
        }
    }
}
