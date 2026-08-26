# Pin the tool version in config and refuse to run on mismatch

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Oakum's version determines bump math, changelog output, and manifest writes. If CI resolves "latest" each run, release behavior can change with no commit in the repository. How is that prevented?

## Decision Drivers

- Behavior must not change without a commit someone reviewed
- Config must never need upkeep to stay correct
- Oakum does not write CI workflows ([ADR-0003](0003-write-only-what-a-command-owns.md)), so it cannot own the pin the way cargo-dist does

## Considered Options

- Pin only at the install site, as knope and semantic-release do
- Declare an exact version in config, refuse to run on mismatch, and verify install pins read-only

## Decision Outcome

Chosen option: **exact version in config, refusal on mismatch, read-only verification of install sites**.

`tool-version` is an exact version, never a range — a range reintroduces a resolution step, which is the drift being prevented. Every command except `upgrade` refuses to run when it disagrees with the binary, in **both** directions, naming the upgrade command. _(Amended 2026-08-22: that every-command gate stays until `version` exists. After that, it applies to commands that write, not to `check` or `status`. See the amendment below.)_ `upgrade` validates against the old schema, runs migrations, writes the new version, regenerates the schema, and reports what changed — writing nothing if migration fails, since a half-migrated config is worse than a stale one.

cargo-dist supplies this model, including the mandatory `Version`-not-`VersionReq` rule and refusal in both directions. It gets there partly by generating CI with the version baked in, which ADR-0003 rules out here. The substitute is read-only. `check` looks at **install sites**: versioned install lines in GitHub workflows, an exact root `package.json` `oakum` dependency, and an exact `oakum` / `cargo:oakum` pin in `.mise.toml` / `mise.toml`. It compares what it finds to `tool-version` and reports **matching, mismatched, or not found**. A missed look is never treated as fine. `run: oakum check` is an invocation, not a pin.

Two additions cargo-dist lacks:

- **Unknown config keys are a hard error**, enforced by `deny_unknown_fields`. Of eight tools surveyed only release-plz enforces this; changesets silently drops unknown keys as deliberate policy, and bumpy declares `additionalProperties: false` in a schema its loader never reads.
- **The version is stamped into the output** — the version-PR body and the changelog footer — so a bad release identifies the version that produced it.

The JSON Schema is written as a generated sibling of the config and referenced with taplo's `#:schema ./_schema.json` directive, so it tracks the installed binary. changesets' version-pinned unpkg URL freezes at init; release-plz's unversioned URL tracks `main` rather than your binary.

### Consequences

- Good, because behavior provably cannot change without a reviewed commit
- Good, because a schema change appears as a diff in the upgrade commit rather than as silently different editor behavior
- Bad, because every patch release forces an `upgrade` commit in every consuming repository; a Renovate bump will fail CI until that commit lands
- Neutral, because that failure is the gate working as designed — the alternative is a silent behavior change

### Confirmation

Never self-upgrade in CI. That would convert a loud failure back into the silent change this decision exists to prevent.

## Amendment (2026-08-22)

Self-hosting collides with the gate: the binary that cuts oakum 0.2.0 is 0.1.0. Bumping `tool-version` to 0.2.0 in that same release would refuse every later command until the install pin can fetch a binary that does not exist yet.

**No exemption.** cargo-dist (source, 2026-08-18) gates `dist generate` rather than every command, and exempts a `vX.Y.Z-github-BRANCHNAME` prerelease plus `--allow-dirty`. Oakum does not copy the escape hatch. When `version` lands, the binary-vs-config gate applies to every command that writes except `upgrade` (`add`, `generate`, `version`, `release`, `init`, `migrate`). `check` and `status` stay off that gate so a version PR can report drift without needing the not-yet-published binary. That mismatch is a per-invocation refusal, not part of the shared readiness path [ADR-0020](0020-one-precondition-path.md) uses for `check` and `release` — a green `check` can still precede a `release` that refuses because the binary disagrees with `tool-version`. `version` reads `tool-version` before it writes the bump, so the older binary still matches while creating the PR; install pins are edited in that same self-host commit (not a `version` write — [ADR-0003](0003-write-only-what-a-command-owns.md) forbids writing CI). Oakum's own CI runs the workspace binary against that commit, not a crates.io pin of the previous release.

Until `version` exists, the current "every command except `upgrade`" gate stays. Oakum still has no `tool-version` of its own; narrowing it now would only weaken consumer CI. The missing `.changeset/`, `_config.toml`, and release workflow are leftover scaffolding, not an exemption.

## Amendment (2026-08-25)

`version` has landed. The write gate is `add`, `generate`, `version`, `ci version-pr`, `init`, and `migrate`; `upgrade` remains the repair. `release` is named in the 2026-08-22 list but is not a command yet, so it is not in the gate. `check` and `status` stay off the gate. Hidden read commands (`reachable-tags`, `detect-release-tools`, `plan-intent`, `tag-drift`) are also off it; `detect-release-tools` must run in a repository with no config yet.

## More Information

- [tool-version-pinning.md](../research/tool-version-pinning.md)
- The `#:schema` directive is taplo's convention, not part of TOML, so editor support varies. Worth stating in the README rather than assuming.
