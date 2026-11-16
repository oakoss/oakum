# Dependency updates need no intent, and release only on request

- Status: accepted
- Date: 2026-11-15
- Deciders: Jace Babin
- Amends: [ADR-0029](0029-plan-from-one-intent-artifact.md), [ADR-0039](0039-derive-what-a-change-reaches-and-what-covers-it.md)

## Context and Problem Statement

With change files and conventional commits both on, a Renovate or Dependabot pull request that moves a dependency a versioned package ships cannot cover itself. tm measured it on oakum 0.10.1 (TM-139): a root `Cargo.lock` update committed as `fix(deps)` fails `check --strict`, asking `tm-cli` for a bump file, and as `chore(deps)` it passes and releases nothing. Renovate cannot write a bump file itself: its post-upgrade commands are blocked by default on the hosted app, and run before it commits, when `generate` has no commit to read. The documented workaround, a CI job that runs `generate` and pushes the file, needs a token other than `GITHUB_TOKEN`, a guard against re-runs, and `gitIgnoredAuthors`. And an edit to a package's own development dependencies asks for intent though nothing it ships changes (`okm-cgrl`). What should a pull request that only moves dependencies need, and when should it release?

## Decision Drivers

- A bot cannot write release intent, and the workaround that writes it for the bot is the heaviest setup in the guide
- [ADR-0039](0039-derive-what-a-change-reaches-and-what-covers-it.md) already derives which packages a dependency change reaches at runtime, and that a change reaching only development dependencies ships nothing
- [ADR-0029](0029-plan-from-one-intent-artifact.md) keeps one intent artifact; a commit type deciding a release reopens the dual input it closed
- Whether a dependency update earns a release of its own is release policy, like `versioning` in [ADR-0022](0022-zero-major-versioning.md): two repositories with the same graph answer it differently, a library batching updates and tm wanting its binary out with each security fix ([ADR-0004](0004-derive-facts-configure-preference.md)). What an update reaches, and the level it implies, stay derived

## Considered Options

