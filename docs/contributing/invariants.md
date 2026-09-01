# Invariants

Rules that override agent and tool defaults. The oakum CLI must obey these; agents should too when touching the product.

**Every command writes only the files it owns.** No git hooks, no git config, no edits to `AGENTS.md` or `CLAUDE.md`, no CI workflow files, no commits that were not requested. `version` owns the manifests it bumps, the lockfile entries those bumps invalidate, declared `extra-files` for bumped packages, and — when the Cargo member named `oakum` is bumped — `tool-version` in `.changeset/_config.toml`; nothing else touches those. Anything else is printed, never performed. `check` is pure — it reports drift and names the fix.

**Discovery must be read-only.** `cargo metadata` without `--no-deps` writes a `Cargo.lock` into a lock-free crate, and `pnpm exec` performs an install. Neither belongs on a read path.

**Never collapse "we didn't look" into "it's fine."** Verifications report three outcomes. A tag whose downstream workflow could not be confirmed is `unverified`, not `ok`.

**Config expresses preference; facts are derived.** Before adding a config key, establish that it describes a preference rather than something readable from the repository. A key that restates the dependency graph will rot.
