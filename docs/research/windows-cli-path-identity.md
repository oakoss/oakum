# Windows CLI spawn and path identity

- Date: 2026-09-02
- Author: research session (oakum)
- Scope: Why the Windows CI job on [#169](https://github.com/oakoss/oakum/pull/169) has failed a new class each round, and which remaining hazards are the same class versus a new one. Not an ADR.

## Question

The Windows job (`mise run check` / `audit` / `test` on `windows-latest`) has been green up to a new panic each push. One-cause-per-round made each log diagnosable and also spent a full job per class. What do the platform docs and this repository actually say, and which leftover failures should the next try treat as one class?

## Sources

Accessed **2026-09-02** unless noted.

### Rust std (stable docs, fetched 2026-09-02)

- [`std::process::Command`](https://doc.rust-lang.org/stable/std/process/struct.Command.html) — Windows search order; `.exe` may be omitted; other extensions must be included ([rust-lang/rust#37519](https://github.com/rust-lang/rust/issues/37519))
- [`std::fs::canonicalize`](https://doc.rust-lang.org/stable/std/fs/fn.canonicalize.html) — on Windows, extended-length (`\\?\`) syntax
- [`Path::display`](https://doc.rust-lang.org/stable/std/path/struct.Path.html#method.display) — `Display` of the path; separators are the platform's
- [`Path` equality](https://doc.rust-lang.org/stable/std/path/struct.Path.html) — component-wise structural equality, not filesystem identity

### This repository (on disk 2026-09-02, `82cafcb`)

- `crates/oakum/src/discover/pnpm.rs` (`pnpm_command`, PATH × PATHEXT, mise fallback)
- `crates/oakum/tests/support/mod.rs` (`command_on_path`)
- `crates/oakum/src/cli/add.rs` (`discover_workspace`, `find_manifest_dir`, `repo_path_display`)
- `crates/oakum/src/cli/repository.rs` (`discover_from` canonicalizes; `ambient_path` is that canonical path)
- `crates/oakum/src/cli/fs.rs` (`normalized_windows_path`)
- `crates/oakum/src/cli/install_pin.rs` (workflow path via `Path::new(".github/workflows").join(&name)` then `path.display()`)
- `crates/oakum/src/test_fixture.rs` / `crates/oakum/tests/support/fixture.rs` (`CARGO_TARGET_TMPDIR`)
- `crates/oakum/tests/io_boundary.rs` (comment: `CARGO_TARGET_TMPDIR` is inside the repository)
- `crates/oakum/tests/check.rs` (`matching_package_json_pin_without_workflow_is_ready`, `unversioned_install_action_tool_pin_is_unverified`)
- `crates/oakum/tests/migrate_cli.rs` (`migrate_on_tty` / `python3` + `pty`, not `#[cfg(unix)]`)
- `crates/oakum/tests/init_cli.rs` (`init_on_tty` is `#[cfg(unix)]`)

### Measured: GitHub Actions

Run [33704539855](https://github.com/oakoss/oakum/actions/runs/33704539855) job [Windows](https://github.com/oakoss/oakum/actions/runs/33704539855/job/100490753556) on `82cafcb` (`fix(cli): find mise node.exe the same way as pnpm on Windows`):

| Target | Result |
|---|---|
| lib tests | 684 passed |
| bin tests | 380 passed |
| `add_cli` | 24 passed |
| `cargo_lockfile` | 7 passed |
| `changeset_foreign_parsers` | 15 passed |
| `check` | 72 passed, **2 failed** |

`mise run check` and `mise run audit` on that job succeeded.

## Findings

### Class 1 — spawn by short name (PATHEXT, mixed PATH, mise)

Rust's Windows `Command` searches PATH itself. The stable docs say the `.exe` extension may be omitted; files with other extensions must include the extension. That is narrower than cmd.exe's PATHEXT walk (`.COM;.EXE;.BAT;.CMD`).

Git Bash is this job's `defaults.run.shell`. Inferred: that shell mixed Win32 dirs with MSYS spellings (`/c/Users/...`) in earlier `program not found` panics on this branch; run [33704539855](https://github.com/oakoss/oakum/actions/runs/33704539855) does not print `PATH`.

Already fixed in this PR for production `pnpm` and for test `command_on_path("pnpm"|"node")`. `Command::new("cargo")` in discovery has not failed on this job: `cargo.exe` is on a Win32 PATH entry after the rustup setup.

`cmd /C <name>` is refused: it searches the working directory for `*.cmd` / `*.bat`.

### Class 2 — `\\?\` versus non-verbatim path equality (current failure)

`repository::discover_from` canonicalizes. On Windows that yields an extended-length path (`\\?\D:\...`). `discover_workspace` then walks `std::env::current_dir()` toward `repo.ambient_path()`:

```rust
if dir == stop || !dir.pop() {
    return None;
}
```

`Path` equality is structural. `D:\a\oakum\oakum\target\tmp\...\pin-npm` and `\\?\D:\a\oakum\oakum\target\tmp\...\pin-npm` are not equal, so the walk does not stop at the fixture git root. The next `Cargo.toml` is the checkout workspace. `ensure_contained` then correctly reports that workspace as outside the fixture.

Measured stderr from `matching_package_json_pin_without_workflow_is_ready`:

```text
error: workspace discovery failed (cargo: workspace root \\?\D:\a\oakum\oakum is outside repository \\?\D:\a\oakum\oakum\target\tmp\oakum-check-pin-npm-8468-27\pin-npm)
```

This test is the only `check` fixture in that file that writes `package.json` and **no** `Cargo.toml`. Fixtures that call `cargo_package` hit `Cargo.toml` on the first `find_manifest_dir` iteration and never need `dir == stop`. That is why 72 other `check` tests passed on the same job.

`CARGO_TARGET_TMPDIR` sits inside the cargo target dir, which sits inside the checkout. `io_boundary.rs` already records that. On Unix the same walk-past would also find the checkout `Cargo.toml` if `current_dir` and the canonical repo path compared unequal (for example `/var/folders` vs `/private/var/folders`). Inferred: Darwin CI has not shown this failure. Windows `canonicalize` always adds `\\?\` ([std::fs::canonicalize](https://doc.rust-lang.org/stable/std/fs/fn.canonicalize.html)).

This is a production Windows bug, not only a fixture bug: `find_manifest_dir` can walk out of the git root whenever cwd and the canonical root differ only by a verbatim prefix.

`normalized_windows_path` already strips `\\?\` / `\\?\UNC\` for symlink containment. `find_manifest_dir` does not use it.

**Inferred, not measured on a local Windows box:** the `dir == stop` mismatch is the mechanism that matches the log. A Windows debugger was not attached.

### Class 3 — `Path::display` uses `\` (current failure)

`Path::new(".github/workflows").join("ci.yml")` on Windows displays as `.github/workflows\ci.yml`.

Measured stderr from `unversioned_install_action_tool_pin_is_unverified`:

```text
error: unverified: `.github/workflows\ci.yml` installs oakum without a version
```

The assertion is `stderr.contains(".github/workflows/ci.yml")`.

`add.rs` already has `repo_path_display` (`replace('\\', '/')`) for bump-file paths after `add_cli` failed the same way. `install_pin.rs` still uses `path.display()` for repo-relative workflow paths. `discover::paths::repo_relative` already rewrites `\` to `/` for package directories.

### Class 4 — Unix-only tools still compiled into Windows tests

Not failed on this job (test order stopped at `check`). Inventory of `Command::new` of Unix names **without** `#[cfg(unix)]` on the caller:

| Site | Name | Windows outlook |
|---|---|---|
| `tests/migrate_cli.rs` `migrate_on_tty` | `python3` + `import pty` | Inferred: `pty` is Unix-only (Python `pty` docs). `init_cli.rs` already gates the same helper. |
| `tests/fixture_probe.rs` `a_fixture_is_usable_everywhere_a_path_is` | `true` | Constructs `Command`, does not spawn. |
| `tests/check.rs` / `release_cli.rs` | `sh` | Already `#[cfg(unix)]`. |
| `tests/detect_cli.rs` / `config_cli.rs` | `mkfifo` | Already `#[cfg(unix)]`. |

Do not treat these as the same class as 2 or 3.

## Conclusions

`gh run list --repo oakoss/oakum --branch test/windows-containment --workflow CI` (2026-09-02) listed every CI run on this branch as `failure`. Only [33704539855](https://github.com/oakoss/oakum/actions/runs/33704539855) has a failed Windows log quoted in this note. Earlier runs, from `git log` subjects plus that `gh run list`:

| SHA | Run | Commit subject |
|---|---|---|
| `8e38f24` | [33704008738](https://github.com/oakoss/oakum/actions/runs/33704008738) | print bump-file paths with forward slashes |
| `ad375db` | [33703388210](https://github.com/oakoss/oakum/actions/runs/33703388210) | treat NT UNC symlink targets as Win32 UNC paths |
| `ccb4a63` | [33699501892](https://github.com/oakoss/oakum/actions/runs/33699501892) | find mise pnpm.exe when Git Bash PATH is mixed |
| `d5bf2ed` | [33696443257](https://github.com/oakoss/oakum/actions/runs/33696443257) | resolve pnpm via PATHEXT and JSON-escape Windows paths |
| `76cf205` | [33694476837](https://github.com/oakoss/oakum/actions/runs/33694476837) | point cargo-deny at rustup's cargo on Windows |
| `0568a4e` | [33693559217](https://github.com/oakoss/oakum/actions/runs/33693559217) | gate unix-only check helpers for Windows clippy |

Inferred from those subjects, not from re-fetched logs: clippy and cargo-deny PATH, then JSON/`/tmp`/mixed PATH/`pnpm.exe`, then UNC and `add` slashes, then mise `node.exe`, then the two `check` failures quoted above.

Classes 2 and 3 already failed in the same job log. Fixing only one still fails `check` and costs another full Windows run.

Class 2 is the one that leaks a parent cargo workspace on a real Windows git repo whose cwd spelling is not the `\\?\` form `discover_from` stored. Class 3 is user-facing output.

## Implications / actions

Recommendations, not decisions:

- Next try should cover **both already-failed `check` classes** in one change: stop `find_manifest_dir` on a Windows-normalized identity, and print repo-relative pin paths with `/` the way `repo_path_display` / `repo_relative` already do.
- Gate `migrate_on_tty` on `unix` in a later change, matching `init_on_tty`. Do not mix that into the `check` fix; it has not failed yet and is a different class.
- Do not `cmd /C` to find tools. Do not batch remaining Unix commands into the path-identity fix.

## Open questions

- Whether `current_dir()` on GHA `windows-latest` is exactly the non-verbatim form (inferred from the log, not printed).
- Whether `Command::new("cargo")` needs the PATHEXT walker once a job runs without rustup's Win32 PATH (not observed).
- Whether other user-facing `path.display()` calls on repo-relative `Path::join` results (changelog, version, write_set) will fail later assertions. Only `install_pin` + `add` have been measured.

## Raw data

Windows job test order on `82cafcb` (from the failed log): lib → bin → `add_cli` → `cargo_lockfile` → `changeset_foreign_parsers` → `check` (fail, suite stops).
