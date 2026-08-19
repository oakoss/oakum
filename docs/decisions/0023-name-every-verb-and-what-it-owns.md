# Name every verb and the files it owns

- Status: accepted
- Date: 2026-08-19
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0003](0003-write-only-what-a-command-owns.md) says a command writes only what it owns, and forbids modifying manifests or lockfiles "without an explicit, separate command". It never names those commands. [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) enumerates three verbs — `status` reports, `check` decides, `release` acts — and the docs reference six more. `version` is the sharpest case: the repository-root `README.md` and `AGENTS.md` both grant it the manifest and lockfile write authority, and no ADR establishes that it exists. What is the full verb list, and what does each one own?

## Decision Drivers

- ADR-0003's rule is only enforceable against a list; "the files it owns" is not a test until ownership is written down
- The lockfile carve-out is real and its reasoning is non-obvious, so leaving it in the repository-root `README.md` alone puts a decision in a file that ADR-0003 treats as output
- A verb that appears in one spec and no decision is a verb nobody agreed to
- [ADR-0020](0020-one-precondition-path.md) depends on `check` and `release` sharing one path, which requires knowing where each stops

## Considered Options

- Leave ownership distributed across the specs that happen to mention a command
- Name the verbs here and let each spec carry its own detail
- Write a spec per command and reference no decision

## Decision Outcome

Chosen option: **name every verb and its writes here; specs carry the detail.**

| Verb | Writes | Stops at |
|---|---|---|
| `init` | `.changeset/_config.toml`, `.changeset/_schema.json`, `.changeset/README.md` | prints the workflow rather than writing it ([specs/init.md](../specs/init.md)) |
| `migrate` | the same three, plus the existing `.changeset/*.md` it transforms | reports the old tool's removal rather than performing it ([specs/migrate.md](../specs/migrate.md)) |
| `add` | one `.changeset/*.md` per invocation | writes what a human authored, never the plan ([specs/bump-files.md](../specs/bump-files.md)) |
| `generate` | `.changeset/*.md` derived from commits | writes a file a human can edit, never the plan directly ([ADR-0019](0019-both-change-files-and-commits-each-disableable.md)) |
| `version` | the manifests it bumps, the lockfile entries those bumps invalidate, changelogs, and the version pull request | does not tag and does not publish |
| `check` | nothing | reports drift and names the fix ([ADR-0003](0003-write-only-what-a-command-owns.md)) |
| `status` | nothing | emits data and renders text, never delivers ([ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md)) |
| `release` | the tag, and the GitHub release against it | the artifacts the tag triggers, which are cargo-dist's ([ADR-0011](0011-stop-at-the-tag.md), [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)) |
| `upgrade` | `tool-version` in `.changeset/_config.toml`, and `.changeset/_schema.json` | writes nothing if migration fails ([ADR-0007](0007-pin-the-tool-version-in-config.md)) |

`plan` is not a verb. It is the pure module [ADR-0002](0002-single-crate-until-io.md) is written around, and its output reaches users through `status --json` and `check --explain`. [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) considered and rejected shipping "plan only" as the product.

**Invoking a command requests its writes.** That is what ADR-0003's "without an explicit, separate command" means, and it is why `version` may create a commit while ADR-0003 forbids "any commit the user did not request" — running `version` is the request. The rule bites on side effects, not on a command doing the job its name states.

**The lockfile is `version`'s, and only for the entries its own bumps invalidate.** A Cargo version bump makes `Cargo.lock` stale for the bumped package, and a stale lockfile breaks the next `--locked` build, so leaving it alone hands a defect to the next CI run. Nothing else in the tool touches a lockfile, and `version` touches no entry it did not invalidate. Regenerating the lockfile wholesale would pull in unrelated dependency updates under a command the user invoked to change a version number.

**`check` and `release` share one precondition path** ([ADR-0020](0020-one-precondition-path.md)), so the table's "stops at" column is the only difference between them.

### Consequences

- Good, because ADR-0003 becomes testable: every write has a named owner, and a write with no owner in this table is a bug
- Good, because the `--locked` reasoning now has a decision behind it. `README.md` keeps stating it as a contract, which [ADR-0003](0003-write-only-what-a-command-owns.md)'s confirmation requires, but the contract no longer stands on a file and nothing else
- Bad, because a new verb now costs an amendment here rather than a spec alone; that is the point, but it is friction
- Neutral, because [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md)'s "three verbs, three jobs" stays true as a statement about `status`, `check`, and `release` specifically — it was never the whole list, and this table makes that explicit

### Confirmation

Every command in the shipped CLI appears in this table, and every file the tool writes traces to exactly one row. A command that grows a write not listed here is the drift ADR-0003 exists to prevent.

## More Information

- [ADR-0003](0003-write-only-what-a-command-owns.md) — the rule this table makes enforceable
- [ADR-0007](0007-pin-the-tool-version-in-config.md) — the version gate every verb but `upgrade` runs first
- [ADR-0011](0011-stop-at-the-tag.md) — why `release` stops where it does
- [ADR-0020](0020-one-precondition-path.md) — why `check` and `release` cannot disagree about "ready"

**Open:** whether `version` and `release` stay separate verbs or `release` subsumes `version` behind a flag. They are separate here because they write different things at different times: `version` opens a pull request a human reviews, and `release` acts on what merged. Collapsing them would put a manifest write and a tag push under one invocation. Nothing in v0 depends on the answer.
