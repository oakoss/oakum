# AGENTS.md

Oakum is a release tool: version math, changelogs, tags, and GitHub releases across npm and Cargo workspaces. The differentiator is graph-derived dependent bumping — see `README.md` for why libraries and delivery artifacts need opposite rules.

`mise run check` and `mise run test` are the local gates; CI mirrors them.

## Rules that override defaults

Always-loaded copy of [docs/contributing/invariants.md](docs/contributing/invariants.md) — `@` / agent includes do not expand markdown links.

**Every command writes only the files it owns.** No git hooks, no git config, no edits to `AGENTS.md` or `CLAUDE.md`, no CI workflow files, no commits that were not requested. `version` owns the manifests it bumps and the lockfile entries those bumps invalidate; nothing else touches either. Anything else is printed, never performed. `check` is pure — it reports drift and names the fix.

**Discovery must be read-only.** `cargo metadata` without `--no-deps` writes a `Cargo.lock` into a lock-free crate, and `pnpm exec` performs an install. Neither belongs on a read path.

**Never collapse "we didn't look" into "it's fine."** Verifications report three outcomes. A tag whose downstream workflow could not be confirmed is `unverified`, not `ok`.

**Config expresses preference; facts are derived.** Before adding a config key, establish that it describes a preference rather than something readable from the repository. A key that restates the dependency graph will rot.

## Session defaults

- Commit messages: `type(scope): summary`.
- One branch per session: `<type>/<short-description>`.
- Don't push or open PRs unless asked.
- Prefer `bd bootstrap` to `bd init`. Do not run `bd setup codex`. Details: [docs/contributing/task-tracking.md](docs/contributing/task-tracking.md).

## More detail

- Contributor map: [docs/contributing/index.md](docs/contributing/index.md)
- Crate layout and split triggers: [docs/contributing/structure.md](docs/contributing/structure.md)
- Documentation discipline: [docs/contributing/documentation.md](docs/contributing/documentation.md)
- Docs layout (decisions, specs, research, ideas, guide): [docs/README.md](docs/README.md)
- Decision records (check before proposing something already decided against): [docs/decisions/](docs/decisions/)
- Pull requests: [docs/contributing/pull-requests.md](docs/contributing/pull-requests.md)
- Commits and branches: [docs/contributing/conventions.md](docs/contributing/conventions.md)
