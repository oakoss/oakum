# Renovate rule matching

Read 2026-08-20 from Renovate's source at tag `44.35.2`, fetched per file from `raw.githubusercontent.com/renovatebot/renovate/44.35.2/`. [ADR-0025](../decisions/0025-support-one-rust-version.md) makes `Cargo.toml`'s `rust-version` equal `.mise.toml`'s Rust pin, which puts the supported floor behind a Renovate rule; this records why that rule fires and the two ways it would stop without erroring.

## The rule under test

`.github/renovate.json` sets `automerge: true` globally and groups minor and patch updates into one pull request. The last entry in `packageRules` excepts the toolchain:

```json
{
  "matchManagers": ["mise"],
  "matchPackageNames": ["rust"],
  "groupName": null,
  "automerge": false
}
```

Four things have to hold for that to fire. Each was checked against the file that decides it.

## The manager id is `mise`, not its display name

`lib/modules/manager/mise/index.ts:23` sets `displayName = 'mise-en-place'`, which is not what `matchManagers` compares against — the id comes from the directory name. Its `defaultConfig.managerFilePatterns` lead with `'**/{,.}mise{,.*}.toml'`, which matches `.mise.toml`.

## The table form is extracted

`lib/modules/manager/mise/extract.ts:103` defines `parseVersion`, which handles a bare string, an array, and — at line 114 — `isObject(toolData) && isNonEmptyString(toolData.version)`. So `rust = { version = "1.97.1", components = "clippy,rustfmt" }` yields `1.97.1` and is a dependency Renovate sees.

## `matchPackageNames` compares `packageName`, never `depName`

`lib/util/package-rules/package-names.ts:9` — `PackageNameMatcher.matches` destructures `{ packageName }` from the input config, returns `false` when it is absent, and otherwise calls `matchRegexOrGlobList(packageName, matchPackageNames)`. There is no `depName` fallback.

`lib/modules/manager/mise/upgradeable-tooling.ts:179` maps `rust` to `{ packageName: 'rust', datasource: RustVersionDatasource.id }`, so the literal `"rust"` matches.

## Later rules win, and the repository's own rules are last

`lib/util/package-rules/index.ts:45` iterates `config.packageRules` in order and, on each match, applies the rule over what came before — the comment at line 48 reads "Package rule config overrides any existing config". Presets are merged ahead of the repository's own entries, so the rule above sits after the `matchPackageNames: ["*"]` grouping rule and overrides both its `groupName` and the global `automerge: true`.

Corroborated by resolving this repository's config against `44.35.2` (725 effective rules after presets): `rust` patch, minor, and major each come out `automerge=false, groupName=null`, against a control of `dprint` minor coming out `automerge=true` in the grouped branch. The leftover `groupSlug` on the grouping rule is inert once `groupName` is null — `branch-name.js:42` gates the group-branch path on `if (update.groupName)`.

## Renovate cannot keep the two files in step by itself

`lib/modules/manager/cargo/extract.ts` contains no case-insensitive match for `rust-version`, `rustVersion`, or `msrv` — zero hits over the whole file. The cargo manager reads only the dependency sections, so a pin bump edits `.mise.toml` alone and leaves `Cargo.toml` behind. That is why `crates/oakum/tests/layout.rs::the_declared_floor_equals_the_pinned_toolchain` exists rather than trusting the rule to keep them equal.

## Two ways this goes quiet

**A rule copied to another tool will probably match nothing.** `rust` is unusual in `upgradeable-tooling.ts` for carrying a bare `packageName`. The neighbouring entry at line 186 maps `swift` to `swift-lang/swift`, so `matchPackageNames: ["swift"]` would match no package, leave automerge on, and report no error. Check the `packageName` in that file before reusing this shape.

**Config validation is not evidence the rule fires.** Running `renovate-config-validator --strict` against `.github/renovate.json` with `matchManagers: ["mise"]` replaced by `["bogusmgr"]` prints `Config validated successfully against 1 file(s)` — byte-identical to the run against the real file. A typo in either match key degrades to silent automerge.

Re-check all of the above on a Renovate major, and whenever another tool is added to the same rule.
