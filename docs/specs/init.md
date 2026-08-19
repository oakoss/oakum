# init

- Status: draft
- Version: 0.1
- Last updated: 2026-08-19
- Driving ADRs: ADR-0003, ADR-0005, ADR-0007, ADR-0019, ADR-0022, ADR-0023

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

These three files are exactly what [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md) assigns to `init`.

**Flags:**

| Flag | Effect |
|---|---|
| `--versioning <semver\|zero-major>` | Sets the version policy ([ADR-0022](../decisions/0022-zero-major-versioning.md)). Defaults to `zero-major`, and is written to config explicitly either way |
| `--interactive` | Runs a guided wizard instead of the default silent path. Exits non-zero immediately when stdin is not a terminal, naming the equivalent flags, so it cannot block a script or a pipe |

**Every setting the wizard can produce is reachable as a flag.** The wizard is sugar over the flag surface, never a second configuration path — otherwise an agent or a CI run cannot reproduce what a human produced, and the two paths drift.

**Never writes:** manifests, lockfiles, CI workflow files, git config, git hooks, the repository-root `AGENTS.md` and `CLAUDE.md`, any file on the remote, or any commit.

**Prints:**

- The workflow YAML to add, with `tool-version` already substituted — never a fixed snippet, so what is pasted matches what `check` will later verify
- What it created, by path
- The uninstall instruction, so removal does not require reading documentation
- Any migration hazards it detected
- The `--interactive` flag, so the wizard is discoverable without reading documentation

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

### Non-interactive by default; the wizard is opt-in

Prompts are an enhancement over a working non-interactive path. `changeset init` has no `--yes`, ignores `CI=1`, and hangs on piped stdin, which makes it unusable from a script or an agent. Oakum's must complete with no terminal attached.

**The wizard is reached by `--interactive`, never by detecting a terminal.** Auto-detection is the conventional design and it fails for the caller this rule exists to protect: agents frequently run through a PTY, so a TTY check would prompt exactly the one that cannot answer. Requiring the flag means terminal detection is never load-bearing, and the default path cannot block regardless of what it runs under. `init` runs once per repository, so costing a human one flag is close to free — and the silent run names the flag among the things it prints.

The one terminal check that remains is inside `--interactive` itself: asked to prompt with no terminal attached, it exits non-zero and names the flags that would have produced the same config. That keeps the requirement above true — nothing blocks on a prompt when input is not a terminal — without putting detection on the default path.

## Edge cases

- **Already initialized, with a flag that disagrees** — an explicitly passed `--versioning` whose value differs from the existing config is reported and exits non-zero, naming the config edit that would change it. Accepting a flag and discarding it is the silent-drop failure `migrate.md` records for changesets' stale `prettier` key.
- **Already initialized** — the ADR-0007 version gate runs first. If `_config.toml` pins a `tool-version` this binary does not match, `init` refuses in either direction and names `oakum upgrade`; the ADR exempts only `upgrade`, so there is no "already initialized" shortcut past it. Matching, it reports that and exits zero, changing nothing.
- **Another release tool detected** — writes nothing and names `oakum migrate`. See [migrate](migrate.md).
- **An agent instruction file already in `.changeset/`** — `AGENTS.md`, `CLAUDE.md`, or `GEMINI.md`, matched exactly — is reported, and the run continues. Neither reader's hazard is reachable here: `init` routes to `migrate` the moment it detects knope or changesets, so no other tool reads this directory when `init` acts. What the file signals is that something is treating `.changeset/` as a notes directory, and a later lowercase variant *would* be fatal — which is worth saying once, not worth refusing over. `migrate` warns about the same files where the hazard is real. See [bump-files](bump-files.md).
- **No packages found** — reports it and exits zero. An empty repository is not an error.
- **A stray ancestor workspace file** — refuses to proceed. Discovery would silently describe a different repository; see [workspace-discovery.md](../research/workspace-discovery.md).

## Open questions

- Whether `init` should offer to write the workflow to a path the user names. It is still the user performing the write, but it edges toward owning a file oakum does not.
- What the *non-interactive* default is for which intent mechanisms are enabled. [ADR-0019](../decisions/0019-both-change-files-and-commits-each-disableable.md) settles that both change files and conventional commits are supported and either is disableable; `--interactive` gives the question a venue to be asked in. Neither settles what a flagless `init` should write, and enabling neither leaves nothing to plan from.

## Change log

- 2026-08-18: initial draft (v0.1)
- 2026-08-18: ADR-0019 settles that both mechanisms exist and either is disableable, which makes the init-time question live (v0.1)
- 2026-08-19: `--versioning` and `--interactive` added; the wizard is opt-in and every answer it produces has a flag equivalent (v0.1)
- 2026-08-19: ADR-0023 added to the driving list, since it now names the three files this command owns (v0.1)
