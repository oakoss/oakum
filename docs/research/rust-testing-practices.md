# Rust unit and integration testing practices for oakum

- Date: 2026-08-21
- Author: research session (oakum)
- Scope: What oakum can improve in unit/integration testing; which crates the Rust community actually uses in 2025–2026; how those options fit ADR-0002 and oakum's existing fixture / foreign-parser suites.

## Question

What can oakum do to improve unit and integration testing? Which testing frameworks and crates does the Rust community actually use as of this research date, and what are community best practices? Every finding below is a recommendation, not an adoption decision.

## Executive summary

Oakum already follows Cargo's unit / integration / doctest split, keeps purity tripwires as tests rather than prose, and snapshots the pure planner with knope-style `in/` + `out/` fixtures. The strongest **recommendations** (not decisions) are:

1. **Keep the fixture-directory pattern for plan and discover** — peers (knope) use the same shape; oakum's harness already matches it.
2. **Treat growing CLI surface as the trigger for a CLI assert crate** (`assert_cmd` / snapbox `cmd`) — today's `tests/cli.rs` is a binary smoke test only.
3. **Cleanup-on-drop for scratch dirs.** Oakum's integration and unit fixtures use a hand-rolled `Fixture` (container under `target/tmp`, sandboxed `gitconfig`, clippy-safe arithmetic) rather than `tempfile`; cleanup-on-drop is landed. Prefer `tempfile` (or `assert_fs`) for simpler scratch dirs that do not need that sandbox.
4. **Evaluate `cargo-nextest` as a runner, not a rewrite** — process-per-test helps isolation for shell-out suites; it is not documented in the Cargo book and would change how shared install caches (foreign parsers) behave across processes.
5. **Add property or table-driven coverage on version/range math when invariants are hard to enumerate** — `proptest` / `rstest` solve that; they do not replace directory fixtures.
6. **Do not adopt `trybuild` for the clippy denylist** — oakum already drives `clippy-driver` in `tests/io_boundary.rs`; trybuild targets rustc diagnostics UI.

All candidate crates are `[dev-dependencies]` or external tools. [ADR-0002](../decisions/0002-single-crate-until-io.md) states that `[dev-dependencies]` and `tests/` do **not** trigger a crate split; the pure `plan` path stays free of them.

## Sources

Accessed **2026-08-21** unless noted.

### Official Rust / Cargo / rustc

