# What peer tools do when a full clone has no tags

- Date: 2026-08-21
- Author: Jace Babin
- Scope: when a successful look finds zero tags, what is "current version," and is that distinct from a fresh repository?

## Question

[ADR-0014](../decisions/0014-tags-are-the-version-source-of-truth.md) reads current version from tags reachable from `HEAD`. A shallow clone with no tags is already **unverified** (did not look). `okm-coc` asks the leftover case: a **full** history with no tags.

Is "never shipped" distinguishable from "this is a fresh repository"? What do tools that already treat tags as truth do, and what do tools that treat the manifest as truth do?

## Sources

- `knope-versioning` `semver/package_versions.rs` `PackageVersions::from_tags` and `calculate_new_version`, `docs.rs` latest, fetched 2026-08-21
- `knope-dev/knope` `crates/knope-versioning/src/package.rs` `Package::new`, `main`, fetched 2026-08-21
- knope `CHANGELOG.md`, **0.21.3 (2025-08-16)**, "Fix pre-release versioning when there are no previous stable versions"
- `semantic-release` `lib/definitions/constants.js` (`FIRST_RELEASE`) and `lib/get-next-version.js`, `master`, fetched 2026-08-21
- semantic-release FAQ, "Can I set the initial release version of my package to `0.0.1`?", gitbook, fetched 2026-08-21
- changesets CLI docs: `version` updates from `package.json`; `publish` / `git-tag` write tags from the current `package.json` ([command-line-options.md](https://github.com/changesets/changesets/blob/main/docs/command-line-options.md))
- `release_plz_core` `next_ver.rs` `process_git_only_package`, `docs.rs` latest, fetched 2026-08-21
- release-plz config docs, `git_only`, fetched 2026-08-21
- cargo-release `docs/reference.md`: bump levels apply to the version in `Cargo.toml`; `--prev-tag-name` is for change detection, not current version
- rust-lang/cargo `src/cargo/ops/cargo_new.rs` (`version = "0.1.0"`) and the Cargo book, *Creating a New Package*, fetched 2026-08-21
- npm docs, *Creating a package.json file*: `version` is always `1.0.0`; config `init-version` default `"1.0.0"`, fetched 2026-08-21

## Findings

### Two families

Tools that **derive current version from tags** treat empty history as "no previous release" and pick a first next version from a constant (or from files, in knope's later overlay).

Tools that **derive current version from the manifest** never ask this question. Empty tags do not change the bump base.

### knope: tags first, then files overlay

`PackageVersions::from_tags` with no matching tags logs `"No tags found starting with {pattern}"` and returns `Self::default()` (`stable: None`).

`calculate_new_version` with a stable rule and `stable: None` uses the last prerelease's stable component **or `unwrap_or_default()`**. The unit tests `major_unset`, `minor_unset`, and `patch_unset` all assert that default is `0.0.0`.

`Package::new` then always overlays the version from versioned files:

```text
let mut versions = PackageVersions::from_tags(name.as_custom(), git_tags);
if let Some(version_from_files) = version_from_files {
    versions.update_version(version_from_files);
}
```

So knope as a product is **hybrid**. Empty tags alone would be 0.0.0; a `Cargo.toml` at `0.1.0` wins.

That overlay is deliberate. knope 0.21.3 (2025-08-16) **stopped** using tag-empty → 0.0.0 when files had a version, because 0.x rules made a first 1.0.0 prerelease unreachable. The changelog names the old behavior as a bug.

knope does **not** distinguish "fresh repo" from "never shipped." Both are: no matching tags, then files if present.

### semantic-release: tags only, first next is 1.0.0

`get-next-version.js`: if `lastRelease.version` is missing, the next version is `FIRST_RELEASE` (`"1.0.0"`), or `1.0.0-<prerelease>.1` on a prerelease branch. It does not read `package.json` as current.

The FAQ refuses a configurable first version of `0.0.1` / `0.1.0`: 0.x rules are out of scope. To start below 1.0.0 you **hand-cut a tag** first (issue #919: no tag → 1.0.0; after `git tag v0.1.0` → next is 0.1.1).

No third "green field" outcome. Empty tags = first release.

### changesets and cargo-release: manifests are current

changesets `version` bumps from `package.json`. Tags are written at `publish` / `git-tag` from the **already-written** version. A repo with no tags and `"version": "0.0.0"` bumps 0.0.0 → 0.1.0 (or whatever the changeset says). Tags are not consulted for "where are we."

cargo-release's bump levels apply to the version in `Cargo.toml`. Git tags are used to **see what changed** (`--prev-tag-name`); a missing tag does not invent a current version from tags.

These are the family ADR-0014 rejected.

### release-plz: registry by default; git-only treats no tag as initial

Default current version is the cargo registry. `git_only`: `process_git_only_package` returns `None` when no tag matches, with log `"Package {} will be treated as initial release."` Next version then comes from the local project (manifest), not from a tag-derived 0.0.0.

Again: no "fresh vs never shipped" split. No tag under git-only = initial release.

### Init tools start at 0.1.0 or 1.0.0, not 0.0.1

`cargo new` / `cargo init` write `version = "0.1.0"`. That value is hardcoded in `cargo_new.rs`; the Cargo book shows the same generated manifest. Omitting `version` defaults to `0.0.0` and cannot be published. `npm init` writes `1.0.0` (`init-version` default). Neither tool starts a package at `0.0.1`.

## Conclusions

**A full look with zero tags is one fact.** No surveyed tool that reads tags distinguishes a green-field repo from a long-lived repo that never tagged. The observable is the same.

**Tag-truth tools do not read the manifest as current.** semantic-release never does. knope's file overlay is an explicit later patch on top of `from_tags`, documented as fixing 0.x first-release, not as "this is a different kind of empty." Using the manifest as current *because* tags are empty would be the changesets/cargo-release family, which ADR-0014 already declined.

**First next version is a separate number from current.** semantic-release: current none, next 1.0.0. knope `from_tags` alone: current none, and a stable rule then yields 0.0.0 (`major_unset`, `minor_unset`, `patch_unset`), then files overlay. Oakum picked `0.1.0` with a clobber guard; see Implications.

## Implications / actions

**Decided 2026-08-21** in [ADR-0014](../decisions/0014-tags-are-the-version-source-of-truth.md) (*Empty tag history*):

- Full look, zero tags → never released (`current = none`). No third outcome. Do not fill current from the manifest.
- First `version` writes `0.1.0`. Placeholders `0.0.0` and `0.1.0` still write `0.1.0`.
- Exception: if the manifest is already above `0.1.0` (SemVer 2.0 precedence, build metadata ignored), `version` does not write. `check` fails and names the fix: tag the version you meant.
- Shallow vs empty stays unverified vs never released (`okm-ls1`).
- Remaining implementation: `version` must refuse that write; `check` must fail and name it; `okm-tur` must stop omitting untagged-and-ahead.

## Open questions

- knope's file overlay remains a real counterexample if oakum ever wanted a hybrid. It would need a new ADR; ADR-0014 as amended still rejects it.

## Raw data

| Tool | Current version when tags are empty | First next (if it computes one) | Manifest used as current? |
|---|---|---|---|
| knope `from_tags` | `PackageVersions::default()` (`stable: None`) | 0.0.0 if bumping stable with no files overlay | no |
| knope `Package::new` | tags, then `update_version` from files | file version if no stable tag (since 0.21.3) | **yes, overlay** |
| semantic-release | no `lastRelease.version` | `1.0.0` | no |
| changesets | n/a (does not ask tags) | bump from `package.json` | **yes** |
| cargo-release | n/a (does not ask tags) | bump from `Cargo.toml` | **yes** |
| release-plz `git_only` | no matching tag → initial release | local project / manifest | **yes** (as next) |
