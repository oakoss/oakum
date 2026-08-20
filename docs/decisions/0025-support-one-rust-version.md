# Support exactly one Rust version

- Status: accepted
- Date: 2026-08-20
- Deciders: Jace Babin

## Context and Problem Statement

`Cargo.toml` declared `rust-version = "1.91"` while `.mise.toml` pinned `rust = "1.97.1"` exactly. Nothing recorded why those differed, and no ADR covered the question — a floor and a ceiling existed as an accident of two files being written at different times. Should oakum support a range of Rust versions, or one?

## Decision Drivers

- A declared floor is a promise to someone. Who holds it, and does it cost them anything to lose it?
- A mechanism that cannot fail is worse than no mechanism; the project already treats silent success as the failure mode to design against ([ADR-0002](0002-single-crate-until-io.md))
- Every dependency must be vetted against a floor, and that vetting is recurring work
- [ADR-0021](0021-distribute-through-three-channels.md) ships prebuilt binaries through shell, PowerShell, Homebrew, and npm, so compiling from source is the minority install path

## Considered Options

- **One version** — raise the floor to the pin, delete the MSRV job and the `check-msrv` task
- **A real floor** — set `rust-version` to the measured minimum and fix `check-msrv` to install it
- **No `rust-version` key at all** — declare nothing

## Decision Outcome

Chosen option: **one version**. `rust-version` is now `1.97.1`, equal to `.mise.toml`'s pin; the `msrv` CI job and the `check-msrv` mise task are deleted; and `.github/renovate.json` keeps the Rust pin out of automerge so raising it stays a reviewed decision.

**The floor was fiction, and measurement is what showed it.** Verified 2026-08-20 with `cargo check --workspace --all-targets --locked --ignore-rust-version`:

| Toolchain | Result |
|---|---|
| 1.85 through 1.90 | compiles |
| 1.84 and below | fails — `toml` and its `serde_spanned`, `toml_datetime`, `toml_parser` all declare `rust-version = 1.85` |

Nothing in oakum required 1.91. Six minor versions of the declared floor protected nothing, and the binding constraint below that came from a **dev-dependency**, which no consumer of the binary ever links. The shipping floor was lower still.

**The declaration is self-enforcing, which is why it could drift unnoticed.** At 1.88 and 1.90 cargo refuses with `requires rustc 1.91` before compiling a line, so the number cannot be falsified by the ordinary build — only by passing `--ignore-rust-version` on purpose. A floor nobody can test against is a number, not a policy.

**The CI job tested the floor; the task of the same name did not.** The job resolved `rust-version` and passed it as `MISE_RUST_VERSION` to both the setup action and the check step, so CI genuinely compiled at 1.91. `mise run check-msrv` set no toolchain of its own, so the same command run locally compiled against the pinned 1.97.1 and reported success — reproduced 2026-08-20, where a bare run finished in 0.68s against cached artifacts while `MISE_RUST_VERSION=1.91` compiled for 3.6s. The consequence was not that the floor went untested but that it was testable in only one place: a developer's green run proved nothing, and the divergence surfaced only after pushing. With floor equal to ceiling that asymmetry stops existing rather than needing a fix.

**Keep the key, raise it.** Setting `rust-version = "1.97.1"` is not the same change as deleting the field. With it, an older toolchain gets `oakum requires rustc 1.97.1`; without it, a cryptic error deep inside a dependency. Only one of those is acceptable, which is why "no key at all" was rejected.

**The counter, answered rather than noted.** With floor equal to pin, every bump of the pin raises the supported floor. `.github/renovate.json` sets `automerge: true` and groups minor and patch updates, so a Rust bump is a minor update that would land unreviewed — Renovate is configured here but has never opened a PR, so this was prospective rather than live. A `packageRules` entry now matches the `mise` manager's `rust` package with `automerge: false` and no group, so the pin moves only through a pull request someone reads. That makes "we support what we test" a decision each time rather than a side effect.

### Consequences

- Good, because it deletes a broken mechanism and a fictional number together, rather than repairing one to protect the other
- Good, because no dependency ever needs vetting against a floor again — a cost the `toml` dev-dependency already incurred once, and the same dependency that turned out to set the real floor
- Good, because CI loses a job that tested a version nothing used
- Bad, because `cargo install oakum` from crates.io now requires the pinned toolchain. ADR-0021 makes source compilation the minority path and those users get a clear error rather than a mystery, but it is a real loss for anyone pinned to an older Rust
- Bad, because the supported floor now moves whenever the pin does. The Renovate rule makes that visible; it does not make it free
- Neutral, because the floor was already effectively the pin for anyone who ran the checks locally — this makes the declaration match what was being tested

### Confirmation

Revisit if oakum is ever depended on as a library rather than installed as a binary, since an MSRV buys downstream consumers something only then. Revisit also if a Renovate pin bump is ever declined for compatibility reasons, which would be evidence that a range is wanted after all.

`crates/oakum/tests/layout.rs::the_declared_floor_equals_the_pinned_toolchain` is what keeps the single version single. Inheritance alone does not: it makes the members agree with the root, while the split returns when the root and `.mise.toml` diverge. That direction is the silent one — a floor *above* the pin makes cargo refuse, a floor *below* it passes every check, and below is the only direction a pin bump produces, since Renovate's cargo manager does not read `rust-version` at all.

**The Renovate rule was verified against Renovate's own source rather than assumed**; the matched fields, the observed outcome, the control, and the ways a copied rule would silently match nothing are recorded in [Renovate rule matching](../research/renovate-rule-matching.md).

## Pros and Cons of the Options

### A real floor at the measured minimum

- Good, because it preserves `cargo install` on toolchains back to 1.85
- Good, because a fixed `check-msrv` that sets `MISE_RUST_VERSION` itself would make local and CI agree by construction — the same argument `.mise.toml` already makes for pinning `rust` exactly rather than using `stable`
- Bad, because the floor would be set by a dev-dependency, so it would move for reasons that have nothing to do with what oakum ships
- Bad, because it keeps the recurring vetting cost for a compatibility guarantee nobody has asked for

### No `rust-version` key at all

- Good, because it is the smallest change
- Bad, because the failure mode is a resolution or compile error inside a dependency rather than a sentence naming the required version