- [The Rust Programming Language — Writing Automated Tests](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [The Rust Programming Language — Test Organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html) (unit vs integration; `tests/common/mod.rs` shared helpers; `CARGO_BIN_EXE_<name>`)
- [The Cargo Book — Tests](https://doc.rust-lang.org/cargo/guide/tests.html)
- [The Cargo Book — Cargo Targets: Tests](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#tests) (libtest harness; integration tests; `harness = false`; serial integration-test binaries)
- [The Cargo Book — `cargo test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) — **no `nextest` mention** (confirmed by page grep 2026-08-21)
- [The rustc book — Tests](https://doc.rust-lang.org/rustc/tests/index.html) (`rustc --test`, libtest CLI, filters, `--test-threads`)

### Crates (crates.io + docs.rs; versions as of 2026-08-21)

| Crate | Newest stable | Recent downloads (crates.io) | Primary docs |
| --- | --- | --- | --- |
| `cargo-nextest` | 0.9.143 (published 2026-08-04) | ~1.7M | [crates.io](https://crates.io/crates/cargo-nextest), [nexte.st design: how it works](https://github.com/nextest-rs/nextest/blob/main/site/src/docs/design/how-it-works.md), [why process-per-test](https://github.com/nextest-rs/nextest/blob/main/site/src/docs/design/why-process-per-test.md) |
| `assert_cmd` | 2.2.2 | ~14.9M | [docs.rs/assert_cmd/2.2.2](https://docs.rs/assert_cmd/2.2.2/assert_cmd/) |
| `predicates` | 3.1.4 | ~39.0M | [docs.rs/predicates/3.1.4](https://docs.rs/predicates/3.1.4/predicates/) |
| `assert_fs` | 1.1.4 | ~3.2M | [docs.rs/assert_fs/1.1.4](https://docs.rs/assert_fs/1.1.4/assert_fs/) |
| `tempfile` | 3.27.0 | ~168M | [docs.rs/tempfile/3.27.0](https://docs.rs/tempfile/3.27.0/tempfile/) |
| `insta` | 1.48.0 | ~24.1M | [docs.rs/insta/1.48.0](https://docs.rs/insta/1.48.0/insta/) |
| `rstest` | 0.26.1 (updated 2025-07-27) | ~25.6M | [docs.rs/rstest/0.26.1](https://docs.rs/rstest/0.26.1/rstest/) |
| `proptest` | 1.11.0 | ~44.1M | [docs.rs/proptest/1.11.0](https://docs.rs/proptest/1.11.0/proptest/) |
| `trybuild` | 1.0.120 | ~13.7M | [docs.rs/trybuild/1.0.120](https://docs.rs/trybuild/1.0.120/trybuild/) |
| `snapbox` | 1.2.2 | ~3.0M | [docs.rs/snapbox/1.2.2](https://docs.rs/snapbox/1.2.2/snapbox/) |
| `libtest-mimic` | 0.8.2 | ~4.5M | [docs.rs/libtest-mimic/0.8.2](https://docs.rs/libtest_mimic/) |
| `expect-test` | 1.5.1 | ~6.8M | [crates.io/crates/expect-test](https://crates.io/crates/expect-test) (peer use; not required for oakum today) |

**Cargo itself depends on snapbox.** `rust-lang/cargo` workspace `Cargo.toml` on `master` (fetched 2026-08-21) declares `snapbox = { version = "1.2.0", features = ["diff", "dir", "term-svg", "regex", "json"] }` and `tempfile = "3.27.0"`.

### Oakum constraints and existing tests (on disk 2026-08-21)

- [ADR-0002](../decisions/0002-single-crate-until-io.md): `[dev-dependencies]` and `tests/` do not trigger the split; binary/`tests/` may perform I/O
- [ADR-0024](../decisions/0024-no-std-plan-crate.md): extracted `plan` is `no_std` + `alloc`; fixture harness serde stays outside plan
- `crates/oakum/Cargo.toml`: `[dev-dependencies]` are `changesets = "=0.4.0"` and `toml` only — **no** `insta`, `rstest`, `assert_cmd`, `assert_fs`, `predicates`, `proptest`, `trybuild`, `snapbox`, `tempfile`, or nextest
- Integration suites: `crates/oakum/tests/{cli,io_boundary,layout,no_std_probe,plan_fixtures,changeset_foreign_parsers}.rs` plus `tests/support/mod.rs`
- Gates: `.mise.toml` `[tasks.test]` runs `cargo test --workspace --exclude plan-no-std --all-targets --locked`, then `cargo test --workspace --doc --locked`, then `scripts/test-setup-worktree.sh`; CI's tests job is `mise run test` (`.github/workflows/ci.yml`)

### Peer tools (skim, cited for patterns only)

- **knope** (`knope-dev/knope`, `crates/knope/Cargo.toml` and `tests/` on `main`, 2026-08-21): `dev-dependencies` include `snapbox` (features `path`, `regex`), `tempfile`, `pretty_assertions`. Integration layout is modular under `tests/` with `helpers/test_case.rs` copying `in/` → tempdir, running `snapbox::cmd::Command` against `cargo_bin!("knope")`, asserting against expected files / stdout logs (`dryrun_stdout.log`, `out/`).
- **release-plz** (`release-plz/release-plz`, workspace `Cargo.toml` and `crates/release_plz/tests/all/helpers/cmd.rs` on `main`, 2026-08-21): workspace deps include `assert_cmd`, `tempfile`, `expect-test`, `pretty_assertions`, `wiremock`. CLI helper wraps `assert_cmd::cargo::cargo_bin_cmd!`. Fixture trees under `tests/fixtures/`.
- **changesets** (`changesets/changesets`, `packages/parse/src/index.test.ts` on `main`, 2026-08-21): Vitest unit tests with inline markdown fixtures using `outdent`. Not a Rust pattern, but the same "parser bodies as fixtures" idea oakum's foreign-parser suite already implements by shelling to `@changesets/parse`.

## Findings

### What the official docs say oakum should already be doing

The Book splits tests into **unit** (`#[cfg(test)]` modules next to code; may call private items) and **integration** (`tests/*.rs` as separate crates; public API only). Shared helpers must live in `tests/common/mod.rs` (or equivalent subdirectory), not `tests/common.rs`, or Cargo treats the helper file as its own empty integration target ([Book ch. 11.3](https://doc.rust-lang.org/book/ch11-03-test-organization.html)).

Cargo's target docs add:

- Integration tests link `[dependencies]` **and** `[dev-dependencies]`.
- Each `tests/*.rs` file becomes its **own binary**; Cargo runs those binaries **serially**, while libtest may run `#[test]` functions inside one binary in parallel ([Cargo Targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#tests)).
- Binary crates under test expose `CARGO_BIN_EXE_<name>` to integration tests; oakum's `cli.rs` already uses this.
- `harness = false` on a `[[test]]` opts out of libtest so a custom `main` can drive trials ([same page](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#the-harness-field)); that is the intended hook for `libtest-mimic`.

The rustc book documents the libtest CLI (`--exact`, `--skip`, `--test-threads`, capture behavior) that both `cargo test` and nextest ultimately speak to ([rustc Tests](https://doc.rust-lang.org/rustc/tests/index.html)).

Oakum's `.mise.toml` already encodes a Cargo-book nuance: `--all-targets` does not cover doctests, so a second `cargo test --doc` is required.

### What oakum already does well

| Practice | Where |
| --- | --- |
| Unit tests beside plan / discover / changeset logic | `src/plan/*`, `src/discover/*`, `src/changeset/*` under `#[cfg(test)]` |
| Integration crates for public + structural assertions | `tests/*.rs` with shared `tests/support/` (subdirectory form; Book-correct) |
| Snapshot fixtures for pure compose | `tests/fixtures/plan/*/in\|out` + `plan_fixtures.rs`; documented in fixture README as knope-shaped |
| Foreign-parser confirmation against real peers | `changeset_foreign_parsers.rs` + `fixtures/changeset-foreign` (pnpm + `@changesets/parse`, knope `changesets` crate) |
| Silent-failure tripwires | `io_boundary.rs` (clippy denylist armed), `no_std_probe.rs` (ADR-0024 wiring), `layout.rs` (workspace manifest shape) |
| Gates that do not skip doctests | `mise run test` / CI mirror |
| Dev-deps kept off the ADR-0002 trigger | Explicit comment in `crates/oakum/Cargo.toml` |

The plan fixture design keeps serde **out** of `plan` so `no_std` stays honest: the harness owns JSON, not the pure module. That matches ADR-0024's purity constraint better than pulling `insta`'s serde features into plan itself.

### Gaps and risks specific to oakum

**CLI coverage is thin.** `tests/cli.rs` only checks that the binary runs and prints `oakum`. Peers (knope snapbox `TestCase`, release-plz `assert_cmd`) treat CLI as a first-class fixture surface. As `check` / `version` / explain grow, stdout/stderr and exit codes become the user-visible contract; library unit tests will not catch flag wiring bugs.

**Discover and foreign parsers shell out.** Discovery uses `std::process::Command` (`cargo metadata`, `pnpm list`, …). The foreign-parser suite runs `pnpm install` into a fixture runtime. Risks:

- Parallel libtest threads inside one binary can contend on shared install dirs / metadata caches (the suite already comments that shared target dirs across processes are unsupported).
- nextest's process-per-test model improves isolation but **breaks in-process `OnceLock` install sharing**. Each process would need the same on-disk lock/marker the suite already uses, or installs repeat.
- Failure modes that look like "oakum bug" may be missing Node/pnpm on a machine; the suite should keep distinguishing tool absence from assertion failure (oakum's broader "unverified ≠ ok" invariant).

**Hand-rolled temp directories.** Integration and unit fixtures now use a shared `Fixture` guard (`tests/support/fixture.rs` / `src/test_fixture.rs`) with cleanup-on-drop, a sandboxed `gitconfig`, and leak markers. Simpler scratch dirs that do not need that sandbox can still use `tempfile::TempDir` / `assert_fs::TempDir` ([tempfile docs](https://docs.rs/tempfile/3.27.0/tempfile/), [assert_fs docs](https://docs.rs/assert_fs/1.1.4/assert_fs/)).

**One `#[test]` walks many plan fixtures.** `plan_fixture_suite` loops directories and panics with the case name. That works and keeps compile cost low (one integration binary), but:

- Filtering with `cargo test plan_fixture` cannot select a single case without an env/arg convention.
- Cargo and nextest report one test, not N.
- `libtest-mimic` or a generated module per case would expose each fixture as its own trial, at the cost of binary count / compile time (Cargo warns that many integration files compile slowly).

**Snapshot tooling vs checked-in JSON.** Oakum already snapshots with `assert_eq!` on serde-shaped expected files. `insta` / snapbox / `expect-test` mainly buy **review UX** (`.snap.new`, `cargo insta review`, redactions), not a capability the suite lacks. Adopting them without a workflow for updating `out/plan.json` would duplicate sources of truth.

**Property space on ranges.** Cascade eligibility depends on resolved versions vs declared ranges (ADR-0010 / ADR-0026). Enumerated fixtures catch known shapes; they do not randomly explore grammar edge cases. `proptest` is the community default for that class of bug. Still a recommendation, not a requirement.

### Framework / crate options

| Crate / tool | Problem it solves | Fit for oakum (given ADR-0002 + foreign tools) | Adopt when… | Skip when… |
| --- | --- | --- | --- | --- |
| **libtest + `cargo test`** (stdlib / Cargo) | Collect `#[test]`, parallel within a binary, doctests | Already the gate | Always: stay the source of truth for CI unless nextest is explicitly adopted as the gate | — |
| **cargo-nextest** 0.9.x | Process-per-test; schedule across binaries; retries; clearer failure UX | Dev tool / CI runner only; does not enter `[dependencies]`. Helps isolate shell-out tests; custom harnesses need adaptation | CI time or flaky shared-state tests justify a runner change; team accepts documenting `cargo nextest run` beside or instead of `cargo test` | Suite is small and `cargo test` is fine; foreign-parser install locking has not been re-validated under process-per-test |
| **assert_cmd** 2.2.x | Locate crate binary; assert exit / stdout / stderr; timeouts | Ideal for expanding `tests/cli.rs`; release-plz pattern | CLI flags and user-visible output stabilize | Only smoke-testing the binary |
| **predicates** 3.1.x | Composable boolean matchers for assert_cmd / assert_fs | Comes along with assert_* crates | Assertions need regex / partial stdout / path predicates | Simple `assert_eq!` on full strings suffices |
| **assert_fs** 1.1.x | TempDir + child path setup/assert | Good for version/write tests that mutate trees | File-side-effect CLI or library write paths need fixture sandboxes | Read-only discover fixtures already live under `tests/fixtures/` |
| **tempfile** 3.27.x | RAII temp files/dirs; Cargo and knope use it | Useful for simple scratch dirs; oakum's git fixtures use a hand-rolled `Fixture` instead | Simple scratch without a sandboxed gitconfig | Prefer `Fixture` for suite repos that need git isolation |
| **snapbox** 1.2.x | Snapshot toolbox for data, commands, dirs; redactions; Cargo's own choice | Closest peer match (knope). Overlaps assert_cmd+insta. Strong if oakum wants one toolbox for CLI + file diffs | Building knope-like `in`/`out` CLI harnesses; need `[DATE]`/`[COMMIT]`-style redactions | Plan JSON fixtures stay hand-asserted and CLI stays minimal |
| **insta** 1.48.x | Snapshot files + interactive review (`cargo insta`) | Useful for large Debug/JSON dumps; **do not** pull into `plan` itself | Review workflow for evolving public JSON/text beats editing `out/*.json` by hand | Existing `out/plan.json` files are already the review artifact |
| **expect-test** 1.5.x | Minimal expect-string updates (release-plz) | Lighter than insta | Prefer rust-analyzer inline expects over `.snap` files | Already committed to directory fixtures |
| **rstest** 0.26.x | Fixtures + `#[case]` / `#[values]` tables | Good for bump/cascade matrices inside unit tests | Combinatorial cases are painful as N separate `#[test]`s | Directory-driven suites already enumerate cases |
| **proptest** 1.11.x | Property tests + shrinking | Strong for `DeclaredRange` / semver admission / cascade predicates | An invariant is stated but hard to list exhaustively | Every interesting case is already a named fixture |
| **trybuild** 1.0.x | rustc UI / compile-fail snapshots | **Poor fit** for clippy denylist and `no_std` probe; oakum already uses purpose-built harnesses | Future proc-macros or rustc-facing API misuse messages | Enforcing clippy.toml or plan-no-std wiring |
| **libtest-mimic** 0.8.x | Custom harness that still looks like libtest | Fits dynamic "one trial per fixture directory" without N Cargo targets | Filtering/reporting per plan case becomes painful | Single-loop suite remains readable |

### Peer patterns (skim)

**knope:** One integration entry (`tests/main.rs` modules), shared helpers, per-scenario directories with `in/` / `out/` / stdout logs, `tempfile::TempDir`, snapbox commands + redactions. Oakum's plan fixtures already mirror the directory half; the CLI half is what knope invests in snapbox for.

**release-plz:** `assert_cmd` for the binary, workspace `tests/fixtures/` for multi-crate samples, `expect-test` / `pretty_assertions` for readable diffs, mocks (`wiremock`) for HTTP. Relevant when oakum grows GitHub API code, not for plan purity.

**changesets (JS):** Inline string fixtures in Vitest. Oakum's foreign-parser suite is stricter: it feeds **oakum-written bodies** into both parsers and asserts package names, not merely exit 0. That is closer to confirmation testing than to unit-testing the JS parser itself.

### Community best-practice checklist (mapped to oakum)

Official and peer practice, stated as a checklist with oakum-specific next steps. Items are **recommendations**.

1. **Keep unit tests for algorithms; integration tests for boundaries.**  
   *Next:* Leave bump/cascade/compose unit tests in `src/plan`; keep discover shell-out and clippy/`no_std` tripwires in `tests/`.

2. **Share helpers through `tests/<name>/mod.rs`, not sibling `.rs` files.**  
   *Next:* Already done (`support/`). Preserve that layout when adding helpers.

3. **Doctests are part of the public contract; run them explicitly if using `--all-targets`.**  
   *Next:* Keep the second `cargo test --doc` in `[tasks.test]`; assert that comment if someone "simplifies" the task.

4. **Snapshot user-visible or large structured output; unit-assert small pure returns.**  
   *Next:* Prefer extending `fixtures/plan` / discover fixtures over ad-hoc giant `assert_eq!` strings. Consider snapbox/insta **only** if review friction on `out/` files becomes real.

5. **CLI: assert exit code and streams, not only "binary starts".**  
   *Next:* When `check`/`version` grow, add assert_cmd or snapbox cases; keep using `CARGO_BIN_EXE_oakum` or `Command::cargo_bin`.

6. **Temp resources: RAII cleanup.**  
   *Landed for suite fixtures:* hand-rolled `Fixture` with cleanup-on-drop. *Still optional:* `tempfile`/`assert_fs` for simpler scratch dirs that do not need the git sandbox.

7. **Isolation for global state and foreign tools.**  
   *Next:* Document whether foreign-parser tests may run in parallel; if adopting nextest, re-verify the pnpm install lock under process-per-test.

8. **Property-test invariants; fixture-test examples.**  
   *Next:* Candidate: range admission / path-linked vs authored `*` (ADR-0026) with `proptest`; keep linesmith historical cases as fixtures.

9. **Dev-dependencies stay off the purity trigger.**  
   *Next:* Any new test crate goes in `[dev-dependencies]` only; never into plan's dependency graph (ADR-0002 / ADR-0024).

10. **Do not collapse "tool missing" into "test passed".**  
    *Next:* Foreign-parser and discover suites should keep failing loudly when `pnpm`/`cargo`/clippy-driver are absent, same spirit as `io_boundary`'s unverified path.

## Conclusions

The Rust community's 2025–2026 testing stack around Cargo is still **libtest + `cargo test`**, with an optional runner (**cargo-nextest**) and a small set of mature assert/snapshot crates (**assert_cmd**, **predicates**, **assert_fs**, **tempfile**, **snapbox**, **insta**, **rstest**, **proptest**). Oakum already matches the Book's organization and knope's fixture-directory approach for the pure planner. Fixture RAII is handled by oakum's own `Fixture` guard; the largest remaining gaps are **CLI assertion depth** and **optional runner isolation** for shell-out suites. Missing a unit-test framework for `plan` is not the problem.

This research does not require changing ADR-0002: test crates and `tests/` remain free relative to the split trigger, and plan purity continues to be enforced by clippy + `plan-no-std`, not by which assert crate the harness uses.

## Implications / actions

These are research implications, **not** locked decisions:

- Prefer extending fixture directories and tripwire tests over adding a framework just to have one.
- If/when CLI surface lands, look first at **assert_cmd** (release-plz) or **snapbox** (knope / Cargo); pick one family to avoid overlapping stacks.
- Suite git fixtures use the hand-rolled `Fixture` guard; keep **tempfile** in mind only for simpler scratch dirs that do not need a sandboxed gitconfig.
- Revisit **cargo-nextest** only with a measured CI or flake problem, and re-test foreign-parser install under process-per-test before flipping the mise gate.
- Consider **proptest** for range/cascade invariants once those APIs stabilize; keep historical linesmith cases as fixtures.
- Leave **trybuild** aside unless oakum grows rustc-facing UI needs; do not replace `io_boundary` / `no_std_probe` with it.
- Any adoption should update `.mise.toml` / CI deliberately so local and CI gates cannot drift (existing check/test pattern).

## Open questions

- Should per-fixture **named** tests (libtest-mimic or generated modules) outweigh the compile cost of more integration targets?
- Is snapbox or assert_cmd the better single CLI toolbox once oakum has more than one user-facing command?
- Should foreign-parser tests remain in the default `mise run test` gate on every runner, or move behind a feature / ignored marker when Node is optional?
- Does plan ever expose a stable JSON schema worth doctesting / snapshotting as a public artifact (ADR-0002's second split trigger), and would that change snapshot-tool choice?

## Raw data (optional)

### Oakum gate commands (verbatim from `.mise.toml`, 2026-08-21)

```toml
[tasks.test]
run = [
  "cargo test --workspace --exclude plan-no-std --all-targets --locked",
  "cargo test --workspace --doc --locked",
  "scripts/test-setup-worktree.sh",
]
```

### ADR-0002 excerpt (dev-deps / tests)

> `[dev-dependencies]` do not count either way — a test harness cannot be reached from library code.
>
> Binary targets (`src/main.rs`, `src/bin/*`) and `tests/` are excluded. The binary is where CLI-level I/O belongs, and a test is a separate crate library code cannot reach — the same reasoning that excludes dev-dependencies.

### cargo-nextest execution model (summary)

From nextest's own design doc: `cargo test` runs each **test binary** serially (tests inside a binary may be parallel); nextest lists tests then runs **each test in its own process** in parallel across the suite. Custom harnesses may need adaptation. Process-per-test is an intentional permanent default.
