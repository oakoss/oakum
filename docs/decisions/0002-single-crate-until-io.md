# Stay a single crate until the first I/O dependency

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

**Title note:** the accepted title names the original trigger. See the 2026-08-19 amendment under Decision Outcome — the manifest trigger cannot fire for this project's planned I/O, and the marker trigger replaced it as primary.

## Context and Problem Statement

Every comparable tool is a Cargo workspace: knope has `knope` plus `knope-versioning`, release-plz has `release_plz` plus `release_plz_core`, cargo-dist has `cargo-dist` plus `cargo-dist-schema`. Should oakum start as a workspace or a single crate?

## Decision Drivers

- The planner is specified as a pure function; that property needs to survive contributors
- Boundaries are not yet known, and cross-crate churn while they move is expensive
- A workspace for one developer with one binary is overhead until it pays for something

## Considered Options

- Workspace from the start, mirroring the peers
- Single crate with `lib.rs` and `main.rs`, splitting on an observable trigger

## Decision Outcome

Chosen option: **single crate, with modules named for the crates they would become**. Each peer's split was earned by a specific need and arrived after the tool worked. Splitting for organization is not a reason — modules already do that.

Two triggers would justify a split, and both are observable rather than aesthetic:

- **A second module under `src/` carries an I/O opt-out marker.** Any marker counts today, including one earned for ambient input rather than I/O; the open question below may narrow that. `plan` is pure today because nothing it can reach performs filesystem, process, or network I/O. Once something does, nothing structural prevents plan code from calling it, and the invariant degrades silently. **Amended 2026-08-19: this trigger was originally "the first I/O dependency lands in `[dependencies]`", which cannot fire** — discovery shells out through `std::process::Command`, which needs no dependency and no manifest change. The open question below records the problem; `clippy.toml` now carries the replacement, which denies the call sites instead. A dependency landing in `[dependencies]` still counts where it applies; it is no longer the only thing that does. `[dev-dependencies]` do not count either way — a test harness cannot be reached from library code.
- **Something outside oakum parses its JSON output.** That output becomes a public interface, and its types belong in a crate consumers can depend on without the binary — the reason `cargo-dist-schema` exists.

**Amended 2026-08-19: the manifest gained a second member, and that member is not a split.** `plan-no-std` ships no code of its own — it mounts `plan`'s sources through one `#[path]` and compiles them under [ADR-0024](0024-no-std-plan-crate.md)'s `#![no_std]`, so `cargo` checks the constraint rather than a reviewer. It publishes nothing, and declares only dependencies the shipping crate already lists, so nothing reaches `plan` through a probe that the shipping build does not carry. Neither trigger above has fired; this decision is unchanged. `AGENTS.md` carries the same distinction for anyone reading the tree before the ADRs.

**Amended 2026-08-20: "single crate" now means one shipping crate under the oakoss layout, not a package at the repository root.** The root manifest is virtual — `[workspace]` plus `[workspace.package]`, no `[package]` — and both members live under `crates/`: `crates/oakum` and `crates/plan-no-std`. This matches linesmith and oakterm, verified on disk 2026-08-19, and it landed before more code entered `plan`. Nothing about the triggers changed: one shipping crate, and the probe still is not a second. The `src/` and `tests/` paths named throughout this record are relative to `crates/oakum`.

The move had to keep two mechanisms working, and neither survives by accident. `clippy.toml` stays at the workspace root, where the build still finds it — clippy ascends from `CARGO_MANIFEST_DIR`, which cargo seeds with each unit's package directory, so a denylisted call in `crates/oakum` still fails. (`CLIPPY_CONF_DIR` overrides that starting point and cargo never sets it; the tests set it explicitly, the build does not. Both verified 2026-08-20 on clippy 0.1.97.) And `edition` and `rust-version` had to land in `[workspace.package]` rather than in the member, because two line-anchored greps read the root manifest directly: lefthook's rustfmt hook, which needs the edition since rustfmt reads no manifest, and CI's MSRV job, which installed the floor. ([ADR-0025](0025-support-one-rust-version.md) later deleted that job; the edition grep remains.)

### Consequences

- Good, because the lint enforces purity from the first call site. **Amended 2026-08-19:** this originally read "and the compiler takes over at the split", which the open question below refutes — extraction hands enforcement to the dependency list, and nothing on the denylist arrives through a dependency
- Good, because moving a module to a crate later is a directory move and a manifest line
- **Amended 2026-08-19:** the enumerated entry points are lint-enforced rather than conventional — `clippy.toml` denies them and `tests/io_boundary.rs` proves the denylist is armed. Coverage is still convention: I/O reached through a path nobody listed is not caught, and a path rooted outside `std` cannot even be probed, so the list is a floor rather than the whole invariant
- Neutral, because integration tests in `tests/` can exercise the library across the same boundary a separate crate would create, so testability is not a reason to split

### Confirmation

Revisit when a second module under `src/` needs the opt-out attribute, or when `Cargo.toml` gains any dependency capable of filesystem, process, or network access. `clippy.toml` carries the marker rule, and `tests/io_boundary.rs` proves the denylist is loaded and that every path in it still resolves.

