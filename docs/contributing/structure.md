# Structure

A virtual root manifest with every package under `crates/`, the layout linesmith and oakterm use. `crates/oakum` is the one crate this project ships, with modules named for the crates they would become; the `src/` and `tests/` paths below are relative to it. Split only on a trigger you can observe:

- **A second module under `src/` needs the I/O opt-out attribute.** `plan` is pure today because nothing it can reach touches the filesystem, network, or a subprocess. `clippy.toml` denies those call sites, and a module permitted to reach them opts out with `#[expect(clippy::disallowed_methods, reason = "...")]`; the second module under `src/` to carry that attribute is the trigger. A dependency landing in `[dependencies]` counts too, but it cannot be the only trigger — discovery shells out through `std::process::Command`, which needs no dependency at all. Dev-dependencies, binary targets (`src/main.rs`, `src/bin/*`), and `tests/` are not triggers: library code cannot reach a test harness, and a binary is where CLI-level I/O belongs.
- **Something outside oakum parses its JSON output.** That output is then a public interface, and its types belong in a schema crate consumers can depend on without pulling in the binary.

Splitting for organization alone is not a trigger; modules already do that.

`crates/plan-no-std` sits beside it as a probe rather than a second shipping crate: it compiles `plan`'s own sources under ADR-0024's `#![no_std]`, a constraint the main build cannot express. `publish = false`, a `src/lib.rs` holding nothing but one `#[path]`, and only dependencies `crates/oakum` already lists are what mark it as one — so nothing reaches `plan` through it that the shipping build does not carry. It is neither a split nor a trigger for one.

See [ADR-0002](../decisions/0002-single-crate-until-io.md).
