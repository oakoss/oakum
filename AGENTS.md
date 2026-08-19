# AGENTS.md

## What this is

A release tool: version math, changelogs, tags, and GitHub releases across npm and Cargo workspaces. The differentiator is graph-derived dependent bumping — see `README.md` for why libraries and delivery artifacts need opposite rules.

## Rules that override defaults

**Every command writes only the files it owns.** No git hooks, no git config, no edits to `AGENTS.md` or `CLAUDE.md`, no CI workflow files, no commits that were not requested. `version` owns the manifests it bumps and the lockfile entries those bumps invalidate; nothing else touches either. Anything else is printed, never performed. `check` is pure — it reports drift and names the fix.

**Discovery must be read-only.** `cargo metadata` without `--no-deps` writes a `Cargo.lock` into a lock-free crate, and `pnpm exec` performs an install. Neither belongs on a read path.

**Never collapse "we didn't look" into "it's fine."** Verifications report three outcomes. A tag whose downstream workflow could not be confirmed is `unverified`, not `ok`.

**Config expresses preference; facts are derived.** Before adding a config key, establish that it describes a preference rather than something readable from the repository. A key that restates the dependency graph will rot.

## Documentation

`docs/README.md` explains the layout and which directory a given document belongs in. Before changing behavior, check `docs/decisions/` — a decision recorded there was expensive to reach, and several encode findings that took real verification to establish. Contradicting one means writing a new ADR that supersedes it, not quietly diverging.

Claims about external tools belong in `docs/research/` with their sources. An undated or unsourced finding is worse than none.

## Structure

One crate, with modules named for the crates they would become. Split only on a trigger you can observe:

- **The first I/O dependency lands in `[dependencies]`.** `plan` is pure today because nothing it can reach touches the filesystem, network, or a subprocess. Once something does, extract `plan` so the dependency list enforces its purity instead of code review. Dev-dependencies are not the trigger — library code cannot reach a test harness.
- **Something outside oakum parses its JSON output.** That output is then a public interface, and its types belong in a schema crate consumers can depend on without pulling in the binary.

Splitting for organization alone is not a trigger; modules already do that.

## Task tracking

Use `bd` (beads). Run `bd prime` for the command reference and session protocol.

Run `mise run setup` after cloning. A fresh clone has no beads database — `.dolt/` and `*.db` are gitignored — so until it runs, every `.beads/hooks/*` hook exits 3 and skips itself with one line on stderr. Setup runs `bd bootstrap`, which clones the task graph from the `sync.remote` in `.beads/config.yaml`; the graph lives on a Dolt remote, not in this repository.

Prefer `bd bootstrap` to `bd init` generally. Bootstrap leaves `core.hooksPath` unset and `AGENTS.md` untouched; `bd init` sets the former — making git bypass lefthook silently — rewrites the latter, and commits with a message `cog verify` rejects. `core.hooksPath` must stay unset: `lefthook.yml` calls `.beads/hooks/*` directly, and `mise run check` fails if the setting reappears.

Do not run `bd setup codex`. This repository is worked in Claude Code; that command writes a `.codex/` directory and an `.agents/` skill nobody here reads. Both `bd init` and `bd setup codex` also append a managed block to this file — strip it and keep the pointer above.

## Conventions

- Commit messages: `type(scope): summary`.
- One branch per session: `<type>/<short-description>`.
- Don't push or open PRs unless asked.
