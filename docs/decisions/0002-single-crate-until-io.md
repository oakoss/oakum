# Stay a single crate until the first I/O dependency

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

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

- **The first I/O dependency lands in `[dependencies]`.** `plan` is pure today because nothing it can reach performs filesystem, process, or network I/O. Once something does, nothing structural prevents plan code from calling it, and the invariant degrades silently. Extracting `plan` makes the dependency list enforce purity instead of code review. `[dev-dependencies]` do not count — a test harness cannot be reached from library code, so adding one is not the trigger.
- **Something outside oakum parses its JSON output.** That output becomes a public interface, and its types belong in a crate consumers can depend on without the binary — the reason `cargo-dist-schema` exists.

### Consequences

- Good, because purity is enforced by the compiler at exactly the moment it stops being free
- Good, because moving a module to a crate later is a directory move and a manifest line
- Bad, because the pure-planner rule rests on convention until then; the module's doc comment carries it
- Neutral, because integration tests in `tests/` can exercise the library across the same boundary a separate crate would create, so testability is not a reason to split

### Confirmation

Revisit when `Cargo.toml` gains any dependency capable of filesystem, process, or network access.

## Open questions

**The trigger cannot fire for the I/O this project actually plans.** Discovery is committed to shelling out to `cargo metadata` and `pnpm list`, and that is `std::process::Command` — no dependency, no manifest change, nothing for the trigger to observe. Under the rule as written, `plan` could reach a subprocess while `Cargo.toml` still lists only `semver`, `serde`, and `serde_json`, and the split would never be prompted. That is precisely the silent degradation this ADR exists to prevent.

The mechanism is verified and available: `clippy.toml` with `disallowed-methods` in its inline-table form (which carries a `reason` into the error), `#![deny(clippy::disallowed_methods)]` at the crate root, and `#[allow(clippy::disallowed_methods)]` on `discover` alone. Tested 2026-08-18 — a call in `plan` fails the build, the same call in an allowed module passes. The `#[allow]` then becomes a greppable marker of the I/O boundary, and the trigger becomes: **when a second module needs that attribute, extract `plan`.**

Two caveats keep this a trigger detector rather than the enforcement itself. The denylist is keyed by path, so it catches only what it enumerates: `std::process::Command::new` is caught, and I/O reached through a crate whose path nobody listed is not. And a lint is lint-time where a crate boundary is compile-time. The split remains the real enforcement; the lint is what tells you the trigger fired.

**Higher-order calls are not a caveat.** Clippy [#8849](https://github.com/rust-lang/rust-clippy/issues/8849) was closed as completed on 2022-05-20 by a change that matches *references* to a disallowed function rather than only calls to it. Retested 2026-08-19 on clippy 0.1.97, the version `.mise.toml`'s pinned Rust 1.97.1 supplies: a bare `std::process::Command::new` as a function pointer, a call through `type Cmd = std::process::Command`, and passing the constructor into a generic all three fail the build. Enumerate the path and the lint follows it.
