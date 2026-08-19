# init

- Status: draft
- Version: 0.1
- Last updated: 2026-08-18
- Driving ADRs: ADR-0003, ADR-0005, ADR-0007

## Overview

`oakum init` prepares a repository. It is the one command most likely to overreach, so its contract is written down here rather than left to implementation.

Surveying comparable tools found that only `knope init` and `changeset init` write nothing but their own config. The others add dependencies, run package-manager commands, write CI files, open pull requests against your remote, or edit agent instruction files. ADR-0003 rules all of that out.

## Requirements

### Functional

- Create oakum's own configuration and nothing else
- Leave the repository able to explain itself: someone who opens `.changeset/` afterwards can write a correct bump file without leaving the directory
- Print, rather than perform, every step it does not own
- Be runnable non-interactively

### Non-functional

- Idempotent: running it twice changes nothing the second time
- Never blocks on a prompt when input is not a terminal

## Interface / Contract

**Writes:**

| Path | When |
|---|---|
| `.changeset/_config.toml` | always, if absent — carries the binary's exact `tool-version`, which ADR-0007 makes mandatory and which the printed workflow must match |
| `.changeset/_schema.json` | always — generated, tracks the installed binary |
| `.changeset/README.md` | always, if absent |

**Never writes:** manifests, lockfiles, CI workflow files, git config, git hooks, `AGENTS.md`, `CLAUDE.md`, any file on the remote, or any commit.

**Prints:**

- The workflow YAML to add, with `tool-version` already substituted — never a fixed snippet, so what is pasted matches what `check` will later verify
- What it created, by path
- The uninstall instruction, so removal does not require reading documentation
- Any migration hazards it detected

## Behavior

### `init` is for a repository with no release tool

When it detects another release tool, `init` writes nothing, reports what it found, and names `oakum migrate`. Adoption is a different job with different risks, and running it under the name `init` is how a command grows conditional behavior.

Detected by the presence of any of:

| Tool | Marker |
|---|---|
| knope | `knope.toml` |
| changesets | `.changeset/config.json` |
| bumpy | `.bumpy/_config.json` |
| release-please | `release-please-config.json` or `.release-please-manifest.json` |
| release-plz | `release-plz.toml`, or `[workspace.metadata.release_plz]` in `Cargo.toml` |
| semantic-release | `.releaserc*`, `release.config.js`, or a `release` key in `package.json` |
| nx release | a `release` key in `nx.json` |

A `.changeset/` directory holding bump files but no config is also treated as migration, since something wrote them.

This is what removes the README conditional. Every `.md` file directly inside `.changeset/` is a bump file to knope, which has no name-based skip list and aborts its whole run on the first parse failure — so a `README.md` there breaks it. Because `init` only ever runs where no other tool reads that directory, it can write one unconditionally, and the case where it would have caused damage is handled by `migrate` instead.

### Non-interactive

Prompts are an enhancement over a working non-interactive path. `changeset init` has no `--yes`, ignores `CI=1`, and hangs on piped stdin, which makes it unusable from a script or an agent. Oakum's must complete with no terminal attached.

## Edge cases

- **Already initialized** — the ADR-0007 version gate runs first. If `_config.toml` pins a `tool-version` this binary does not match, `init` refuses in either direction and names `oakum upgrade`; the ADR exempts only `upgrade`, so there is no "already initialized" shortcut past it. Matching, it reports that and exits zero, changing nothing.
- **Another release tool detected** — writes nothing and names `oakum migrate`. See [migrate](migrate.md).
- **No packages found** — reports it and exits zero. An empty repository is not an error.
- **A stray ancestor workspace file** — refuses to proceed. Discovery would silently describe a different repository; see [workspace-discovery.md](../research/workspace-discovery.md).

## Open questions

- Whether `init` should offer to write the workflow to a path the user names. It is still the user performing the write, but it edges toward owning a file oakum does not.
- Whether a repository with neither bump files nor conventional commits configured should be an `init`-time choice or deferred to first use.

## Change log

- 2026-08-18: initial draft (v0.1)
