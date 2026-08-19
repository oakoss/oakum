# oakum

A release tool that derives dependent version bumps from the dependency graph, across npm and Cargo workspaces.

Oakum is the tarred fiber driven into the seams between a ship's planks to keep the hull watertight. This tool exists because releases leak: in one repository, eight fixes were versioned, tagged, and reported as published while never reaching a single user, and every check passed.

**Status: pre-release. Nothing here is stable, and no release has been cut.**

## What it does that other tools don't

Most release tools rewrite a dependency's pin when it bumps. Few decide, from the graph, whether the _dependent_ needs a release at all — and the ones that do apply a rule that is correct for libraries and wrong for binaries.

A library whose published range still covers the new version does not need republishing; a consumer re-resolves at install time and gets the fix for free. A delivery artifact is different. Its dependencies are baked in at build time, nothing re-resolves, and the fix reaches users only if the artifact is rebuilt and re-released. Treat the two the same and a caret range silently swallows the release.

Oakum derives which is which — from the binary targets Cargo resolves for a package, from the `bin` field in npm — rather than asking you to declare it. Reading resolved targets rather than looking for `src/main.rs` is what makes `autobins = false` and explicit `[[bin]]` entries come out right.

## Design rules

**Config expresses preference; facts are derived.** What depends on what, what is publishable, what needs a bump, and which artifact ships are all read from the repository on every run. Templates, titles, the tag format oakum writes, and commit messages are yours, because they describe output rather than the repository, so they cannot go stale. Two keys are neither: `tool-version`, which pins the binary allowed to run, and `resolves-dependencies-at`, which states when your package manager fixes a dependency — a fact about the ecosystem that oakum cannot read from your repository.

**Every command writes only the files it owns.** Oakum does not install git hooks, touch git config, edit `AGENTS.md`, write CI workflows, or create commits you did not ask for. `version` owns the manifests it bumps and the lockfile entries those bumps invalidate — a Cargo version bump invalidates `Cargo.lock`, so leaving it stale would break the next `--locked` build. Nothing beyond that is written. `check` is pure: it reports drift and names the fix, never applies it.

**Failure is loud, and "we didn't look" is never reported as "it's fine."** Every verification has three outcomes, not two. A version that could not be confirmed says so.

## Non-goals

No plugin runtime. Support only for the package managers this repository's projects actually use. No prerelease channels, snapshot releases, or staged publishing until a real repository needs them. No template value may execute a shell command — templates render, and anything that needs generating runs in your workflow and arrives as a file.

## License

MIT