- **A deps-scoped `fix` commit is its own intent** (tm's proposal)
- **An oakum CI step that writes the bump file** for bot pull requests
- **A preference for which commits count as intent**
- **Dependency updates need no intent; a preference releases them**

## Decision Outcome

Chosen option: **dependency updates need no intent; a preference releases them**, because it keys on what the change does rather than on who wrote it or how its commit is worded, reuses the reach ADR-0039 already derives, and needs no setup for the common case.

### What a dependency update is

A path through which a change reaches a package, by ADR-0039's rules, is a dependency-update path when it is one of:

- a lockfile's dependency reach: a row of the root `Cargo.lock` or an entry of the workspace's `pnpm-lock.yaml`, except rows a declaration added or removed in the same range, which belong to that declaration's change;
- a changed existing entry's version requirement, features or source in `dependencies`, `optionalDependencies`, `peerDependencies`, `build-dependencies`, or a target's tables of these, whether the entry sits in the package's manifest, in the manifest of a package oakum does not manage that ADR-0039 reaches it through (`tm-store` for `tm-cli`), in `[workspace.dependencies]` or in a catalog.

An entry naming a workspace member is not one, wherever it is declared: the cascade and `version`'s rewrites own those edges ([ADR-0008](0008-cascade-only-along-runtime-edges.md), [ADR-0010](0010-derive-cascade-from-declared-ranges.md)), and a feature or source edit to one needs intent as any other change does.

A change is a dependency update for a package when every path through which it reaches the package is one, so a manifest edit that also touches another field, such as `description`, is not; a change also reaching it another way, a source file or any other manifest field, is covered as ADR-0039 says. Where ADR-0039 reads commits (releaseless coverage, the commits-only plan, `generate`), a commit whose every path to a package is a dependency-update path releases nothing for it, whatever its message says: a conventional commit of any type, a scope naming the package, and a message that is not conventional (Dependabot's `Bump serde from …`) are all read by their paths, so a `fix(deps)` commit beside a `refactor:` one leaves the branch covered by commits that release nothing. A side of a diff that cannot be read is `unverified`, as ADR-0039's root files are.

A change reaching a package only through development dependencies asks nothing. That was ADR-0039's rule for the root files and the graph; it now holds for an edit to the package's own `dev-dependencies` or `devDependencies` too, which asked for intent before.

### What covers it, and when it releases

A new preference, `dependency-updates` in `.changeset/_config.toml`:

| `dependency-updates` | Coverage | Plan |
|---|---|---|
| `"ignore"` (default) | covered | nothing: the update ships with the package's next release, unannounced |
| `"release"` | covered | a release for each managed package a dependency-update path reaches in its range, at the level below, with a synthesized note |

This holds in every intent mode: a `fix(deps)` commit whose only reach is a dependency update neither asks for a bump file nor, under conventional commits only, plans a release by its type, and `generate` writes nothing for it, since a file it wrote would release what `"ignore"` holds back.

Under `"release"`, each dependency-update path that reaches a package adds its level and note, whatever else reaches the package in the same range. Levels and notes are read entry by entry: a moved dependency entry adds its level even when the same edit also changed another field and so needs intent for coverage. The range is read as one diff, the one [ADR-0029](0029-plan-from-one-intent-artifact.md)'s `okm-8pub` amendment gives the commits-only plan. A package oakum versions but does not tag has no range that survives the merge, so `"release"` releases nothing for it; a bump file still does. A package releases, and at what level:

- **A package that resolves its dependencies at build** (a `bin`, or `resolves-dependencies-at = "build"`): `patch` for any update that reaches it, including a lockfile update that reaches a managed library it depends on at runtime (see the ADR-0039 amendment below). What it ships is a build, and nothing its users declare changes. A `bin` package that also ships a library (a Cargo `lib` target built as a Rust library, or an npm `main` or `exports`; see More Information) takes the library rule too when its declared requirements moved, and the higher level wins.
- **A library reached only through a lockfile** (`Cargo.lock` or `pnpm-lock.yaml`): no release. Its users resolve its ranges themselves and never read its lockfile, published or not, so they get nothing new, as ADR-0039 already reads `pnpm-lock.yaml`.
- **A library whose own declared requirements moved** (its manifest or an unmanaged package's it ships, an inherited `[workspace.dependencies]` entry, or a catalog entry): `patch` when every moved declaration is compatible, `major` when any is not.

A declaration counts when it is in force: an npm name in both `dependencies` and `optionalDependencies` is read from `optionalDependencies`, as discovery reads it ([ADR-0008](0008-cascade-only-along-runtime-edges.md)), so moving the shadowed entry adds nothing. Workspace members are not read. A move is compatible when:

- for `dependencies`, `optionalDependencies`, `build-dependencies` and their target tables, the new requirement's lowest version falls within the caret range of the old one's, or the new range accepts every version the old one did, so `^1.2` to `^1.5` and `^1.5` to `^1.2` are, and `^0.40` to `^0.41` and `^1` to `^2` are not;
- for `peerDependencies`, the new range accepts every version the old one did, so `^1` to `^1 || ^2` is, and `^1 || ^2` to `^1` or `^2` is not, since a consumer on a dropped version can no longer install it;
- for features, none is removed, and none of the dependency's default features is turned off; adding one is compatible.

A requirement with no lowest version on one side or both (a git revision, a URL, a dist-tag, a switch between registry and git) is not compatible. Build dependencies count, since what they generate can reach a library's compiled API ([ADR-0008](0008-cascade-only-along-runtime-edges.md)). Whether an incompatible move breaks a library's users depends on its public API, which no manifest shows; a major that was not needed costs users a deliberate upgrade, while a patch that was breaking breaks them, so the level errs upward. Under `versioning = "zero-major"`, the default, a major below 1.0.0 renders as the next minor ([ADR-0022](0022-zero-major-versioning.md)); under `"semver"` it is 1.0.0.

The derived level combines with pending bump files as bump files combine with each other ([bump-file spec](../specs/bump-files.md)): the highest release level wins, and a `none` file raises nothing. A bump file cannot lower a derived level; lowering is left to a later decision.

The note names each direct dependency, of the package or of an unmanaged package it ships, that the update reached through, once, from its version at the start of the range to its version at `HEAD`: the declared requirement when it moved (`host ^1 || ^2 → ^2`), otherwise the locked version (`rusqlite 0.40.0 → 0.41.0`). A name declared by more than one owner or target, with different versions, is qualified by it (`windows-sys (cfg(windows)) …`, `rusqlite (tm-store) …`). It sits under `### Changed`, as a cascade line does ([ADR-0032](0032-synthesize-cascade-changelog-line.md)), beside any notes from bump files. A direct dependency whose own version did not move, its reach coming through crates a lock-file maintenance moved below it, reads `rusqlite (its dependencies updated)`, and those crates are not listed by name; a reach through a managed library reads the same way under the library's name (`tm-core (its dependencies updated)`). A features change reads `serde: feature derive removed`, or `serde: default features off`.

`status`, `status --json`, `ci pr-status`, `version` and `ci version-pr` plan with the same rule, so a dependency pull request's comment names the release its merge will make, and a release made only of dependency updates opens a version pull request as one with bump files does. Such a version commit deletes no bump file, which is how coverage recognizes a version commit today; coverage recognizes it by its writes, without recomputing the plan: every write is one `version` owns, at least one package's manifest version moved, and each that moved gained that version's section in its changelog, so an unrelated edit riding along still asks for intent.

**This amends ADR-0029.** Under `dependency-updates = "release"` the plan also reads the dependency updates in each package's range, and in every intent mode a dependency update's commit plans nothing by its type.

**This amends ADR-0039.** A package a dependency update reaches is covered without a bump file or a releasing commit, and an edit to a package's own development dependencies asks nothing. A version commit is recognized by its writes, not by a deleted bump file. A `Cargo.lock` update that reaches a managed library also reaches the packages that depend on it at runtime and resolve their dependencies at build, as a `pnpm-lock.yaml` update already does, since otherwise nothing carries a library's lockfile-only update, which releases nothing, to the build that ships it.

### Consequences

- Good, because a bot's dependency pull request passes `check --strict` with no token, no CI job and no bump file
- Good, because an edit to a package's own development dependencies, whatever its commit type, asks nothing and releases nothing (`okm-cgrl`)
- Good, because a repository that wants updates released sets one key, and the release says what moved, at a level read from the move
- Bad, because under `"ignore"` a security update sits unreleased until something else triggers a release, and its changelog does not mention it
- Bad, because under `"ignore"` a library's own range bump, which changes what its consumers resolve, is not asked for a bump file; its maintainer can still add one
- Bad, because under `"release"` a library takes a major for an incompatible move, a removed feature or an incompatible build-dependency move its API does not expose, and no bump file lowers it
- Bad, because a repository on conventional commits only, whose `fix(deps)` commits release today, releases nothing for them under the `"ignore"` default until it sets `"release"`; a library such a commit reached only through `Cargo.lock`, which releases today, releases nothing under either value. That changes what an input plans, so the change ships as a breaking one ([ADR-0022](0022-zero-major-versioning.md))
- Bad, because a pnpm workspace with `shared-workspace-lockfile=false` keeps a lockfile per package, which is not the workspace's `pnpm-lock.yaml`; a lock-maintenance pull request there still asks for intent
- Neutral, because a `rust-version` change is not a dependency update and still needs intent, and a root `rust-toolchain.toml` reaches no package under ADR-0039 already, unless the repository's root is itself a package, which it then reaches through its directory

### Confirmation

Confirmed when tm drops its bot-PR CI step and a monthly Renovate batch passes `check --strict` and, under `"release"`, releases `tm-cli` with its notes. Revisit if `"ignore"` users ask for the notes without a release, for the preference per package, or for a way to lower a derived major.

## More Information

**Amended 2026-11-15 (`okm-nj20`): three rules the implementation plan settled.**

- An entry's dependency-update fields are its version requirement, `features`, `default-features` and source (registry, git URL, path, `rev`, `tag`, `branch`); a change to `optional`, a `package` rename, an npm alias's target or any other key needs intent.
- Adding or removing a `[workspace.dependencies]` or catalog entry is not a dependency update. A member's declaration is compared as it resolves through inheritance or its catalog, and an added or removed entry reaches a member only through that resolved declaration, so switching `"^1"` to `workspace = true` or `catalog:` that resolves to the same requirement changes nothing, even when the entry is added in the same range.
- A package ships a library when it has an npm `main` or `exports`, or a Cargo `lib` target whose crate types include `lib`, `rlib`, `dylib` or `proc-macro`; one built only as `cdylib` or `staticlib` does not. A `resolves-dependencies-at = "build"` package without a `bin` takes the build rule only.

Sources and follow-ups:

- `okm-nj20`: the requirement and options; `okm-cgrl`: the development-dependency case this closes
- [Running oakum in GitHub Actions](../guide/github-actions.md): the bot-PR recipe this makes unnecessary
- Follow-ups not decided here: the preference per package; lowering a derived level; a `status` field listing updates waiting under `"ignore"`
