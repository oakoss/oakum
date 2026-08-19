# Make the extracted `plan` crate `no_std` with `alloc`

- Status: accepted
- Date: 2026-08-19
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0002](0002-single-crate-until-io.md) extracts `plan` into its own crate once a second module needs an I/O opt-out marker, so that the dependency list enforces purity instead of code review. That premise is half true, and the half it gets wrong is the half that matters here.

I/O can reach `plan` through two channels, and extraction closes one of them:

| Channel | Example | Closed by extraction? |
|---|---|---|
| A third-party crate | `ureq::get` | **Yes.** A crate cannot name a dependency absent from its own manifest |
| `std` directly | `std::process::Command::new` | **No.** Every crate links `std` |

Everything on `clippy.toml`'s denylist today is the second kind. So extraction as planned would close a channel nothing currently uses, and leave open the one every denied path travels. What closes the second channel?

## Decision Drivers

- [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)'s linesmith replay requires `plan` to be a function of its arguments; that is what the project's correctness claim rests on
- The lint fires under `cargo clippy` and enforces at lint-time; a build failure is neither
- The denylist is keyed by path and catches only what it enumerates, which ADR-0002 records as a limitation it cannot fix from inside
- Whichever way this goes constrains how `plan` may be written, so deciding after it has code is more expensive than deciding now

## Considered Options

- **`no_std` with `alloc`** — `plan`'s own source cannot name `std::`
- **An ordinary `std` crate** — extraction closes the third-party channel only
- **Do not extract** — retire the split trigger and let the lint be the enforcement permanently

## Decision Outcome

Chosen option: **`no_std` with `alloc`**, because it and extraction are complementary rather than alternatives. Extraction closes the third-party channel; `no_std` closes the direct-`std` channel; neither closes the other. Together, `[dependencies]` becomes the only remaining route for I/O into `plan` — which is exactly the property ADR-0002 wanted the dependency list to have, and could not deliver alone.

**Measured, not assumed.** [no-std plan feasibility](../research/no-std-plan-feasibility.md) carries the probes, commands, and output, on rustc 1.97.1 and re-checked on the 1.91.1 floor. The results this decision rests on:

| Property | Result |
|---|---|
| `Command::new`, `fs::read`, `TcpStream::connect`, `SystemTime::now`, `env::var` named directly | all five rejected, `E0433`, at `cargo build` |
| `String`, `Vec`, `BTreeMap`, `format!` | compile, from `alloc` |
| `HashMap` | not in `alloc`; `BTreeMap` or `hashbrown` replaces it |
| `core::error::Error` | available; stabilised in 1.81, below the 1.91 floor |
| `semver` | works with `features = ["serde"]` — without it, `Version` has no `Serialize` and a derive containing it fails |
| `serde` | works with `default-features = false, features = ["derive", "alloc"]` |
| `serde_json` | works with `default-features = false, features = ["alloc"]` — omitting `alloc` is a hard `compile_error!` |
| One `extern crate std;` line | reopens all of it, zero errors |

**State the limit precisely, because the obvious overstatement is wrong.** `no_std` stops `plan`'s *own source* from naming `std::`. It does not stop I/O: a `no_std` crate that depends on a std-backed crate can shell out through it, verified. What that means is not that the mechanism fails — it means the two channels stay separate, and closing the direct one pushes every remaining route into `[dependencies]`, where it is a manifest line a reviewer sees.

### Consequences

- Good, because enforcement moves from `cargo clippy` to `cargo build` for the direct-`std` channel, which is where every denied path lives today
- Good, because it covers paths the denylist never enumerated. `clippy.toml` catches only what it lists; `no_std` does not need a list
- Good, because it makes ADR-0002's rationale true — not by replacing it, but by removing the channel that bypassed it
- Bad, because it is escapable. One `extern crate std;` reopens everything, so it is a stronger opt-out than `#[expect]`, not a guarantee
- Bad, because `plan` loses `HashMap` and takes `BTreeMap` or a `hashbrown` dependency
- Bad, because a std-backed dependency still reintroduces I/O. The gain is that it becomes visible in the manifest rather than invisible in a function body
- Neutral, because nothing changes until extraction; the constraint lands on how `plan` is written from now, which is the reason to decide now

### Confirmation

The extracted crate compiles under `#![no_std]` with `extern crate alloc`, and a denylist path named in its source fails `cargo build` rather than only `cargo clippy`. If that cannot be achieved at extraction time, the honest response is the third option below rather than a `std` crate described as more than it is.

**A caveat that will mislead someone otherwise:** Cargo unifies features across a workspace build. If `plan` declares `semver = { default-features = false }` while the binary declares plain `semver = "1"`, the single built copy carries `std` anyway — verified. `plan`'s manifest flags do not hold `std` out of its dependencies; only `#![no_std]` in its own source holds `std` out of `plan`.

## Pros and Cons of the Options

### An ordinary `std` crate

- Good, because it costs nothing and constrains no dependency
- Good, because it does close the third-party channel at build time. `plan` could not call `ureq` unless `plan`'s own manifest listed it, verified — this is real enforcement and ADR-0002 was right about it
- Bad, because that channel is empty today and the one in use stays open. Every path on the denylist is reachable from a `std` crate, so the split would change nothing about the I/O that actually threatens `plan`

### Do not extract

- Good, because it is honest if `no_std` proves infeasible: a lint that fires under `cargo clippy` is then the whole enforcement, and saying so beats implying more
- Good, because it deletes the marker-count machinery deferred in `okm-81i` outright
- Bad, because it gives up the third-party channel too, which extraction closes for free
- Weaker than it looked before the channel distinction: extraction buys something real on its own, so "do not extract" now costs more than it appeared to

## More Information

- [no-std plan feasibility](../research/no-std-plan-feasibility.md) — the probes behind every measurement above
- [ADR-0002](0002-single-crate-until-io.md) — the split trigger and the open question this answers
- [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) — the linesmith replay that makes `plan`'s purity load-bearing
- [ADR-0018](0018-own-the-plan-engine.md) — why `plan` is owned rather than borrowed, which is what makes its dependency list oakum's to constrain
- `okm-81i` — the deferred marker count, which decides *when* extraction triggers; this decides what it is worth

**Open:** whether `plan` needs `serde_json` at all. It is listed above as compatible, but [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) puts rendering outside the planner. The feasibility claim does not depend on the answer.

**Open:** whether `hashbrown` is acceptable in `plan` if profiling ever wants hashing. It compiles under `no_std`, so the choice is about dependency count rather than feasibility. `BTreeMap`'s ordered iteration is independently desirable for a planner whose acceptance test is reproducing a recorded history, which is an argument for `BTreeMap` on its own merits rather than a consequence of this decision.
