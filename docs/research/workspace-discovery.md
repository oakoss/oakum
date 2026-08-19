# Workspace discovery: asking the package manager

- Date: 2026-08-18
- Author: Jace Babin
- Scope: Whether package discovery can be zero-config across workspace and single-package repositories, and what can silently return the wrong answer.

## Question

Oakum discovers packages by asking the package manager rather than parsing manifests. Does that work without configuration in every repository shape we care about, and what makes it silently wrong?

## Sources

Constructed scratch repositories exercised against `cargo 1.94.1`, `pnpm 11.22.0`, `npm 11.17.0`. All commands run and output recorded.

## Findings

### Why ask the package manager at all

`cargo metadata --format-version 1 --no-deps` resolves `version.workspace = true` inheritance and lists implicit members — path dependencies inside the workspace become members without appearing in the `members` array, so parsing `Cargo.toml` alone is never complete. It also reports each dependency's `req` and `path`, which is what identifies an intra-workspace edge.

`pnpm list -r --depth -1 --json` returns a flat `{name, version, path, private}` per package.

### Single-package repositories work with no configuration

| Shape | Command | Result |
|---|---|---|
| Lone Rust crate | `cargo metadata --no-deps` | exit 0; `workspace_root` is the crate directory; `workspace_members` has one entry |
| Lone pnpm package | `pnpm list -r --depth -1 --json` | exit 0; one entry, same four fields as the workspace case |
| Lone npm package | `npm query ":root"` | exit 0; name, version, private, path |

Cargo models a lone crate as a one-member workspace. **There is no "is this a workspace" field, and inventing one is a bug**: both `workspace_members.len() > 1` and `workspace_root != package dir` would wrongly reject every single-package repository.

For npm, `npm query ":root"` is the only install-free command that returns all four fields. `npm ls` exits 1 when a declared dependency is not installed and reports no path or private flag. `npm query ".workspace"` returns an empty array — exit 0, no error — in a workspace that has never been installed.

### The stray-ancestor hazard is silent and severe

A single-package pnpm repository nested under *any* directory containing a `pnpm-workspace.yaml` reports the ancestor's packages and **omits itself entirely**. Exit 0, nothing on stderr. A release run there would version and publish unrelated packages and skip the intended one.

The child having its own `.git` does not stop the upward search. Dropping `-r` does not help. A `pnpm-workspace.yaml` with no `packages:` key still establishes a root.

This was reproduced accidentally before it was tested deliberately: stray files left in `/private/tmp` by unrelated processes caused a package five directories below to vanish from the listing, replaced by a single entry carrying neither a name nor a version.

**Detection is one side-effect-free probe.** `pnpm root -w` discriminates three ways:

| Result | Meaning |
|---|---|
| errors, `--workspace-root may only be used inside a workspace` | genuinely single-package |
| a path inside the git repository | legitimate workspace |
| a path outside the git repository | stray ancestor — abort |

The test is **containment, not equality**. A workspace rooted in a subdirectory — `repo/js/pnpm-workspace.yaml` in a polyglot repository — is legitimate, and `pnpm root -w` returns `repo/js/node_modules` for it. Requiring the parent to equal the git root would reject that, and would reject nested Cargo workspaces for the same reason. Only a root that is an *ancestor* of the git root is the hazard, because the repository cannot own it.

Cargo fails loudly in the equivalent case: exit 101, with a message naming both manifests and both fixes (add to `members`, or add an empty `[workspace]` table). Relay it verbatim. Its one silent case is a child that genuinely *is* a declared member, caught by the same containment check. npm has no upward hazard at all — it stops at the nearest ancestor `package.json`.

### Two commands mutate the repository

- `cargo metadata` **without** `--no-deps` writes a `Cargo.lock` into a crate that had none.
- `pnpm exec` performs an install, creating `node_modules` and a lockfile.

`pnpm root -w`, `pnpm list`, and `cargo metadata --no-deps` are all clean.

### Adapter unit is the package manager, not the ecosystem

npm has no `workspace:` protocol at all — `EUNSUPPORTEDPROTOCOL`. It uses plain ranges plus symlinking. The protocol is pnpm, yarn, and bun only. pnpm does not read `workspaces` from `package.json`, so the same repository looks like a workspace to one manager and not to another.

Cargo's `publish` field inverts under a falsy check: in `cargo metadata`, `null` means publishable anywhere and `[]` means publishable nowhere.

### Resolved targets are the only reliable delivery-artifact signal

Whether a package ships a binary decides which cascade rule applies, and looking for `src/main.rs` or `src/bin/*.rs` gets it wrong in both directions. `cargo metadata --no-deps` reports the targets Cargo actually resolved, which already accounts for `autobins` and explicit `[[bin]]` entries.

Verified 2026-08-18 on `cargo 1.94.1`. A package with `autobins = false`, one declared `[[bin]] name = "declared"`, and an undeclared `src/bin/ghost.rs`:

```text
  ab           kinds=['lib']
  declared     kinds=['bin']
```

`ghost.rs` is absent despite sitting at the conventional path, and `declared` is present because the manifest names it. Path-scanning would have found the opposite set.

## Conclusions

Zero-config discovery works in every shape tested. The risk is not failure — it is a confident wrong answer from pnpm under a stray ancestor.

## Implications / actions

- Comparing the resolved workspace root against the git root is a **precondition** that runs before any plan is computed, not a diagnostic emitted afterward.
- Discovery is a read-only path. `--no-deps` always; `pnpm exec` never.
- Do not derive an "is a workspace" boolean from member count or root path.
- `--ignore-workspace` fixes the nested case but is undocumented in `pnpm list --help`, and at a genuine workspace root it emits several concatenated JSON arrays that break any single-document parser. Never pass it unconditionally.

## Open questions

- Yarn and Bun untested; out of scope until a repository needs them.
- Whether `--ignore-workspace` on `pnpm list` is a supported contract or incidental behavior.
