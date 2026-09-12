# Migrating to oakum

`oakum migrate` transforms another tool's bump files and config into oakum's, then prints what it could not do for you. It is not a workflow translator: the release pipeline is the one part of a cutover that has to be rewritten rather than ported, and this guide says why.

Run it from the repository root:

```sh
oakum migrate          # prints the plan and asks
oakum migrate --yes    # applies it
```

## What `migrate` writes

It writes only files it owns — `.changeset/_config.toml`, `.changeset/README.md`, `.changeset/_schema.json`, and the bump files it transforms. Your old tool's config is left on disk, untouched.

Backing out is not always the same operation, so follow the uninstall line the run prints rather than a fixed list. It names the files oakum actually created: a repository that already had a `.changeset/README.md` keeps it, and the line names two files instead of three. Bump files need separate handling — migrating from `bumpy` **copies** them into `.changeset/` and leaves the originals, so the copies are yours to delete; migrating from `@changesets/cli` **rewrites them in place**, so backing out means restoring their previous contents from git.

## The acceptance test

**`oakum status` names every package you expect to release.**

Not "`oakum check --strict` exits 0". That was the instruction given to one real cutover, and it was satisfied immediately by a config that managed nothing at all — the gate went green precisely because the config was wrong.

oakum has since closed the specific hole that cutover fell into: a config whose only packages are private now refuses, naming the fix.

```text
every selected package is private, so no plan can name one and a release would do nothing;
set `private-packages.version = true` to version them, and `private-packages.tag = true` to tag them
unverified: this config manages no package on either axis
```

One shape still passes, deliberately. An `include`/`exclude` that selects nothing exits `0`, because emptying a selection is a decision the config states and a gate must not refuse one — `check`'s own header says `check: 0 of 1 package(s) selected` on that run, which is the tell.

So the argument is narrower than it was, and it still holds. A plan is a positive statement about what will happen; a gate exiting 0 is the absence of a complaint, and absence is what a no-op config produces. Reading the plan needs no guard to have been written in advance for the particular way your config is wrong. Ask for it:

```sh
oakum status
```

