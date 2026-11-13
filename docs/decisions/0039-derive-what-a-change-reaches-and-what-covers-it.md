# Derive what a change reaches, and what covers it

- Status: accepted
- Date: 2026-10-31
- Deciders: Jace Babin
- Amends: [ADR-0028](0028-releaseless-bump-files-like-bumpy.md)

## Context and Problem Statement

A tm pull request (oakoss/tm#101) refactored two internal crates with no behavior change. It needed a hand-written CI step and an `--empty` bump file, for two reasons:

- `check` never asks about an internal crate a released package ships. Coverage drops a change to an unmanaged package into a separate report (`changed_by_standing` in `crates/oakum/src/cli/coverage.rs`), so `tm-store` changing says nothing about `tm-cli`, which ships it.
- With change files and conventional commits both on, a branch whose commits all say `refactor:` or `test:` still needs a file to say it releases nothing. The commits already say so.

The first gap is not only a coverage gap. A `fix(store): …` commit whose paths touch only `tm-store` reaches no package: `generate` refuses with nothing to write, and in a commits-only repository the fix is never released. A gate that asks `tm-cli` for coverage would ask for a file `generate` cannot write.

What does a change reach, and what covers a package it reaches?

## Decision Drivers

- Which packages ship an internal crate is a fact in the dependency graph, not a preference ([ADR-0004](0004-derive-facts-configure-preference.md)). No config keys.
- One answer to "what does this change reach" for every reader: coverage, `generate`, the commits-only plan. Two answers mean the gate asks for something the writer cannot produce.
- A look that could not run is `unverified` ([ADR-0034](0034-exit-two-for-unverified.md)), never covered.
- A refusal names the commit that caused it and the command that fixes it.

## Considered Options

- **A config list of the internal crates each package ships**: restates the graph, and will rot.
- **Reach for coverage only**: closes the gate gap and leaves `generate` and the commits-only plan unable to attribute an internal fix.
- **One derived reach rule for every reader of changes**, with releaseless commits as coverage.

For releaseless commits, two scopes were weighed: the whole branch (no bump file of its own and every commit releases nothing) or each package (every commit reaching it releases nothing). Branch scope leaves `b` uncovered on a branch with `fix(a)` and `refactor(b)` even after `a` gets its file, and differs from how the commits-only plan already reads a commit that releases nothing.

## Decision Outcome

Chosen option: **one derived reach rule for every reader of changes, and per-package releaseless coverage**, because it is the only option where the gate never asks for intent that `generate` cannot write, and it adds no configuration.

### What a change reaches

A changed path reaches the package whose directory is its longest prefix, as today. When that package is not managed (unmanaged, or excluded by `include`/`exclude`), the change also reaches every managed package that depends on it along a runtime edge: the edges a release cascades along under [ADR-0008](0008-cascade-only-along-runtime-edges.md) (npm `dependencies`, `peerDependencies`, `optionalDependencies`; Cargo `dependencies` and `build-dependencies`). The walk continues through other non-managed packages and stops at the first managed package on each path; past it, the cascade rules of ADR-0008 to [ADR-0010](0010-derive-cascade-from-declared-ranges.md) decide.

Excluded packages are followed like unmanaged ones. ADR-0027 makes exclusion a statement about a package's own version, changelog and tag, not about what ships it.

A reach through other packages records its path, written `tm-cli (via tm-store)`; a longer chain names each package in order. When several paths reach the same package, the shortest is named, ties broken by name.

The rule serves every reader of changes:

- coverage in `check`, `status` and `ci pr-status`;
- `generate`'s path fallback, and a scope that names a non-managed package;
- the commits-only plan;
- the report of releasing commits that reach no package.

A `fix(store): …` commit therefore releases `tm-cli` at `patch`, and `generate` writes that line.

A changed non-managed package that a managed package ships is reported as that package's change. Only one that no managed package ships stays in the unmanaged report.

### Workspace-root files

A Cargo workspace's root `Cargo.toml` and `Cargo.lock` sit in no member's directory, yet they decide what each member builds: a `rusqlite` bump with `bundled` changes the SQLite compiled into tm's binary. Both files are compared as data on each side of a change, never as text, and what moved reaches members as below; those members then go through the reach rule above like any changed package, so an internal crate carries the change to the package that ships it.

- **`[workspace.dependencies]`**: a changed entry (version, features, source, anything in it) reaches each member that inherits it with `workspace = true` under `[dependencies]`, `[build-dependencies]` or a target's runtime table. A member that writes the dependency out, or inherits it only under `[dev-dependencies]`, is not reached. A path entry whose only change is its `version` reaches nothing: `version` writes it, and the crate it names reaches through the workspace graph.
- **`[workspace.package]`**: a changed field reaches the members that inherit it, except `version`, which `version` itself writes for an inheriting member; a hand-set version is the hand-set report's.
- **`[workspace.metadata]`** reaches nothing; no build reads it.
- **Anything else** (`[profile]`, `resolver`, `[patch]`, `[replace]`, `members`, `[workspace.lints]`, …) reaches every managed Cargo package. Mapping each setting to the members it affects would be guesswork, and a missed one ships unreviewed. A root that is also a package keeps its own tables (`[package]`, `[dependencies]`, `[target]`, `[features]`, `[lints]`, `[[bin]]`, …) out of this: they reach that package through its directory, like any member's manifest.
- **`Cargo.lock`**: a registry or git row added, removed, or changed in any field is followed up the lock's dependency edges to the members whose rows list the last crate passed through and that declare it, by name, as a `dependencies` or `build-dependencies` entry. The name suffices because `cargo metadata --no-deps` reports a source from before `[patch]`; when a member declares one name more than once, the declarations whose source (a git source's `#<commit>` aside) and version requirement both match the row decide; with none, those whose version requirement matches, since `[patch]` rewrites a source but must meet the requirement; with none of those, all of them stand. Rows on the earlier side are matched against the declarations as they were there: each member's `Cargo.toml` (`[project]` read as `[package]`) and the root's `[workspace.dependencies]`, read from git at that side and never by running cargo, with sources spelled as `cargo metadata --no-deps` reports them, URLs parsed as cargo parses them. `registry = "<name>"` takes its index from `CARGO_REGISTRIES_<NAME>_INDEX`, else from the nearest `.cargo/config` or `.cargo/config.toml` naming it, from the workspace root up the repository at that side, else from `$CARGO_HOME`'s; a name none of them gives leaves that declaration's source unknown, which neither matches nor rules out a row, so its requirement decides. A member declares nothing there when its manifest is absent there, when it was no member of the workspace there (a `[workspace]` of its own; under an `exclude` prefix that no `members` entry, read literally, overrides; or matched by neither a `members` glob nor, transitively, a member's path dependency under the root), or when cargo would refuse it there, as with inheritance the root there cannot answer. A member reached carries the walk on only to members whose rows list it with no runtime workspace graph edge to it: a registry or git declaration `[patch]`ed onto the member is no edge, since `cargo metadata --no-deps` reports no `path` for it, so the walk reaches that member, named by the declaration (`Cargo.lock: helper`). A member on a runtime graph edge is reached through the graph instead, yet the walk still passes on from it, since the graph stops at a `[patch]`ed declaration above it; the walk does not pass a member's row on to a path crate outside the workspace. A row's `dependencies` are compared as the rows they resolve to, since Cargo re-spells `name` as `name version` once a second version is locked. Rows without a source are the workspace's own, which `version` retargets, so a version commit asks nothing of them, even under a registry row a member is `[patch]`ed into; a path crate outside the workspace is one too, which the walk passes through to the member that declares it. A sourceless row that picks another of the versions locked moves no sourced row when each stays locked for some other crate, so its dependencies are compared too: a member reaches the picked crate it ships and carries it on as above, and a path crate outside the workspace starts a walk. Cargo locks a member and a path crate outside the workspace under one name when their versions differ; while a side holds more than one sourceless row of a name, rows pair across the sides by version (the one row left over on each side pairing with the other's, as `version` moves it), and the member is the row at the member's version on that side, its version on the earlier side read with its declarations, paired with the member's row on the other side before the rest pair, or a row paired with it; when neither side holds the member's version (`generate` reading an older commit), no row of the name is the member, so a walk passes through rather than stopping short. A lock added on the branch counts every row as changed.

A pnpm workspace's `pnpm-workspace.yaml` sits in no member's directory either, and its catalogs decide the range a member publishes: pnpm replaces `catalog:` with the entry's value when it packs the member.

- **Catalogs**: an entry added, removed, or given another value, in `catalog` or any table under `catalogs` (`catalog` and `catalogs.default` both being the default one), reaches each npm member whose `dependencies`, `optionalDependencies` or `peerDependencies` take it, by the manifest key, with `catalog:` or `catalog:<name>`, as the member's `package.json` reads on the later side, an `optionalDependencies` entry overriding a `dependencies` one of the same key. An entry that installs the same workspace package on both sides, by its key or an `npm:` alias, with its earlier range retargeted at that package's version, is the pin `version` writes, so it reaches nothing; any other change to it, a hand edit within the package's version included, changes what its takers publish and reaches as any other. A file pnpm refuses (`catalog` and `catalogs.default` both present, or a `catalogs` table that is null) does not parse, and neither does a member's `package.json` that is not JSON.
- **`pnpm-lock.yaml`**: an entry added, removed, or changed in its `packages` entry (a registry `deprecated` notice, and an empty `bundledDependencies`, aside; a pnpm upgrade that rewrites `hasBin` or `libc` changes what installs, so it counts) or its `snapshots` edges, and a project's runtime dependency resolved to another version, is followed up the snapshots' `dependencies` and `optionalDependencies` to the projects whose `dependencies` or `optionalDependencies` lead to it. A linked workspace project (`link:`) or local directory (`file:`) is no entry, and specifiers and the lockfile's `catalogs` are not compared, so `version`'s lockfile writes reach nothing. A project reached counts only toward the packages that build it in: itself, or any package depending on it at runtime past managed ones too, that resolves its dependencies at build (a `bin`, or `resolves-dependencies-at = "build"`). A published library ships no lockfile, so its users resolve its ranges afresh and a lock change gives them nothing new. Other `pnpm-workspace.yaml` settings (`overrides`, `patchedDependencies`, `packageExtensions`, …) reach through the lockfile, which pnpm moves when they change what installs (measured for `overrides`); one that moves no entry changes nothing a package ships. A lockfile in a format oakum has not measured does not parse.

A change that reaches only through development dependencies asks nothing, for the reason [ADR-0008](0008-cascade-only-along-runtime-edges.md) gives for the cascade: nothing a release ships contains it.

The sides compared are those the reader diffs: coverage's merge base against `HEAD` (and against the working tree for the uncommitted half, reading files the tree changed from disk), and for `generate` and releaseless commits each commit against its first parent, a root commit against nothing. A side that does not parse is `unverified`, never "nothing changed", and so is a member's `Cargo.toml` on the earlier side of a lock change when it does not parse; one that parses but was no member there, or that cargo would refuse there, declares nothing there, as above. Nothing runs cargo to resolve either side. A version commit's writes to the root files are not dropped before the comparison, as they are in a package's directory: the rules above excuse the parts `version` writes, and a setting riding along in the same file still counts.

The reason travels beside the path: `tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: rusqlite)`, and as `through` in `--json`.

### What covers a package a change reaches

A managed package a change reaches is covered by any of:

1. a bump file on the branch that names it, at any level including `none`;
2. an empty bump file on the branch, which covers every changed package (ADR-0028);
3. with `conventional-commits` on, the branch's commits, when every commit that reaches the package releases nothing.

Otherwise it is uncovered.

A commit reaches the packages `generate` attributes it to: its scope when that names a package, otherwise its paths, both through the reach rule above. A commit releases nothing when it is conventional, not breaking, and neither `feat` nor `fix` ([generate spec](../specs/generate.md)). A message that is not conventional releases, at `patch`, as `generate` reads it.

| Intent configured | What covers a package |
|---|---|
| change files only | a bump file (unchanged) |
| change files and conventional commits | a bump file, or commits that all release nothing |
| conventional commits only | commits, read as the plan reads them (unchanged) |

A `fix` commit covered by a `none` file stays covered: the file is the author saying it releases nothing, which is what ADR-0028 made it for.

**This amends ADR-0028.** Under both intent sources, a branch whose commits release nothing needs no file to cover the packages they reach.

**Merge commits are skipped.** They carry no paths of their own, as in `generate`, so merging the base into a branch neither covers nor uncovers anything.

**A revert of a commit on the branch cancels both.** Git's `Revert "…"` commit names its target in its body (`This reverts commit <sha>.`). When the target is in the range being read, neither commit counts, for coverage or for the plan. A revert of a commit outside the range is read like any other non-conventional message, and releases, because undoing a released change changes what ships.

### When the look cannot run

A shallow clone is `unverified` (exit 2), naming `fetch-depth: 0`, as today: the same range supplies paths and messages. A dependency graph or commit messages that cannot be read leave coverage `unverified` too. Neither is ever read as covered.

### What each surface says

`check`'s text, the pull-request comment and `--json` ([ADR-0036](0036-report-the-whole-check-run-as-data.md)) report the same things for each changed managed package:

- the path it was reached by, empty for a direct change, and what in a workspace's root files it came through;
- how it is covered: the bump files that name it, an empty bump file, or the releaseless commits with each one's short hash and type;
- when uncovered, each commit that releases, with its short hash, subject, and why it releases (`feat`, `fix`, breaking, or not conventional).

Releaseless coverage is shown, not only counted. The comment reads "releases nothing: refactor, test" so a reviewer sees the claim the commits make. Coverage takes the commit type on trust, as conventional commits always have.

A refusal names the fix:

- `oakum generate` for a releasing commit when `generate` is allowed, otherwise `oakum add`;
- `oakum add --none <package>` when the change should release nothing;
- for a message that is not conventional, that oakum reads it as a `patch`.

It never suggests rewording or rebasing commits.

### Consequences

- Good, because tm's refactor is covered by its own commits, with no CI step and no file.
- Good, because a fix in an internal crate releases the package that ships it, in both intent modes, where it was silently dropped before.
- Good, because the gate and `generate` read changes the same way, so the gate never asks for a file that cannot be generated.
- Bad, because a pull request that changes only an internal crate passed `check --strict` and is now refused until something covers it. That is a refusal on an input that worked, breaking under [ADR-0022](0022-zero-major-versioning.md). A repository without conventional commits covers it with a `none` file.
- Bad, because coverage trusts the commit type: a behavior change typed `refactor:` passes the gate. The comment showing the claim is the defense.
- Bad, because an npm package that bundles an internal package listed under `devDependencies` is not reached. The release cascade has the same gap under ADR-0008, so the two rules stay one rule.
- Bad, because a dependency update touching only the root files, which passed `check --strict`, is refused until something covers it; a dependency-bot pull request now needs a bump file or a releaseless commit type. Breaking under ADR-0022, as above.
- Neutral, because oakum's own workspace is unaffected: `crates/oakum` reaches `crates/plan-no-std` only through `[dev-dependencies]`, which reach does not follow.

### Confirmation

tm removes its inline CI step once this ships. Revisit if repositories routinely cover behavior changes with releaseless types, or if bundled `devDependencies` reach a decision for the cascade.

## More Information

- Driving bead: `okm-e9p8.1`, epic `okm-e9p8`. Implemented by `okm-e9p8.3` (releaseless commits), `okm-e9p8.4` (the reach rule, including `generate` and the commits-only plan) and `okm-e9p8.6` (the report). The exact `--json` field names are settled with `okm-e9p8.6`.
- The [generate spec](../specs/generate.md) is updated with the implementation.
- Deferred: a pull-request-title check for squash-merged commits-only repositories. `check --explain` (`okm-bluv`) has since shipped.
- Open: a conventional `revert:` commit releases nothing under `generate`'s table, unlike git's default revert subject.

**Amended 2026-11-12 (`okm-ar54`):** a pnpm workspace's catalogs reach the members that take them, and its lockfile the packages that build it in, as the Cargo root files do. A catalog bump passed `check --strict` with "no version-managed package changed" (measured, pnpm 12.16.1), so this too is a refusal on an input that worked. The lockfile rule stops at libraries, unlike `Cargo.lock`'s, so a lockfile refresh on a library-only repository asks for nothing.
