# What covers a change

`oakum check` looks at every package a branch changed and asks whether something on the branch says what that change does to a release. A change with an answer is covered. A change with no answer is uncovered: `check` reports it, and `check --strict`, which the workflow from `init` and `migrate` runs, fails on it. The rules are [ADR-0039](../decisions/0039-derive-what-a-change-reaches-and-what-covers-it.md).

## What counts as changed

A package changed when the branch touched a file under its directory: the diff from the merge-base with `origin/main` (else `main`, else `master`), or from `--from <ref>`. The first line of `check` names the range it diffed.

A package oakum does not version, such as a `publish = false` crate, is not asked for coverage itself. When a package oakum versions depends on it through `dependencies` or `build-dependencies` (npm: `dependencies`, `peerDependencies`, `optionalDependencies`), directly or through other packages oakum does not version, the change is that package's change, reported with the path it came through: `app (cargo) (via store)`. A change no versioned package ships is not asked for coverage: `oakum status` lists it under "Changed but not version-managed", unless `include`/`exclude` leaves the package out, which keeps it silent.

A version commit's own writes are not a change, whatever its subject: in a commit that deletes a bump file, the manifest, changelog, lockfile and catalog edits in a package directory whose `CHANGELOG.md` the same commit wrote are left out. Its other edits, its `extra-files` writes, and anything a merge or a later commit touched still count.

## What covers it

Only the branch's own intent covers a change: a bump file the branch adds or edits, not one already pending on the base branch, and the branch's own commits. Which of these count depends on the intent the repository configures (`change-files` and `conventional-commits` in `.changeset/_config.toml`, both on by default):

| `covered_by` kind | `check` says | Counts when |
|---|---|---|
| `bump-file` | ``covered by `.changeset/beta-g.md` (minor)`` | change files are on |
| `none-entry` | ``covered by `none` in `.changeset/x.md` `` | change files are on |
| `empty-file` | ``the empty bump file `.changeset/x.md` covers every changed package`` | change files are on |
| `commit` | ``covered by commit `ed63ee8` (fix, patch)`` | conventional commits only |
| `releaseless-commit` | `covered by commits that release nothing (refactor)` | conventional commits are on |

- **A bump file** covers each package it names, at any level. A `none` entry (`oakum add --none --packages alpha:none`) covers the package and releases nothing; an empty bump file (`oakum add --empty`) covers every changed package. See [Writing bump files](bump-files.md).
- **A commit** reaches the packages its scope names, otherwise the packages its files are under, through the same reach as above. With change files off, the commits are the intent: a `feat`, `fix` or breaking commit covers the packages it reaches, at the level it asks for. With change files on, such a commit asks for a bump file rather than covering the package itself.
- **Commits that release nothing**, with both modes on, cover a package when every commit on the branch that reaches it is conventional, not breaking, and neither `feat` nor `fix`: `refactor:`, `test:`, `chore:`, `docs:` and the like. One `feat` or `fix` commit, or one message that is not conventional (which oakum reads as a patch), and the package needs a bump file again. A releasing commit counts against every package whose files it touches, even when its scope names another. Merge commits are skipped, and a git revert of a commit on the same branch cancels both. With conventional commits only, each commit is the intent, so a commit that releases nothing covers what it reaches beside any that release.

A package can be covered more than one way; `check` names each.

### `perf:` commits and squash merges

A `perf:` commit releases nothing, like `refactor:`: conventional commits release only `feat`, `fix` and breaking changes. A speed-up users would notice needs a bump file added by hand at the level it warrants, usually `patch` (`oakum add --packages app:patch`).

`check` reads the branch's commits, not the title a squash merge gives the commit on the default branch. With both modes on, a branch of `refactor:` commits passes on its own; squash-merged under a `fix:` pull request title, it leaves a `fix:` commit no bump file covers. Nothing on the default branch flags it: `oakum status` there diffs from `HEAD` itself and plans nothing, and only `oakum status --from <last tag>` shows the package uncovered. Squash under a title whose type matches the branch's commits, or add the bump file the title asks for before merging.

