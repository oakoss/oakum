# Where an install pin actually lives

- Date: 2026-08-22
- Author: Jace Babin
- Scope: Whether `check` should treat a root `package.json` oakum pin as equal to a GitHub Actions pin, require a workflow pin, or use some other rule.

## Question

okm-24p and [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) say `check` "parses the workflow for oakum's own invocation." The implementation also treats a root `package.json` exact `oakum` dependency as a pin. A matching `package.json` with no `oakum@` in YAML is MATCHING, not NOT FOUND.

Should oakum treat `package.json` as an install-pin source equal to GitHub workflows, require a workflow pin regardless, or use some other rule (for example: any install site; workflow required only when `.github/workflows` exists)?

## Sources

Oakum, read 2026-08-22:

- [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) (full)
- [ADR-0021](../decisions/0021-distribute-through-three-channels.md)
- [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)
- [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md)
- [tool-version-pinning.md](tool-version-pinning.md)
- [cargo-dist-npm-installer.md](cargo-dist-npm-installer.md)
- [guide/github-actions.md](../guide/github-actions.md)
- `crates/oakum/src/cli/install_pin.rs`
- `crates/oakum/src/cli/preconditions.rs`
- `crates/oakum/tests/check.rs`

cargo-dist (`axodotdev/cargo-dist` `main`, fetched 2026-08-22):

