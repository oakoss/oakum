# Plan from one intent artifact at a time

- Status: accepted
- Date: 2026-08-21
- Deciders: Jace Babin
- Amends: [ADR-0019](0019-both-change-files-and-commits-each-disableable.md), [ADR-0023](0023-name-every-verb-and-what-it-owns.md)

## Context and Problem Statement

[ADR-0019](0019-both-change-files-and-commits-each-disableable.md) accepts both change files and conventional commits, each fully disableable. It left open how they compose when both are enabled: whether a commit that maps to a package already covered by a change file is ignored, merged, or reported as a conflict. How does oakum feed the plan?

## Decision Drivers

- Pending bump files batch across many changes into one release; the plan already aggregates many `.changeset/*.md` files ([specs/bump-files.md](../specs/bump-files.md))
- [ADR-0019](0019-both-change-files-and-commits-each-disableable.md) and [ADR-0023](0023-name-every-verb-and-what-it-owns.md) already give `generate` the role of writing change files from commits, not feeding the plan directly
- bumpy is the primary interface peer and keeps the plan files-only, with commits entering only through `generate` ([intent-mechanism composition](../research/intent-mechanism-composition.md))
- The coverage gate (`okm-22h`) needs a single, obvious definition of "covered"

## Considered Options

- **Single artifact (Policy A)** — when change files are enabled, the plan reads only `.changeset/*.md`; commits feed `generate` only. When change files are disabled and commits are enabled, the plan reads commits
- **Parallel merge (Policy B)** — both feed the plan; bump level is highest-wins (knope)
- **Conflict (Policy C)** — disagreeing levels for the same package fail `check` / `status`

## Decision Outcome

Chosen option: **single artifact (Policy A)**, because the repository's release batching is already "many pending files, one cut," ADR-0019/`generate` already converge intent onto change files, and that is what bumpy does — so plan-time overlap between commits and files does not arise when files are on.

| Config | What the plan reads |
|---|---|
| Change files enabled (commits on or off) | `.changeset/*.md` only |
| Change files disabled, commits enabled | Commit-derived intent mapped directly into the plan (same bump/package rules `generate` would use, without writing a file) |
| Both disabled | Invalid — `check` must say so (ADR-0019) |

`generate` requires **both** change files and commit-derived intent enabled: it is the commit→bump-file bridge, writing `.changeset/*.md` a human can edit, and those files are what the plan reads. It does not feed the plan by any other path. If either mechanism is disabled, `generate` is unavailable (or refuses) — otherwise disabling commits would leave a commit parser online, and running it with change files off would write files the commits-only plan ignores. (`add` may still write bump files when change files are the enabled mechanism; when change files are disabled those files are not plan input until that mechanism is turned back on.) Multiple pending files still accumulate until `version` consumes them; one file is not one release.

**This amends ADR-0019** (composition) **and ADR-0023** (`generate` availability). Symmetry of disable switches is unchanged.

**Amended 2026-10-26 (`okm-8pub`): the commits-only plan reads each package's commits since its own last release.** It had read `merge-base(origin/main, HEAD)..HEAD`, which is empty on the default branch, so a commits-only repository planned nothing there unless every run passed `--from`, and one `--from` cannot hold a different base per package. This is the Confirmation trigger below: a linesmith shadow run on `main` planned nothing where knope planned two releases (measured with oakum 0.7.0).

