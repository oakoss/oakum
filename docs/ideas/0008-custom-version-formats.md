# What if version formats beyond plain semver were configurable?

- Status: draft
- Date: 2026-08-19
- Author: Jace Babin
- Promoted to:

## The idea

[ADR-0022](../decisions/0022-zero-major-versioning.md) settled one version-policy question and deliberately left the rest. Two others keep coming up, and they are different in kind.

**Epoch semver.** Anthony Fu's scheme encodes a fourth component into the major position: `{EPOCH * 1000 + MAJOR}.MINOR.PATCH`, so `v2.3.4` at epoch 1 publishes as `1002.3.4`. The argument is that *"humans perceive numbers on a logarithmic scale"* — v1 to v2 reads as monumental while v125 to v126 reads as nothing — so maintainers avoid majors and breaking changes accumulate into rare, large releases. An epoch absorbs the "this is a big deal" signal so the major number can move freely.

**Build metadata.** Appending `+something` to a version, which Cargo's `-sys` ecosystem uses to record the wrapped upstream library: `libgit2-sys` publishes `0.18.7+1.9.6`, `curl-sys` publishes `0.4.90+curl-8.21.0`.

## Why it might matter

Both are things a real repository might want, and one is already a working convention in an ecosystem oakum targets. Neither is exotic — both produce ordinary semver strings that every resolver already understands.

## Sketch

The two need different amounts of work, and [version-format constraints](../research/version-format-constraints.md) is where the evidence lives.

**Epoch is nearly free to allow and awkward to automate.** Nothing in oakum has to change for someone to hand-manage epochs — `1002.3.4` is valid, sorts correctly above `2.0.0`, and cascades normally. Even the arithmetic mostly works out: a breaking change is `+1` on the composite, which is what an ordinary major bump already does. Only the epoch bump is special, rounding up to the next multiple of 1000.

The blocker is expression, not math. Saying "this change is an epoch change" needs a bump level the change-file format does not have, and [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) restricts oakum to the subset all three parsers accept — the same constraint that already rejects `none` as a level. A directive alongside the packages is out too: [specs/bump-files.md](../specs/bump-files.md) permits no key that is not a package in the workspace. So the only route left is config, which cannot express "this particular change is an epoch change" — and that mismatch, rather than the arithmetic, is what makes this a larger design than the feature deserves until someone wants it.

**The zero-cost move is to not preclude it:** nothing may assume a major component below 1000, in parsing, formatting, or display. Same shape as the reachable-tags qualifier in [ADR-0014](../decisions/0014-tags-are-the-version-source-of-truth.md).

**Build metadata is the harder one, and the difficulty is external.** npm and pnpm both strip it at publish while crates.io preserves it, so supporting it as a general feature would mean a Cargo package that keeps the metadata and an npm package that does not. npm at least says so; pnpm strips silently, and pnpm is what every repository here uses.

It also cannot identify anything. Semver §10 makes build metadata invisible to precedence, so `1.2.3+a` and `1.2.3+b` are one version to every resolver, and any increment discards it.

So the honest shape, if it is ever built, is Cargo-only with a **precondition that refuses to plan a `+` version for a package with an npm publish target** — loud, before anything is written, in the style of [ADR-0020](../decisions/0020-one-precondition-path.md). Not a warning after the fact.

## Open questions

- Whether either is wanted by a repository that exists. Neither has a requester today. `README.md`'s non-goals do not name version formats, but they establish the rule these fall under: nothing gets built until a repository the user owns needs it.
- If build metadata is built: where the value comes from. It is dropped by every increment, so it has to be re-derived each release from something — an upstream version in a manifest, a build identifier, a commit — and that source is the actual design.
- Whether the `[packages."<name>"]` table [ADR-0022](../decisions/0022-zero-major-versioning.md) introduced generalizes to a key with no off switch. That table exists because `versioning` needed a real per-package override for graduation, and `versioning` at least goes inert above `1.0.0`. An epoch policy never does, so a workspace mixing epoch and plain versioning would depend on the override permanently rather than during one transition.
- Whether other registries reachable through a custom publish command preserve build metadata. Only npm, pnpm, and crates.io were tested.

## Related work

- [version-format constraints](../research/version-format-constraints.md) — what each ecosystem does, tested rather than assumed
- [ADR-0022](../decisions/0022-zero-major-versioning.md) — the version-policy question that *was* settled
- [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) — why a new bump level is not available