- [`cargo-dist/src/lib.rs`](https://raw.githubusercontent.com/axodotdev/cargo-dist/main/cargo-dist/src/lib.rs) — `do_generate_preflight_checks`, `check_integrity`
- [`cargo-dist/src/errors.rs`](https://raw.githubusercontent.com/axodotdev/cargo-dist/main/cargo-dist/src/errors.rs) — `MismatchedDistVersion`
- [`cargo-dist/src/backend/ci/github.rs`](https://raw.githubusercontent.com/axodotdev/cargo-dist/main/cargo-dist/src/backend/ci/github.rs) — `GithubCiInfo::check`
- [`cargo-dist/templates/ci/github/release.yml.j2`](https://raw.githubusercontent.com/axodotdev/cargo-dist/main/cargo-dist/templates/ci/github/release.yml.j2)
- cargo-dist's own [`.github/workflows/release.yml`](https://raw.githubusercontent.com/axodotdev/cargo-dist/main/.github/workflows/release.yml)
- [Config: `cargo-dist-version`](https://axodotdev.github.io/cargo-dist/book/reference/config.html)
- [CLI: `dist generate --check`](https://axodotdev.github.io/cargo-dist/book/reference/cli.html)
- [Customizing CI](https://axodotdev.github.io/cargo-dist/book/ci/customizing.html)
- [Install page](https://axodotdev.github.io/cargo-dist/book/install.html)

JS-first and install-channel sources, fetched 2026-08-22:

- [changesets/action `src/utils.ts`](https://raw.githubusercontent.com/changesets/action/main/src/utils.ts)
- [bumpy `docs/github-actions.md`](https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/github-actions.md)
- [taiki-e/install-action README](https://raw.githubusercontent.com/taiki-e/install-action/main/README.md)
- [cargo-binstall README](https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/README.md)
- [Homebrew `setup-homebrew` README](https://github.com/Homebrew/actions/blob/master/setup-homebrew/README.md)

## Findings

### What oakum's docs say versus what `check` scans

ADR-0007's chosen option is **"exact version in config, refusal on mismatch, read-only verification of the workflow"** ([ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md)). The substitute for cargo-dist generating CI is named as a workflow parse:

> The substitute is read-only: `check` parses the workflow for oakum's own invocation and compares the version it finds, reporting **matching, mismatched, or not found** — never letting "not found" read as fine.

The rejected alternative in the same ADR is **"Pin only at the install site, as knope and semantic-release do."** "Install site" was already in the option set. The chosen wording used "the workflow" because that is where cargo-dist bakes its pin.

[ADR-0003](../decisions/0003-write-only-what-a-command-owns.md) forbids writing CI workflow files. The consequence is explicit: "it removes CI generation as an option, so the tool-version pin must be verified rather than owned." [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md) lists `check` as writing nothing.

[ADR-0021](../decisions/0021-distribute-through-three-channels.md) then names a second install site for the npm channel:

> Good, because a JavaScript repository pins oakum in `devDependencies` and its workflow resolves the pinned version the way bumpy's does, with `jq` and nothing else

That is the intended JS CI shape: the version lives in `package.json`; the workflow does not repeat `oakum@x.y.z`. The same ADR's drivers still say ADR-0007 "requires a workflow to invoke an exact version." That is the tension.

The user-facing guide still describes only the cargo-dist-shaped path ([guide/github-actions.md](../guide/github-actions.md)):

> This finds oakum's own invocation in your workflows and compares the version against `_config.toml`.

The printed example is `cargo binstall --no-confirm oakum@0.1.0`. That page has no `package.json` / `pnpm exec` example.

### What the implementation actually scans

`check` is wired in `crates/oakum/src/cli/preconditions.rs`: after `enforce_tool_version`, if `tool-version` is set it calls `install_pin::verify`.

`crates/oakum/src/cli/install_pin.rs` (module comment, 2026-08-22) states both sources:

> `check` scans `.github/workflows` and the root `package.json` for an exact oakum version and compares it to `tool-version`.

`collect_pins` unions them: every `.yml`/`.yaml` under `.github/workflows`, then optionally one root `package.json` pin. Empty union:

> unverified: no oakum install pin in `.github/workflows` or `package.json`

Workflow recognition is **install lines only**, not every oakum invocation. `is_install_line` returns true for:

- `tool: oakum` / `- tool: oakum` (taiki-e/install-action style)
- a `uses:` line that contains `install-action`
- a `run:` command containing `binstall` or `cargo install`

It does not treat `brew`, `npx`, `pnpm exec`, `pnpm dlx`, or a bare `oakum` invocation as a pin. `bare_oakum_invocation_is_not_a_pin` asserts `run: oakum check` yields no versions. `check_only_workflow_is_not_a_pin` in `crates/oakum/tests/check.rs` asserts that `tool-version` plus only `run: oakum check` is unverified (`no oakum install pin`).

`package.json` is a pin when `oakum` appears as a **string exact version** (optional leading `v`) in `dependencies`, `devDependencies`, `optionalDependencies`, or `peerDependencies`. A range (`^0.1.0`), `latest`, or a non-string value is unverified, not skipped. `package_json_exact_pin_is_collected` asserts a root `"devDependencies":{"oakum":"0.4.2"}` with **no workflow** is a collected pin.

A missing `.github/workflows` directory is not an error (`NotFound` continues). A missing root `package.json` is `Ok(None)`. A JS repo that pins oakum only in `package.json` and invokes it with `pnpm exec oakum check` is MATCHING today. Requiring a YAML `oakum@` would make that repo NOT FOUND.

### cargo-dist does not parse workflows for `crate@version`

cargo-dist's version gate is **binary versus config**, not a YAML scan. `do_generate_preflight_checks` in `cargo-dist/src/lib.rs` (`main`, 2026-08-22):

```text
fn do_generate_preflight_checks(dist: &DistGraph) -> DistResult<()> {
    // Enforce cargo-dist-version, unless...
    //
    // * It's a magic vX.Y.Z-github-BRANCHNAME version,
    // which we use for testing against a PR branch. ...
    //
    // * The user passed --allow-dirty to the CLI (probably means it's our own tests)
    if let Some(desired_version) = &dist.config.dist_version {
        let current_version: Version = std::env!("CARGO_PKG_VERSION").parse().unwrap();
        if desired_version != &current_version
            && !desired_version.pre.starts_with("github-")
            && !matches!(dist.allow_dirty, DirtyMode::AllowAll)
        {
            return Err(DistError::MismatchedDistVersion {
                config_version: desired_version.to_string(),
                running_version: current_version.to_string(),
            });
        }
    }
```

The error (`cargo-dist/src/errors.rs`):

> You're running dist {running_version}, but 'cargo-dist-version = {config_version}' is set in your Cargo.toml

Help: "Rerun 'dist init' to update to this version." No mention of parsing YAML, npm, or Homebrew.

The second check is **generated CI versus disk**. `check_integrity` is commented "(This is currently equivalent to `dist generate --check`)" and calls `run_generate` with `check: true`. `GithubCiInfo::check` regenerates and diffs:

```text
pub fn check(&self, dist: &DistGraph) -> DistResult<()> {
    let ci_file = self.github_ci_release_yml_path();
    let rendered = self.generate_github_ci(dist)?;
    diff_files(&ci_file, &rendered)
}
```

The CLI book: `--check` is "Check if the generated output differs from on-disk config without writing it." The customizing page: since 0.3.0, cargo-dist "will actually consider it an error for there to be any edits or out of date information in release.yml"; `allow-dirty = ["ci"]` opts out.

The workflow pin exists because cargo-dist **writes** the workflow. The template (`release.yml.j2`) has `run: {{{ dist_install_for_coordinator.run }}}`: an installer command generated from `cargo-dist-version`, not a user-authored `crate@` line. cargo-dist's own `release.yml` installs from a versioned URL, not npm:

> `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/axodotdev/cargo-dist/releases/download/v0.32.0/cargo-dist-installer.sh | sh`

The config book (`cargo-dist-version`): "Your release CI will fetch and use the given version of dist to build and publish your project." "The syntax must be a valid Cargo-style SemVer Version (not a VersionReq!)."

**cargo-dist does not look at `package.json` for its own version.** The npm installer templates exist to distribute **the user's binary**, not to install `dist` in CI. [cargo-dist-npm-installer.md](cargo-dist-npm-installer.md) records that `npx oakum` starts Node, loads `run-<bin>.js`, and spawns the downloaded binary. That is how a JS repo *runs* oakum after `pnpm install`, not a second cargo-binstall pin.

The install page lists installer scripts, Homebrew (`brew install axodotdev/tap/cargo-dist`), pacman, Nix, cargo-binstall, and `cargo install`. It does not list `npx cargo-dist` as the way to install dist itself.

### How JS-first tools pin a CLI

**changesets.** The action does not ship the CLI. `src/utils.ts` (2026-08-22):

```ts
function resolveChangesetsCli(cwd: string) {
  return require.resolve("@changesets/cli/bin.js", {
    paths: [cwd],
  });
}
```

`execChangesetsCli` then runs `node` on that path. The pin is the repo's `@changesets/cli` dependency (and lockfile). SHA-pinning the action does not pin the CLI. [tool-version-pinning.md](tool-version-pinning.md) already recorded this; the live `require.resolve` matches.

**bumpy.** The recommended workflow does not contain `@varlock/bumpy@x.y.z` as a second pin. It reads `package.json` and feeds that version to `bunx` ([docs/github-actions.md](https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/github-actions.md), 2026-08-22):

```yaml
- id: bumpy-version
  name: Resolve bumpy version
  run: |
    VERSION=$(jq -r '.devDependencies["@varlock/bumpy"] // .dependencies["@varlock/bumpy"]' package.json | sed 's/[\^~]//')
    echo "version=$VERSION" >> "$GITHUB_OUTPUT"
    echo "BUMPY_VERSION=$VERSION" >> "$GITHUB_ENV"
- id: plan
  run: bunx "@varlock/bumpy@$BUMPY_VERSION" ci plan
```

Comment in that doc: "We just pin its version from package.json and let bunx fetch it." A YAML scanner looking for `bumpy@x.y.z` on a `run:` line would miss this: the version is in `$BUMPY_VERSION`. bumpy has no separate "parse the workflow for bumpy@version" gate.

**cargo-dist npm installer as oakum's JS channel.** After `pnpm install`, `pnpm exec oakum` / `npx oakum` is the invocation. There is no cargo-binstall step on that path unless the repo adds one. ADR-0021's "jq and nothing else" is the bumpy pattern, not a second crates.io pin.

### taiki-e/install-action, cargo-binstall, Homebrew

These are **workflow pins**, not `package.json` pins.

**taiki-e/install-action.** README (2026-08-22): "To install a specific version, use `@version` syntax" with `tool: cargo-hack@0.5.24`. Unlisted tools fall back to cargo-binstall. oakum's `is_install_line` already treats `install-action` / `tool: oakum@…` as a pin.

**cargo-binstall.** README usage is `cargo binstall radio-sx128x@0.14.1-alpha.5`. The first-party action installs *binstall itself* (`uses: cargo-bins/cargo-binstall@main`, optional `version` for binstall). The tool pin is the later YAML `cargo binstall oakum@x.y.z` line, which oakum already scans.

**Homebrew.** cargo-dist's install page uses `brew install axodotdev/tap/cargo-dist`, with no version token. `Homebrew/actions/setup-homebrew` sets up Homebrew; it does not pin formula versions. Typical GHA `brew install jq` is unpinned. Homebrew has no lockfile comparable to `package.json` plus a lockfile. oakum's scanner **does not** treat `brew install oakum` as a pin. A Homebrew-only install with no `oakum@` in YAML and no `package.json` oakum dep is NOT FOUND today.

### The failure mode that decides the rule

**JS repo (ADR-0021 path):** `"oakum": "0.1.0"` in `devDependencies`, workflow `pnpm exec oakum check`, no `oakum@` in YAML.

- Keep `package.json` as a pin source → MATCHING (today).
- Require a workflow `oakum@` → always NOT FOUND. The real install site is npm (`pnpm install` then `pnpm exec`). That is how changesets and bumpy work. It is also what ADR-0021 described.

**Cargo repo with leftover `package.json`:** `"oakum": "0.1.0"` leftover, `tool-version = "0.1.0"`, no workflow install line.

- Keep `package.json` → MATCHING. Narrow: the leftover must be an **exact** version that equals `tool-version`. A range leftover is already unverified.
- This is a repo that declared an npm pin. If CI never installs from it, the gate still saw a committed exact version. The opposite false-fail (every JS repo on the documented npm path) is the common case the three-channel decision exists for.

**"Require a workflow pin whenever `.github/workflows` exists"** still fails the JS path: those repos have workflows; they invoke oakum without repeating the version in YAML. `check_only_workflow_is_not_a_pin` already encodes that `run: oakum check` is not a pin. That is correct for a Cargo repo, and is why the JS path *needs* `package.json`.

A later slice could treat `pnpm exec oakum` / `npx oakum` as using the npm pin, then require `package.json` to match. That does not drop `package.json` as a source.

## Conclusions

**Recommendation (not a decision): keep `package.json` as an install-pin source equal to GitHub workflows.**

cargo-dist never "parses the workflow for crate@version." It compares the running binary to `cargo-dist-version`, then diffs the CI **it generated** from that version. Oakum cannot generate CI ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)), so it looks at install sites. The ADR-0007 phrase "parses the workflow" names cargo-dist's install site. YAML is not the only legal pin.

For every JS-first peer in this survey, the install pin is the **package**. A workflow that runs `pnpm exec oakum` after `pnpm install` has no `oakum@x.y.z` for a YAML scanner to find. Requiring a workflow pin would make the ADR-0021 path permanently NOT FOUND.

taiki-e/install-action, cargo-binstall, and a versioned curl installer are workflow pins. Homebrew in GHA is also a workflow install, usually **unpinned**; it is not a `package.json` pin, and oakum does not scan it today.

The leftover-Cargo-`package.json` MATCH is real and narrow. That MATCH is a false pass only under a spec that requires a YAML pin. ADR-0021's JS path has no YAML pin by design. Do not require a workflow pin in addition to `package.json`, or merely because `.github/workflows` exists.

## Implications / actions

- Accepted for workflows, root `package.json`, and `.mise.toml` / `mise.toml` (`oakum` and `cargo:oakum` under `[tools]`). [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) and [guide/github-actions.md](../guide/github-actions.md) describe those install sites. Homebrew remains a separate slice.
- If rejected: drop `package.json` scanning. JS repos must duplicate the version into YAML, which contradicts ADR-0021's "jq and nothing else."
- Homebrew / `brew install oakum` as a third install site is a separate slice. It is a workflow-shaped pin, not a reason to drop npm.

## Open questions

- Should a later slice treat `pnpm exec oakum` / `npx oakum` as "uses the npm pin" rather than as a non-pin? Today that is correct (`run: oakum check` is not a pin); the `package.json` entry is what makes the JS path MATCHING.
- Whether to scan Homebrew formulae or a Brewfile. Not answered here; those are workflow/tap pins, not `package.json`.
- Nested `package.json` files (workspace members). The scanner reads only the repo-root file, which matches how bumpy's `jq` snippet reads root `package.json`.

## Raw data (optional)

### Comparison: pin location vs what check/generate verifies

| Tool | Where the pin lives | What check / generate verifies |
|---|---|---|
| **cargo-dist** | Config `cargo-dist-version` (mandatory Cargo `Version`, not `VersionReq`). Generated `release.yml` installs that version (curl installer URL or equivalent). | (1) Running binary vs config → `MismatchedDistVersion`. (2) `dist generate --check` regenerates CI and diffs disk. Does **not** parse arbitrary YAML for `dist@`. Does **not** read `package.json` for dist's own version. |
| **oakum (today)** | Config `tool-version`, plus **union** of `.github/workflows` install lines (`binstall` / `cargo install` / `install-action`) and root `package.json` exact `oakum`. | `enforce_tool_version` then `install_pin::verify`. Empty union → unverified. Any collected pin ≠ `tool-version` → unverified. Bare `oakum check` is not a pin. |
| **changesets** | `@changesets/cli` in the repo's `package.json` / lockfile. | Action `require.resolve("@changesets/cli/bin.js", { paths: [cwd] })`. No YAML version of the CLI. |
| **bumpy** | `@varlock/bumpy` in `package.json`; recommended workflow `jq`s it into `bunx "@varlock/bumpy@$VERSION"`. Simple example is unpinned `bunx`. | No separate workflow-version gate. Pin is the package. |
| **semantic-release** | Docs: `npx semantic-release@major`. Own repo pins exact `npx semantic-release@x.y.z`. | Pin is the npx spec / dependency, not cargo-binstall. |
| **knope** | Action `version:` input; empty default resolves latest. | Action input is the pin. SHA-pinning the action does **not** pin the binary. |
| **taiki-e/install-action** | Workflow `tool: <name>@x.y.z`. Unlisted tools → cargo-binstall. | Workflow pin. oakum already scans this shape. |
| **cargo-binstall** | `cargo binstall crate@version` in the workflow (or equivalent). | Workflow pin. oakum already scans `binstall`. |
| **Homebrew in GHA** | `brew install …` in the workflow; formula usually tracks latest in the tap. `setup-homebrew` does not pin formulae. | Workflow install, typically **unpinned**. Not a `package.json` pin. oakum does **not** scan `brew` today. |

### Strongest primary-source lines

ADR-0007: *"The substitute is read-only: `check` parses the workflow for oakum's own invocation and compares the version it finds, reporting matching, mismatched, or not found — never letting \"not found\" read as fine."*

ADR-0021: *"Good, because a JavaScript repository pins oakum in `devDependencies` and its workflow resolves the pinned version the way bumpy's does, with `jq` and nothing else"*

cargo-dist `do_generate_preflight_checks`: compares `dist.config.dist_version` to `CARGO_PKG_VERSION`; error is `MismatchedDistVersion`. No YAML parse.

cargo-dist `GithubCiInfo::check`: `generate_github_ci` then `diff_files`.

changesets: `require.resolve("@changesets/cli/bin.js", { paths: [cwd] })`

bumpy: `VERSION=$(jq -r '.devDependencies["@varlock/bumpy"] // .dependencies["@varlock/bumpy"]' package.json | sed 's/[\^~]//')`

oakum `install_pin.rs`: `bare_oakum_invocation_is_not_a_pin`; `package_json_exact_pin_is_collected`.