- A package's range starts after the commit its highest reachable tag is on ([ADR-0014](0014-tags-are-the-version-source-of-truth.md), read as [ADR-0030](0030-derive-read-tag-shapes.md)). A package never tagged reads all of `HEAD`'s history.
- A package whose manifest is above that tag and whose changelog has the section, `version`'s untagged release, reads no commits until `release` tags it. Only the tag records what the release included: every reading of history that stood in for it (the merge-base, the changelog's last writer) put released commits back in range under a stale remote ref, a release cut on a branch, a later changelog edit, an uncommitted `version`, or a merge-only bump. `check`'s coverage, which plans nothing, reads the branch's commits for every package (`okm-eng0`, below).
- A package oakum does not tag keeps the merge-base range, having no tag to read from.
- A tag look that cannot answer (a shallow clone, a `--no-tags` remote, unattributable tags, one version tagged on two commits) is unverified, not an empty range.
- `--from` still reads one range for every package. `generate` keeps the merge-base range, since it writes this branch's bump file, and `check`'s coverage still diffs from the merge-base.
- `release` tags the commit that set the pending version, not `HEAD` (`okm-mqd4`): the oldest commit in the newest run of commits along `HEAD`'s first parents whose manifest (or, for `version.workspace = true`, the workspace's `[workspace.package]`) declares it. A commit that lands before a delayed `release` then stays after the tag and is planned next. A version a merge brought in is tagged at the merge, so `main`'s commits between the branch point and the merge sit inside a tag they were never planned for, as they did when `release` tagged `HEAD`. A manifest with no version, or none at all, before the run counts as another version, and the commit that created or restored it set the version; a moved package or a squashed import is therefore tagged where its manifest appeared, the history-walk limit ADR-0014 names. `release` refuses as unverified, never falling back to `HEAD`, when the walk reaches a manifest that names no readable version before it finds where the run began, when the newest commit to change the manifest declares another version, or when the version commit lacks a changelog section `HEAD` has for the version: a version set by hand before adoption, whose section arrived in a commit that changed no manifest, so no walk reaches the commit to tag. The remedy is a tag made by hand, which the next `release` resumes.

Coverage (`okm-22h`) follows the same table: when change files are enabled, coverage is against bump files (including empty / `none` where used); commits never satisfy the gate. When only commits are enabled, coverage is defined against commit-derived intent for changed packages.

**Amended 2026-10-29 (`okm-eng0`): coverage counts only the branch's own intent.** It had counted every pending bump file, and every commit since a package's tag, so between releases a pull request touching a package `main` already had pending intent for passed `check --strict` with none of its own (measured on Renovate PR #263: exit 0 with `main`'s six oakum bump files on disk, exit 1 without them). Now a bump file covers only when the branch touched its path: changed between the merge-base and `HEAD`, or changed, staged or untracked in the working tree. The base's copy is never parsed, so a symlinked file or one the branch is repairing cannot decide. Under commits only, coverage reads the branch's commits after the merge-base for every package. `status` and `ci pr-status` report coverage the same way. One `oakum add` on the branch answers it; a base with no history in common with `HEAD` leaves the look unverified.

### Consequences

- Good, because authors reason about one pending pool of bump files, including files produced by `generate`
- Good, because a forgotten `feat:` cannot silently raise a carefully authored `patch` file (knope's surprise)
- Good, because the coverage gate has one primary definition when files are on
- Bad, because repositories that want knope-style auto-merge without running `generate` must change workflow or disable change files
- Neutral, because whether `generate` skips packages already listed in pending files is product preference, not required by this decision

### Confirmation

Revisit if a commits-only repository cannot get a correct plan without a throwaway change-file write, or if teams that keep both mechanisms enabled routinely need knope-style dual plan input and refuse the `generate` step.

## More Information

- [intent-mechanism composition](../research/intent-mechanism-composition.md) — peer survey (bumpy serialize vs knope merge; no conflict peer)
- [ADR-0019](0019-both-change-files-and-commits-each-disableable.md) — both mechanisms, each disableable
- [ADR-0023](0023-name-every-verb-and-what-it-owns.md) — `generate` owns derived `.changeset/*.md` when both mechanisms are enabled
- [specs/bump-files.md](../specs/bump-files.md) — pending files accumulate; `version` consumes them
- Follow-up: `okm-j1r` (`generate`), `okm-22h` (coverage gate against the table above)
