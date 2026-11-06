# Structure

A virtual root manifest with every package under `crates/`, the layout linesmith and oakterm use. `crates/oakum` is the one crate this project ships, with modules named for the crates they would become; the `src/` and `tests/` paths below are relative to it. Split only on a trigger you can observe:

- **A Rust program outside oakum depends on its JSON output.** That output is then a public interface, and its types belong in a schema crate consumers can depend on without pulling in the binary. A workflow reading `status --json` cannot depend on a crate; a JSON Schema serves it.

I/O is not a trigger. `crates/plan-no-std` keeps `plan` pure (below), and `clippy.toml` denies the I/O entry points across the rest of the library, while a module permitted to reach them opts out with `#[expect(clippy::disallowed_methods, reason = "...")]`. The binary (`src/main.rs`) is where CLI-level I/O belongs.

The `oakum` library's `pub` surface is the seam between the library and the binary that ships beside it, not a curated API — `main.rs` compiles as its own crate, so anything the CLI reads must be `pub` whether or not an outside caller was ever meant to reach it. Treat an item's presence there as "the binary needed it," and reach for a narrower visibility only when the binary does not. A genuine external interface is the JSON-output trigger above, not a `pub fn`.

Splitting for organization alone is not a trigger; modules already do that.

`crates/plan-no-std` sits beside it as a probe rather than a second shipping crate: it compiles `plan`'s own sources under ADR-0024's `#![no_std]`, a constraint the main build cannot express. `publish = false`, a `src/lib.rs` holding nothing but one `#[path]`, and only dependencies `crates/oakum` already lists are what mark it as one — so nothing reaches `plan` through it that the shipping build does not carry. It is neither a split nor a trigger for one.

See [ADR-0002](../decisions/0002-single-crate-until-io.md).

## Integration tests

`tests/it/main.rs` is the one integration test binary; each suite is a module declared there. Add a suite as a module, not as a new `tests/*.rs` file — Cargo builds every top-level file as its own binary, and each one relinks on every library change. Filter with `cargo test -p oakum --test it <module>::`; libtest matches by substring, so `cli::` also selects `add_cli::` and every other `*_cli::` suite. Narrow it with a test name, as in `cli::binary`.

## Test fixtures

Integration tests build repositories through `tests/it/support/fixture.rs`; unit tests use `src/test_fixture.rs`. Both write a marker (`.oakum-fixture` / `.oakum-unit-fixture`) into a container under Cargo's `target/tmp` and remove that container on drop. Set `OAKUM_TEST_RETAIN` to keep marked containers for debugging. `mise run test` clears stale `oakum-*` scratch under `target/tmp` before the suite (keeping `oakum-changeset-foreign`), ends with `scripts/fixture-leak-check.sh` (fails on marked leftovers and unmarked `oakum-*`), and on a green leak check clears that scratch again so CI's rust-cache does not re-ship litter.

The suites spawn the `git`, `cargo` and `pnpm` that `PATH` resolves, and the integration tests' hermetic `PATH` links those same binaries. Run them through `mise run test`, which puts mise's installs on `PATH`. A bare `cargo test` in a shell that resolves tools through mise's shims fails in any copy whose `.mise.toml` is untrusted (`mise ERROR Config files in … are not trusted`); run `mise trust` on the copy first.
