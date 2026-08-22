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

`tool-version` is an exact version, never a range — a range reintroduces a resolution step, which is the drift being prevented. Every command except `upgrade` refuses to run when it disagrees with the binary, in **both** directions, naming the upgrade command. `upgrade` validates against the old schema, runs migrations, writes the new version, regenerates the schema, and reports what changed — writing nothing if migration fails, since a half-migrated config is worse than a stale one.

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

## Open questions

**Self-hosting collides with the gate.** When oakum releases oakum 0.2.0, the binary doing the work is 0.1.0. If that release also bumps `tool-version` to 0.2.0, every later command refuses to run until the workflow's install pin catches up — and the release that would produce the new binary is the thing being blocked.

Either the release updates the config version and the workflow pin in the same commit it tags, or oakum's own repository is exempt from the gate. An exemption is the worse answer: the one repository guaranteed to exercise this decision would be the one repository that never does.

**cargo-dist's own answer, read from its source 2026-08-18, is narrower scope plus a documented bypass.** Its check lives in `do_generate_preflight_checks`, so it gates `dist generate` rather than every command, and it exempts two cases outright: a magic `vX.Y.Z-github-BRANCHNAME` prerelease, commented "which we use for testing against a PR branch", and `--allow-dirty`. That is how the one tool with this mechanism self-hosts — it does not apply the gate everywhere, and it ships an escape hatch for its own development. Narrowing oakum's "every command except `upgrade`" to the commands that write, plus same-commit ordering (the version PR updates `tool-version` and the workflow pin together, so the older binary creating it still reads the older config), resolves this without an exemption.

**As of this writing the repository takes the exemption by omission** — there is no `.changeset/`, no `_config.toml`, no `tool-version` anywhere outside prose, and no release workflow; cocogitto generates the changelog instead. That is defensible while nothing releases, but it is a deferral, not a decision. It closes when oakum first releases itself, and whichever branch is taken then has to be recorded here.

## More Information

- [tool-version-pinning.md](../research/tool-version-pinning.md)
- The `#:schema` directive is taplo's convention, not part of TOML, so editor support varies. Worth stating in the README rather than assuming.
