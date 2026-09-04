# Windows CLI spawn and path identity

- Date: 2026-09-02 (updated 2026-09-03)
- Author: research session (oakum)
- Scope: Why the Windows CI job on [#169](https://github.com/oakoss/oakum/pull/169) failed a new class each round, what shipped in that PR, and which hazards remain for round 2. Not an ADR.

## Question

The Windows job (`mise run check` / `audit` / `test` on `windows-latest`) went from red every push to green after several one-class fixes. What do the platform docs and this repository say, and which leftover failures should round 2 treat as one class?

## Sources

Accessed **2026-09-02** unless noted; post-merge updates **2026-09-03**.

### Rust std (stable docs, fetched 2026-09-02)

- [`std::process::Command`](https://doc.rust-lang.org/stable/std/process/struct.Command.html) — Windows search order; `.exe` may be omitted; other extensions must be included ([rust-lang/rust#37519](https://github.com/rust-lang/rust/issues/37519))
- [`std::fs::canonicalize`](https://doc.rust-lang.org/stable/std/fs/fn.canonicalize.html) — on Windows, extended-length (`\\?\`) syntax
- [`Path::display`](https://doc.rust-lang.org/stable/std/path/struct.Path.html#method.display) — `Display` of the path; separators are the platform's
- [`Path` equality](https://doc.rust-lang.org/stable/std/path/struct.Path.html) — component-wise structural equality, not filesystem identity

### This repository

- `crates/oakum/src/discover/pnpm.rs` (`pnpm_command`, PATH × PATHEXT, mise fallback)
- `crates/oakum/tests/support/mod.rs` (`command_on_path`)
- `crates/oakum/src/cli/add.rs` / `install_pin.rs` (`repo_path_display`)
- `crates/oakum/src/cli/fs.rs` (`normalized_windows_path`, `repo_path_display`, containment)
- `crates/oakum/src/cli/repository.rs` (`discover_from` canonicalizes; `ambient_path` is that canonical path)
- `crates/oakum/tests/support/changeset_foreign.rs` (rust-cache yaml hole repair)
- `crates/oakum/tests/migrate_cli.rs` (`migrate_on_tty` is `#[cfg(unix)]` on `main` after #169)
- `crates/oakum/tests/init_cli.rs` (`init_on_tty` is `#[cfg(unix)]`)

### Measured: GitHub Actions

**Last red before path-identity fixes** — run [33704539855](https://github.com/oakoss/oakum/actions/runs/33704539855) on `82cafcb`: lib/bin/add/cargo_lockfile/changeset_foreign green; `check` 72 passed, 2 failed (classes 2 and 3 below).

**Green after #169** — merge commit `413703f` (`feat(cli): map loopback admin UNC onto the drive letter`); `805cf27` Windows job green twice on re-run.

## Findings

### Class 1 — spawn by short name (PATHEXT, mixed PATH, mise) — shipped #169

Rust's Windows `Command` searches PATH itself. The stable docs say the `.exe` extension may be omitted; files with other extensions must include the extension.

Git Bash on GHA mixed Win32 dirs with MSYS spellings in earlier panics. Fixed for production `pnpm` / test `command_on_path("pnpm"|"node")` and mise `node.exe` discovery.

`cmd /C <name>` is refused: it searches the working directory for `*.cmd` / `*.bat`.

### Class 2 — `\\?\` versus non-verbatim path equality — shipped #169

`repository::discover_from` canonicalizes to extended-length paths on Windows. `find_manifest_dir` compared `dir == stop` structurally, so a fixture cwd without `\\?\` walked past the git root into the checkout workspace.

**Fix:** normalize both sides with `normalized_windows_path` before comparing.

### Class 3 — `Path::display` uses `\` on repo-relative paths — shipped #169 + round 2 tier 1

Measured stderr from `unversioned_install_action_tool_pin_is_unverified` before fix:

```text
error: unverified: `.github/workflows\ci.yml` installs oakum without a version
```

`repo_path_display` (forward slashes, matching git) covers `install_pin`, `add`, `version`, `write_set`, `inherited`, `changelog`, `intent`, and containment errors in `fs`.

Absolute paths in `repository::confirm_ambient` and user-provided `--emit-comment` dirs still use platform `display()`.

### Class 4 — Unix-only tools in Windows test binaries — partially shipped

| Site | Name | Status on `main` |
|---|---|---|
| `tests/migrate_cli.rs` `migrate_on_tty` | `python3` + `pty` | `#[cfg(unix)]` (matches `init_on_tty`) |
| `tests/fixture_probe.rs` | `true` | Constructs `Command`, does not spawn |
| `tests/check.rs` / `release_cli.rs` | `sh` | `#[cfg(unix)]` |
| `tests/detect_cli.rs` / `config_cli.rs` | `mkfifo` | `#[cfg(unix)]` |

### Class 5 — changeset-foreign rust-cache partial restore (yaml hole) — shipped #169

`Swatinem/rust-cache` can restore a workspace `node_modules` tree missing `yaml` / `@changesets/types` junctions. `ensure_js_deps` now verifies reachability from the parse entry after `canonicalize`; on failure it wipes `node_modules` and runs `pnpm install --frozen-lockfile` (which alone can report “Already up to date” while leaving the hole).

Repair assertion lives only in `changeset_foreign_parsers` via `assert_yaml_hole_is_repaired()` — not as a `#[test]` in shared `support` (avoids 26× redundant `pnpm install`).

### Class 6 — Windows pnpm `yaml` junction delete — shipped #169

`remove_existing` falls back to `fs::remove_dir` when `remove_file` fails on junction/reparse points (Access Denied on GHA `windows-latest`).

## Conclusions

PR #169 merged the containment, spawn, UNC, yaml-hole, and primary path-display fixes. Windows CI is green on `main`.

Round 2 tier 1 (`okm-3l8.1`, `okm-3l8.2`, `okm-5b8`) extended `repo_path_display` through version/write paths, refreshed this note, and silenced known-unifiable `cargo-deny` duplicate subgraphs.

Tier 2 (`okm-3l8.3`, `okm-3l8.4`) lands NTFS case-insensitivity and repo-internal junction/symlink containment tests in `cli/config.rs` (and a string-level path-component case unit in `cli/fs.rs`). The portable unit is measured on Darwin. The three `#[cfg(windows)]` integrations passed on `windows-latest` in [CI run 33821168808](https://github.com/oakoss/oakum/actions/runs/33821168808) on `528d0da` (PR #172).

## Implications / actions

Recommendations, not decisions:

- **Done (#169):** normalized manifest-dir stop, `repo_path_display` for workflow/add paths, yaml-hole repair, junction punch, `windows-latest` CI job.
- **Done (round 2 tier 1):** `repo_path_display` through version/write paths; this doc; `deny.toml` skip-tree for `fs-set-times`, skip for `syn`, `multiple-versions = deny`.
- **Done (round 2 tier 2):** path-component case folding against a real absolute internal config symlink; directory junction (mklink /J) and absolute file symlink containment via `open_config` / `resolve_capability_path` — measured green on `windows-latest` (run 33821168808).
- **Tier 3 (optional):** revisit rust-cache policy for changeset-foreign; optional Windows-only path filter on CI.

Do not `cmd /C` to find tools.

## Open questions

- Whether other CLI surfaces (absolute ambient paths, `--emit-comment` output paths) should also normalize separators for copy/paste ergonomics.
- Whether `Command::new("cargo")` needs the PATHEXT walker on a job without rustup's Win32 PATH (not observed).

## Raw data

Windows job test order on the last red run (`82cafcb`): lib → bin → `add_cli` → `cargo_lockfile` → `changeset_foreign_parsers` → `check` (fail, suite stops).

CI run subjects on branch `test/windows-containment` before merge (from `git log` + `gh run list`, 2026-09-02): clippy/PATH → pnpm/PATHEXT → UNC/add slashes → mise node → check failures (classes 2–3) → yaml hole → junction fix → green.
