# Declare version writes outside a manifest

- Status: accepted
- Date: 2026-08-31
- Deciders: Jace Babin
- Amends: [ADR-0023](0023-name-every-verb-and-what-it-owns.md)
- Promotes: [ideas/0001-declarative-extra-files.md](../ideas/0001-declarative-extra-files.md)

## Context and Problem Statement

Some version strings live outside any package manifest: a Claude Code `plugin.json` beside `package.json`, and a shared `marketplace.json` that lists every plugin. Today those copies are kept in sync by a repository script after the bump. How should oakum write them without a per-repo script?

## Decision Drivers

- [ADR-0004](0004-derive-facts-configure-preference.md): config declares preference (*where* a version string lives), not a restatement of the dependency graph
- A declaration can be validated before write; a script can only be run
- Prior art: release-please `extra-files`
- The process-boundary hook in [ADR-0013](0013-no-plugin-runtime.md) stays the escape hatch for irregular cases; it is not this decision

## Considered Options

- Keep scripting the copies (command hook only)
- Declarative `extra-files` on each package (this decision)
- Derive every non-manifest version path from repository convention

## Decision Outcome

Chosen option: **declarative `extra-files`**. Where a version string lives is preference the repository cannot state as a fact; declaring it deletes the sync script for the surveyed cases.

Under `[packages.<name>]`, each entry is:

```toml
[[packages.review-cycle.extra-files]]
path = ".claude-plugin/plugin.json"
format = "json"
key = "version"

[[packages.review-cycle.extra-files]]
path = "/.claude-plugin/marketplace.json"
format = "json"
key = "plugins.{name=review-cycle}.version"
```

**Path.** A leading `/` is repository-root relative. Otherwise the path is relative to the package's manifest directory.

**Format.** v1 ships `json` (CST-preserving). TOML dotted keys can wait; they are not required to settle this decision.

**Key.** Dotted segments. A bare name is an object key. A numeric segment indexes an array. `{field=value}` selects the unique array element whose `field` string equals `value`. Zero matches and multiple matches are errors. This is not full jsonpath.

**Missing targets error.** A missing file or unresolved key fails the `version` run. Skipping would hide drift.

**Shared files.** Two packages may declare writes to the same path in one release. Staging is path-keyed with read-through ([WriteSet](../../crates/oakum/src/cli/write_set.rs)); package order is the workspace package-id order so the result does not depend on plan insertion order.

**Ownership.** This amends [ADR-0023](0023-name-every-verb-and-what-it-owns.md): `version` owns the files named by `extra-files` for packages it bumps, in the same write-set as manifests and lockfile entries.

**Hook remains the escape hatch.** ADR-0013's process-boundary program covers cases this surface cannot express. That hook is out of scope here.

### Consequences

- Good, because claude-plugins can drop `scripts/sync-plugin-versions.mjs` once configured
- Good, because the write is validated before anything lands
- Bad, because a new config key must be kept honest when files move — that is the cost of preference
- Neutral, because the package's own `package.json` / `Cargo.toml` version stays the normal manifest write; `extra-files` is for the copies beside it

### Confirmation

Confirmed when a fixture matching oakoss/claude-plugins PR #26 (`0.14.0` → `0.15.0` into `plugin.json`, `package.json`, and the named `marketplace.json` entry) is produced by `oakum version` alone, without a sync script (`okm-157`).

## More Information

- [ADR-0004](0004-derive-facts-configure-preference.md) — preference vs fact
- [ADR-0013](0013-no-plugin-runtime.md) — process-boundary escape hatch
- [ADR-0023](0023-name-every-verb-and-what-it-owns.md) — verb ownership table
- release-please `extra-files`
