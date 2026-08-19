# Write only the files the invoked command owns

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Setup and release commands routinely reach beyond their stated job. How much is oakum allowed to write?

## Decision Drivers

- A tool that edits files you did not ask it to edit cannot be trusted to run unattended
- Recovering from an unwanted write costs more than performing the write saved
- Prior experience: `bd init` set `core.hooksPath` (silently breaking lefthook), rewrote `AGENTS.md` and `CLAUDE.md`, and auto-committed with a message the commit linter rejected

## Considered Options

- Conventional setup behavior — write config, add a dependency, wire up CI
- Write only what the invoked command owns; print everything else

## Decision Outcome

Chosen option: **write only what the command owns**. Everything else is printed, never performed.

Forbidden without an explicit, separate command: installing git hooks, touching git config, editing documentation the tool did not generate, modifying manifests or lockfiles, writing CI workflow files, acting on the remote, and creating any commit the user did not request.

A corollary that follows directly: **`check` is pure.** It reports drift and names the fix; it never applies it. That in turn disqualifies any feature that makes rendering side-effecting.

Surveying comparable tools showed how far the norm sits from this. Only `knope init` and `changeset init` write nothing but their own config. bumpy's init adds itself to `devDependencies` and, on its migration path, renames `.changeset/` and shells out to remove another package. `release-please bootstrap` writes nothing locally and instead opens a pull request against your repository. `semantic-release-cli` rewrites `package.json` wholesale, sets the version to `0.0.0-development`, writes CI config, and creates a repository secret. `nx init` writes `CLAUDE.md` and a `.claude/settings.json` that registers a third-party plugin marketplace — **and it does this only when it detects an agent environment**, so an agent running it on your behalf gets a materially more invasive result than a human would.

### Consequences

- Good, because every effect of every command is predictable from its name
- Good, because it makes `check` safe to run anywhere, including on untrusted branches
- Bad, because setup takes more steps than tools that wire everything up; `init` prints what to add rather than adding it
- Neutral, because it removes CI generation as an option, so the tool-version pin must be verified rather than owned — see [ADR-0007](0007-pin-the-tool-version-in-config.md)

### Confirmation

This belongs in the README as a stated contract, not just in this file. A violation is a bug of the same severity as a wrong version number.