**Counting the markers is not mechanised yet**, so the trigger is a rule someone has to apply rather than a build that goes red. Deferred deliberately: with no marker in the tree, an automated count asserts that a boundary nobody has drawn has not moved, and the natural implementation — put the marker on the `mod` declaration in `lib.rs`, where lint levels scope through to the module file — makes the count a scan of `lib.rs`'s `mod` declarations rather than a source walk. Doing it at the first real marker is cheaper than doing it now and replacing it then. Tracked as `okm-81i`.

**The tripwire has two silent failure modes, and they are why that test exists.** Deleting `clippy.toml` produces no diagnostic at all, and a mistyped or renamed path disarms its entry alone — clippy reports an unresolvable path from its config loader rather than from the lint system, so `-D warnings` does not escalate it and CI stays green. Verified 2026-08-19 on clippy 0.1.97: a denylist naming `std::fs::read_to_strng` exits 0 while a real `read_to_string` call goes unflagged.

## Open questions

**The trigger could not fire for the I/O this project actually plans — resolved 2026-08-19, below.** Discovery is committed to shelling out to `cargo metadata` and `pnpm list`, and that is `std::process::Command` — no dependency, no manifest change, nothing for the trigger to observe. Under the rule as originally written, `plan` could reach a subprocess while `Cargo.toml` still listed only `semver`, `serde`, and `serde_json`, and the split would never be prompted. That was precisely the silent degradation this ADR exists to prevent.

**Resolved 2026-08-19.** `clippy.toml` carries `disallowed-methods` in its inline-table form (which carries a `reason` into the error), `Cargo.toml`'s `[workspace.lints.clippy]` sets the level to `deny`, and a module permitted to perform I/O opts out with an attribute that becomes the marker of the boundary. The trigger is now: **when a second module under `src/` carries that marker, extract `plan`.**

The level is declared in the manifest rather than as `#![deny]` in each crate root because `[lints]` is package-level and reaches every target — a future `src/bin/*.rs` included, which an attribute would leave to CI's `-D warnings` alone.

Details the implementation settled that this ADR had not:

- **`expect`, not `allow`.** An expectation that stops being fulfilled is a warning, and CI runs `-D warnings`, so a marker left on a module that no longer performs I/O fails the build rather than inflating the count.
- **Counting the markers has two failure modes, and the obvious fix for each causes the other.** A substring search counts prose that merely names the attribute — that read 1 with zero real markers on the day this was installed. A line-anchored pattern misses the form `cargo fmt` produces, since it wraps the attribute across four lines once the `reason` string passes ~30 characters, and `cargo fmt --check` runs in CI, so for a realistic reason the wrapped form is the only one that passes. Whatever counts them has to strip comments and match ignoring whitespace.
- **The denylist reaches past I/O into ambient input** — the clock, argv, and the environment. This ADR states purity as "filesystem, process, or network I/O", and those are none of the three; they are denied because [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)'s linesmith replay needs `plan` to be a function of its arguments, and a changelog stamping a release date breaks that exactly as reading an environment variable would. Whether such a marker counts toward the split trigger is open — see below.
- **Binary targets (`src/main.rs`, `src/bin/*`) and `tests/` are excluded.** The binary is where CLI-level I/O belongs, and a test is a separate crate library code cannot reach — the same reasoning that excludes dev-dependencies.

Two caveats keep this a trigger detector rather than the enforcement itself. The denylist is keyed by path, so it catches only what it enumerates: `std::process::Command::new` is caught, and I/O reached through a crate whose path nobody listed is not. And a lint is lint-time where a crate boundary is compile-time — but whether the split adds any enforcement is itself the open question below, since everything on the list is std-backed and available to any crate.

**Resolved 2026-08-19 by [ADR-0024](0024-no-std-plan-crate.md): extraction enforces half of what this ADR claimed.** I/O reaches `plan` through two channels, and extraction closes one. A third-party crate is closed — `plan` cannot name a dependency absent from its own manifest, which is this ADR's premise working exactly as written. `std` directly is not closed, because every crate links `std`, and every path on the denylist today is that kind. So extraction alone would close a channel nothing currently uses and leave open the one everything travels.

This started as a narrower question — whether a marker earned for the clock counts toward the trigger the same as one earned by I/O. It was not narrower. `Command::new` is the case [AGENTS.md](../../AGENTS.md) names as the trigger rationale, and extraction does no more about it than about the clock.

ADR-0024 settles it by making the extracted crate `no_std` with `alloc`, which closes the direct-`std` channel that extraction cannot. The two are complementary rather than alternatives, and together they leave `[dependencies]` as the only route in — the property this ADR wanted the dependency list to have. That ADR carries the measurements, the limits, and the feature-unification caveat.

**What remains open is narrower: whether an ambient-input marker counts toward the trigger.** Any marker counts today. Under ADR-0024 the distinction matters less than it did, since `no_std` closes the clock and the filesystem alike, and the count is deferred to `okm-81i` regardless.

**Higher-order calls are not a caveat.** Clippy [#8849](https://github.com/rust-lang/rust-clippy/issues/8849) was closed as completed on 2022-05-20 by a change that matches *references* to a disallowed function rather than only calls to it. Retested 2026-08-19 on clippy 0.1.97, the version `.mise.toml`'s pinned Rust 1.97.1 supplies: a bare `std::process::Command::new` as a function pointer, a call through `type Cmd = std::process::Command`, and passing the constructor into a generic all three fail the build. Enumerate the path and the lint follows it.