If a package you expect to release is missing from that list, the config is wrong no matter what `check` says. The usual causes are below under [When a package is missing](#when-a-package-is-missing).

## What carries, and what does not

`migrate` says this per run, naming the file each setting came from. From a real run against a `bumpy` repository:

```text
carried over: `versionCommitMessage` from `.bumpy/_config.json` as `commit-message`
carried over: `privatePackages.version` and `privatePackages.tag` from `.bumpy/_config.json`
not carried over: `access` (`.bumpy/_config.json` is untouched)
not carried over: `baseBranch` (`.bumpy/_config.json` is untouched)
not carried over: `changelog` (`.bumpy/_config.json` is untouched)
not carried over: `gitUser` (`.bumpy/_config.json` is untouched)
```

Three kinds of "not carried over" in that example, and they are different. Only the last two are classifications oakum makes; the first is a judgement about these particular keys:

- **Reported, with nothing said about a counterpart.** `access` and `baseBranch` are dropped generically — oakum names them and stays silent on whether anything replaces them. For these two nothing is lost. Read the generic drops in your own run rather than assuming the same; that line means "oakum did not carry this", not "this does not matter".
- **A near-equivalent that means something else.** `changelog` is named as a decision you owe: oakum's nearest setting is `template`, and writing one from the other would silently change what you get. oakum writes neither and says so.
- **No counterpart, but the absence changes an outcome.** `gitUser` decided who authored release commits and tags. oakum has no identity key, and the printed workflow's `git config` decides only the *tagger* — the version commit is written through the GitHub API and carries the token's own account, which no git config can change.

`versioning` is chosen for you and the reason is printed: a repository already below 1.0.0 under a tool that takes `0.1.3` to `1.0.0` gets `versioning = "semver"`, because renumbering an established release line is not a migration's job. oakum's own default for a new repository is `zero-major`.

## The remaining steps, in order

`migrate` prints these; the order matters more than the list.

1. **Pin oakum.** `migrate` writes `tool-version` into `_config.toml`, and two different checks then read it. `check` looks for an install pin in the repository — a workflow, `package.json`, `.mise.toml`, a Cargo member — and reports `unverified` when it finds none or finds a different version. The write commands (`add`, `generate`, `version`, `release`, `init`, `migrate`) refuse when the *running binary* is not the pinned version. `status` does neither, which is why it works as the acceptance test immediately after a cutover. Add the pin first — the printed workflow carries one, or `pnpm add -D @oakoss/oakum@<version>` — or your first `check` reports unverified with the migration already applied.
2. **Paste the workflow.** `migrate` prints it and does not write it (ADR-0003: every command writes only files it owns).
3. **Add a publish job.** `oakum release` tags and creates the GitHub release. It does not publish. `npm publish` / `cargo publish` need their own job on the tag push (`on: push: tags`).
4. **Widen branch filters.** The version PR opens on `oakum/version-packages`.
5. **Remove the old bump files oakum copied out.** They are left in place deliberately, so nothing is destroyed — but the old tool still counts them, and a workflow still wired to it releases the same packages a second time.
6. **Check for gates on the old directory.** A commit hook or CI step grepping `.bumpy/` will reject oakum's bump files. `migrate` searches git's index outside `.changeset/` and reports what it found — and says plainly what it did not search: untracked files, unstaged edits, submodules, and `.git/hooks/`.
7. **Remove the old tool's dependency and workflow.** Last, so every step above is reversible.

## The CI surface you cannot port

This is the part to read before starting.

A pipeline built on a **plan/act split** — one job computes a mode, downstream jobs run guarded by `--expect-mode` — has no equivalent here, and the guarantee it provides is not expressible in oakum's shape.

oakum has no plan step. `ci version-pr` and `release` each derive their own precondition from repository state at the moment they run, and each decides internally whether to act. Both jobs run on every push to the default branch; the one with nothing to do does nothing.

That is a different safety argument, not a weaker one — there is no window between deciding and acting, because there is no separate decision. But it means a mode-guarded workflow cannot be translated line by line. Rewrite it against the two-job shape in [the workflow guide](github-actions.md), and read [Concurrency](github-actions.md#concurrency-and-why-the-printed-workflow-has-none) there, which explains why the printed workflow has no concurrency group and when you would want one.

## When a package is missing

`oakum status` lists nothing, or misses a package you expect:

- **The package is private.** oakum does not version unpublishable packages unless you opt in (ADR-0027). `migrate` carries `privatePackages` when the source config set it; if yours did not, set `private-packages.version` / `.tag`.
- **`include` / `exclude` emptied the selection.** `status` says so explicitly rather than printing an empty plan.
- **No bump files were transformed.** Check they were where the old tool kept them, and that `migrate` reported writing them.

`oakum check` opens every run that reaches a look by naming how many of your packages the selection keeps and which ref it diffed from, which is usually enough to tell these apart. A run that refuses before reading the config — no `_config.toml`, no repository — prints nothing to stdout and says why on stderr.

## What you can now retire

`migrate` names these too, because a cutover's strongest argument is often something the old tool could not do:

- **`extra-files`** (ADR-0033) — declare a **JSON** file that carries a version, and `oakum version` writes it in the same pass as the manifest, under the same rollback. If you maintain a sync script for a plugin manifest or a marketplace entry, that script, its drift job and its tests can go. v1 writes JSON only: a YAML chart or a README badge is not expressible, and a config naming one is refused at parse.
- **A changelog that lints** (ADR-0031) — oakum writes generated markdown that also reads as prose, so a repository linting it needs no per-release fixup step. This is not free: apply the two lint settings `init` prints first (MD024 `siblings_only`, MD041 off for `.changeset/*.md`), or a repository linting `**/*.md` on defaults starts failing at its second release.

## Getting out

`migrate` prints an uninstall line on every run that applies naming the files it created — which is two or three depending on whether it wrote a `README.md`. Follow that line rather than a remembered list, and see [What `migrate` writes](#what-migrate-writes) above for bump files, which are copied from `bumpy` and rewritten in place from `@changesets/cli`. Your old tool's config was never touched.
