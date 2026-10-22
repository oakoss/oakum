# oakum

A release tool that derives dependent version bumps from the dependency graph, across npm and Cargo workspaces.

Oakum is the tarred fiber driven into the seams between a ship's planks to keep the hull watertight. This tool exists because releases leak: in one repository, eight fixes were versioned, tagged, and reported as published while never reaching a single user, and every check passed.

[![crates.io](https://img.shields.io/crates/v/oakum)](https://crates.io/crates/oakum) [![npm](https://img.shields.io/npm/v/@oakoss/oakum)](https://www.npmjs.com/package/@oakoss/oakum)

**Status:** Published on crates.io, [Homebrew](https://github.com/oakoss/homebrew-tap), and npm (`@oakoss/oakum`); see [releases](https://github.com/oakoss/oakum/releases). Zero-major versioning below 1.0.0 ([ADR-0022](docs/decisions/0022-zero-major-versioning.md)) — APIs and defaults can still change; pin an exact version where you install oakum (a `package.json` devDependency, a workflow install line, or `.mise.toml`), which is the version oakum enforces ([ADR-0038](docs/decisions/0038-the-install-pin-is-the-version.md)).

## What it does that other tools don't

Most release tools rewrite a dependency's pin when it bumps. Few decide, from the graph, whether the _dependent_ needs a release at all — and the ones that do apply a rule that is correct for libraries and wrong for binaries.

A library whose published range still covers the new version does not need republishing; a consumer re-resolves at install time and gets the fix for free. A delivery artifact is different. Its dependencies are baked in at build time, nothing re-resolves, and the fix reaches users only if the artifact is rebuilt and re-released. Treat the two the same and a caret range silently swallows the release.

Oakum derives which is which — from the binary targets Cargo resolves for a package, from the `bin` field in npm — rather than asking you to declare it. Reading resolved targets rather than looking for `src/main.rs` is what makes `autobins = false` and explicit `[[bin]]` entries come out right.

## Design rules

**Config expresses preference; facts are derived.** What depends on what, what is publishable, what needs a bump, and which artifact ships are all read from the repository on every run. Templates, titles, the tag format oakum writes, and commit messages are yours, because they describe output rather than the repository, so they cannot go stale. One key is neither: `resolves-dependencies-at`, which states when your package manager fixes a dependency — a fact about the ecosystem that oakum cannot read from your repository. `init` and `upgrade` will write `_schema.json` next to `_config.toml` and a `#:schema ./_schema.json` line; that directive is taplo's, not part of TOML, so editor support varies.

**Every command writes only the files it owns.** Oakum does not install git hooks, touch git config, edit `AGENTS.md`, write CI workflows, or create commits you did not ask for. `version` owns the manifests it bumps, the lockfile entries those bumps invalidate, and declared `extra-files`. A Cargo version bump invalidates `Cargo.lock`, and a rewritten workspace range invalidates its `pnpm-lock.yaml` entry, so leaving either stale would break the next `--locked` build or frozen install. Nothing beyond that is written. `check` is pure: it reports drift and names the fix, never applies it.

**Failure is loud, and "we didn't look" is never reported as "it's fine."** Every verification has three outcomes, not two. A version that could not be confirmed says so.

## Install

Three install channels, one cargo-dist build behind all of them, so the versions cannot diverge ([ADR-0021](docs/decisions/0021-distribute-through-three-channels.md)):

- `cargo install oakum` — builds from crates.io
- `brew install oakoss/tap/oakum`
- `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@<version>` — the exact version is the install pin oakum enforces. The npm package is a fetcher, not a bundle: a small install script downloads the platform binary from the GitHub release, and pnpm 10 and later run that script only for approved packages. `--allow-build` writes that approval (`allowBuilds`). Without it pnpm 11 and later fail `pnpm add` and `pnpm install` with `ERR_PNPM_IGNORED_BUILDS`; pnpm 10 only warns, and the first `pnpm exec oakum` fetches the binary instead

None of these works offline, and the npm channel needs **two** origins at install time — the registry for the package, then `github.com` for the binary. An environment with an internal npm mirror but no route to GitHub installs the package and then fails in `postinstall`; there, `cargo install oakum` through a crates.io mirror is the only channel that avoids GitHub entirely, since the shell installer and Homebrew fetch from the GitHub release too.

## Non-goals

No plugin runtime. Support only for the package managers this repository's projects actually use. No prerelease channels, snapshot releases, or staged publishing until a real repository needs them. No template value may execute a shell command — templates render, and anything that needs generating runs in your workflow and arrives as a file.

## License

MIT