## Reading `check`

`check` prints one line per way the changed packages are covered, so one bump file naming thirty packages is one line. On a branch that changes three crates in a workspace with both modes on, where `app` ships the unpublished `store`:

```text
coverage: 3 changed package(s), 2 covered, 1 uncovered
coverage: covered by commits that release nothing (refactor): app (cargo) (via store)
coverage: covered by `.changeset/beta-g.md` (minor): beta (cargo)
```

The branch's commits were `refactor(store): …`, `fix(alpha): …` and `feat(beta): …`, with a bump file for `beta`. `alpha` is the uncovered one, and `check --strict` fails, naming the commit that asks for a release and the commands that cover it:

```text
error: 1 package(s) changed with no covering intent
  alpha (cargo): changed with no covering intent; commit `ed63ee8` "fix(alpha): reject an empty key" (fix) asks for a release; add a bump file with `oakum add --packages alpha:patch` (or `oakum add --none --packages alpha:none` if it should release nothing)
```

Each commit is named by its short hash, subject and type: `fix` or `feat`, any type followed by `, breaking` for a breaking change, or `not conventional`. With both modes on and no bump file of the branch's own, the line also offers `oakum generate --from <base>`, which writes one from the branch's conventional commits. With conventional commits off, commits are not read, so the line names no commit and leaves the level to you: `oakum add --packages alpha:<level>`. `check` without `--strict` prints the same lines and exits 0.

`oakum status` lists a covered change that releases nothing under "Changed, releasing nothing", and an uncovered one under "Uncovered" with the command that covers it. The pull-request comment uses the same wording.

### A refactor through an internal crate

oakoss/tm's #101 refactored `tm-store` and `tm-core`, crates that are `publish = false` and ship inside `tm-cli`, the one crate tm publishes, with one `refactor(store): …` commit. tm runs with both modes on. oakum 0.8 listed the two crates as changed but not version-managed and asked nothing of `tm-cli`. From 0.9.0 the change is `tm-cli`'s, and its refactor commit covers it, so the pull request needs no bump file:

```text
coverage: 1 changed package(s), 1 covered, 0 uncovered
coverage: covered by commits that release nothing (refactor): tm-cli (cargo) (via tm-core)
```

With change files only, the same branch needs `oakum add --none --packages tm-cli:none`, or an empty bump file.

## As data

`check --json` carries the same answer under the coverage look (`looks[]` where `look` is `coverage`) as `changed`; `status --json` and templates carry it as the top-level `changed`. Each entry is one changed package:

```json
{
  "name": "alpha",
  "ecosystem": "cargo",
  "via": [],
  "covered_by": [],
  "released_by": [
    { "commit": "ed63ee8a36df996c7cbaef2ace8cad996d1d563d", "subject": "fix(alpha): reject an empty key", "type": "fix", "bump": "patch" }
  ],
  "fix": "oakum add --packages alpha:patch"
}
```

- `via`: the packages oakum does not version that the change came through, as `{ "name", "ecosystem" }`.
- `covered_by`: each way it is covered, by `kind` (the table above) with that kind's `file`, `bump`, `commit` and `type`. Empty when nothing covers it.
- `released_by`: when nothing covers it, the commits asking for a release, each with its `subject`, `type` (`null` for a message that is not conventional) and `bump`.
- `fix`: when nothing covers it, the `oakum add` command that does, with `<level>` to fill in when no commit asks for one; otherwise `null`, as it is with conventional commits only, where no bump file covers a package.

## A shallow clone

Coverage reads the branch's history from the merge-base, so a clone without it cannot answer. In a `--depth 1` clone, `check --strict` exits 2 rather than reading the change as covered:

```text
check: looking at management, tags, install pin, schema, changelogs, staging, notes, and coverage; coverage and notes cannot run without a base ref; not looking at the remote (`--remote` asks for it)
unverified: shallow clone; fetch full history before reading tags
```

In GitHub Actions, check out with `fetch-depth: 0`; [Running oakum in GitHub Actions](github-actions.md) says why the tags need it too.
