# Can the extracted `plan` crate be `no_std`?

- Date: 2026-08-19
- Author: Jace Babin
- Scope: whether `#![no_std]` + `alloc` is feasible for `plan`, what it actually enforces, and what it does not

## Question

[ADR-0002](../decisions/0002-single-crate-until-io.md) extracts `plan` so its dependency list enforces purity. [ADR-0024](../decisions/0024-no-std-plan-crate.md) decides whether that extracted crate is `no_std`. Both rest on claims about what a crate boundary and `no_std` each restrict, and those are measurable rather than arguable.

## Sources

Every result below was produced on **rustc 1.97.1** (`aarch64-apple-darwin`), the version `.mise.toml` pins, and re-checked on **1.91.1**, `Cargo.toml`'s `rust-version` floor. Probe crates were built under the scratch directory, never in the repository.

## Findings

### Two channels reach `plan`, and each mechanism closes one

The claim under test: a crate boundary restricts I/O.

**A crate cannot name a dependency absent from its own manifest.** `planlib` lists only `semver`; the binary lists `helper` (a std crate that shells out). `planlib` calling `helper::shell()`:

```text
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `helper`
error: could not compile `planlib` (lib) due to 1 previous error
```

The binary calls `helper::shell()` freely in the same build. So extraction **does** enforce, at build time, against third-party I/O.

**A `no_std` crate can still perform I/O through a std-backed dependency.** A `#![no_std]` + `extern crate alloc` crate depending on that same `helper`:

```rust
#![no_std]
extern crate alloc;
pub fn does_io() { helper::shell(); let _ = helper::now(); }
```

Compiles clean. So `no_std` closes the crate's own naming of `std::`, not I/O as such.

| Channel | Closed by extraction | Closed by `no_std` |
|---|---|---|
| A third-party crate | yes | no |
| `std` named directly | no | yes |

Neither closes the other, which is why [ADR-0024](../decisions/0024-no-std-plan-crate.md) takes both.

### `no_std` rejects every denied path named directly

A `#![no_std]` + `extern crate alloc` crate naming five denylist paths:

```rust
pub fn a() { let _ = std::process::Command::new("x"); }
pub fn b() { let _ = std::fs::read("/x"); }
pub fn c() { let _ = std::net::TcpStream::connect("127.0.0.1:1"); }
pub fn d() { let _ = std::time::SystemTime::now(); }
pub fn e() { let _ = std::env::var("X"); }
```

```text
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `std`   (×5)
error: could not compile due to 5 previous errors
```

`cargo build`, not `cargo clippy` — which is the whole point, since `clippy.toml`'s denylist only fires under the latter.

**One line reopens all of it.** Adding `extern crate std;` beside `extern crate alloc;` returns zero errors. `no_std` is a stronger opt-out than `#[expect]`, not a guarantee.

### The dependency set supports it

A probe crate carrying a miniature cascade gate, graph types, and JSON rendering:

| Dependency | Configuration | Result |
|---|---|---|
| `semver` | `default-features = false, features = ["serde"]` | builds; its own `default = ["std"]`, `std = []` |
| `serde` | `default-features = false, features = ["derive", "alloc"]` | builds |
| `serde_json` | `default-features = false, features = ["alloc"]` | builds; omitting `alloc` is a hard `compile_error!` |

**`semver`'s `serde` feature is required, not optional.** Without it, a `#[derive(Serialize)]` on a struct containing `Version` fails — `Version: Serialize` is unsatisfied, six errors from one derive. The `serde` feature is what supplies the impl.

### What `alloc` provides and withholds

`String`, `Vec`, `BTreeMap`, and `format!` compile. `HashMap` does not exist:

```text
error[E0425]: cannot find type `HashMap` in module `alloc::collections`
```

`hashbrown` — the implementation behind std's `HashMap` — compiles under `no_std` and is one manifest line away, so this is a dependency-count choice rather than a feasibility limit.

`core::error::Error` is available. It stabilised in 1.81 (`#![stable(feature = "error_in_core", since = "1.81.0")]` in the toolchain source), below the 1.91 floor.

### Cargo unifies features across a workspace

The trap: `plan` declaring `semver = { default-features = false }` does **not** hold `std` out of the built artifact if any other member declares plain `semver = "1"`. Cargo resolves one copy with the union of requested features, and `cargo tree` reports it as `FEATURES=default,serde,std`.

Manifest flags in `plan` do not restrict what `plan`'s dependencies contain. Only `#![no_std]` in `plan`'s own source restricts what `plan` may name.

## Conclusions

`no_std` + `alloc` is feasible for `plan` against every dependency it plausibly needs, on both the pinned toolchain and the MSRV floor. It closes the channel that extraction cannot, and extraction closes the channel it cannot. Neither is sufficient alone, and the pair is what makes `[dependencies]` the only remaining route.

## Implications / actions

- `plan`'s graph is a `BTreeMap`, not a `HashMap`. Tracked on `okm-9ho`, which would otherwise be written with `HashMap` and rewritten later.
- Re-run these probes when the pinned Rust version moves, and when `plan` gains a dependency.
- Do not treat `default-features = false` in `plan` as a purity mechanism; it is not one.

## Open questions

- Whether `plan` needs `serde_json` at all, given [ADR-0016](../decisions/0016-emit-release-state-render-it-never-deliver-it.md) puts rendering outside the planner.
- Whether the same probe should cover the crates [implementation stack](implementation-stack.md) names for the non-`plan` modules. They are outside the boundary, so nothing depends on the answer today.
