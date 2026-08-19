# What version strings oakum can actually produce

- Date: 2026-08-19
- Author: Jace Babin
- Scope: which version formats survive npm, Cargo, and git, and which are rewritten on the way

## Question

Custom version formats keep coming up — keeping a project below `1.0.0`, encoding an epoch in the major component, appending build metadata after a `+`. Before any of that becomes a config surface, one thing has to be settled: **which version strings survive the round trip to a registry and a tag unchanged?** A format that a package manager rewrites is not a format oakum can offer.

## Sources

- Semantic Versioning 2.0.0, semver.org, read 2026-08-19
- `semver` 7.8.5 (node-semver), executed against a real install
- npm 11.17.0 and pnpm 11.22.0, `pack --dry-run` and `publish --dry-run` on an identical `package.json`
- crates.io API, `/api/v1/crates/{name}/versions`, queried 2026-08-19
- `git check-ref-format`, plus tag creation and resolution in a scratch repository
- `googleapis/release-please` `schemas/config.json`; `@varlock/bumpy` 1.18.1 `config-schema.json`; changesets `docs/config-file-options.md`; `knope-dev/knope` `crates/knope-versioning/src/semver/package_versions.rs` and knope.tech's semantic-versioning concepts page, read 2026-08-19

## Findings

### Build metadata is stripped by npm and preserved by Cargo

The same `package.json`, four commands:

| Command | Reported version | Tarball |
|---|---|---|
| `npm pack --dry-run` | `1.2.3+build.456` | `…-1.2.3+build.456.tgz` |
| `npm publish --dry-run` | `1.2.3` | `…-1.2.3.tgz` |
| `pnpm pack --dry-run` | `1.2.3` | `…-1.2.3.tgz` |
| `pnpm publish --dry-run` | `1.2.3` | — |

**npm says so; pnpm does not.** npm prints `npm warn publish "version" was cleaned and set to "1.2.3"`, naming the field and the new value. pnpm prints `📦 oakum-buildmeta-probe-xyz@1.2.3` with nothing indicating the version differs from the manifest. pnpm also strips in `pack`, where npm does not.

So the silent path is pnpm's, and pnpm is what every repository here declares as its package manager ([ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md)).

crates.io does the opposite — it accepts and preserves build metadata, and the `-sys` ecosystem relies on it to encode the wrapped upstream version (counts re-queried 2026-08-19, unchanged):

| Crate | Versions | With build metadata | Example |
|---|---|---|---|
| `libgit2-sys` | 151 | 51 | `0.18.7+1.9.6` |
| `curl-sys` | 149 | 63 | `0.4.90+curl-8.21.0` |

So one version string means two different things depending on which adapter publishes it. The manifest keeps saying `1.2.3+build.456` either way, so on the JavaScript side the repository and the registry permanently disagree — and through pnpm, nothing says so. That is the divergence class this project exists to catch.

### Build metadata cannot identify a release

Two properties from the spec, both confirmed in node-semver:

- §10: *"Build metadata MUST be ignored when determining version precedence. Thus two versions that differ only in the build metadata, have the same precedence."* `semver.eq('1.2.3+a', '1.2.3+b')` returns `true`.
- Any increment discards it. `semver.inc('1.2.3+meta', 'patch')` returns `1.2.4`, not `1.2.4+meta`.

Together these mean build metadata is decoration attached at publish time, never identity. A release line cannot advance through `+build.1`, `+build.2` — those are one version to every resolver.

### Git is not the constraint

`v1.2.3+build.456`, `v1.2.3+20260819`, `v1002.3.4`, and `oakum/v1.2.3+meta` all pass `git check-ref-format`. A build-metadata tag creates, resolves through `rev-parse`, and appears in `git tag -l --merged HEAD`, which is the query [ADR-0014](../decisions/0014-tags-are-the-version-source-of-truth.md) depends on.

### Epoch encoding works, and needs exactly one operation

Under [Epoch Semver](https://antfu.me/posts/epoch-semver) the version is `{EPOCH * 1000 + MAJOR}.MINOR.PATCH`, which keeps it ordinary semver. `1002.3.4` is valid, `major()` returns `1002`, and it sorts above `2.0.0`.

The arithmetic is friendlier than it looks. A breaking change is `+1` on the composite — `1002.3.4` to `1003.0.0` — which is what an ordinary major bump already does. Only the epoch bump is special: it rounds up to the next multiple of 1000, `1002` to `2000`. That single operation is the whole tooling gap.

### knope already does this; only release-please makes it configurable

| Tool | Support |
|---|---|
| knope | **both, hardcoded, no config key.** `bump_stable` matches on `version.major == 0`: a `Major` rule increments the minor, a `Minor` rule increments the patch |
| release-please | the same two behaviors, but opt-in: `bump-minor-pre-major` — *"Breaking changes only bump semver minor if version < 1.0.0"*; `bump-patch-for-minor-pre-major` — *"Feature changes only bump semver patch if version < 1.0.0"* |
| changesets | none |
| bumpy | none |
| cargo-release | none |

The spec endorses the behavior. §4: *"Major version zero (0.y.z) is for initial development. Anything MAY change at any time. The public API SHOULD NOT be considered stable."* Its FAQ goes further: *"start your initial development release at 0.1.0 and then increment the minor version for each subsequent release."*

### What the ecosystem defaults to, and what its users ask for

Four of the five surveyed tools default to strict semver for an explicit `major` below 1.0.0; only knope does not. So strict is the ecosystem default and the thing a migrating user expects.

The complaint traffic points the other way, though, and is a separate question from the default. changesets issues [#1887](https://github.com/changesets/changesets/issues/1887) and [#1228](https://github.com/changesets/changesets/issues/1228) report `0.x` jumping to `1.0.0` as a defect — but both are about a **peer-dependency cascade** forcing the major, not about an explicit `major` change file. PR [#1936](https://github.com/changesets/changesets/pull/1936), *"fix: use minor bump for 0.x packages when peer dependency causes major"*, was **closed unmerged**; both issues were closed on 2026-06-24 by PR #2090, *"change peer dependent bump type to `patch`"*, shipped in `@changesets/cli` 3.0.0. So changesets answered by not forcing the major, never by mapping major to minor.

**That resolution answers an open question elsewhere in these docs.** [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) leaves open whether a `peerDependencies` cascade should match the triggering level rather than patch. changesets shipped `patch` in v3 specifically to stop `0.x` packages jumping to `1.0.0` — which is the tool with the most exposure to that case answering it directly.

Demand for the behavior itself shows up around release-please's flags instead, where open issues report `bump-minor-pre-major` being silently ignored or ask whether patch-for-feat below 1.0 is intended. Nobody files an issue asking for `0.1.3` to become `1.0.0`. (GitHub issue search, 2026-08-19.)

### Both pre-1.0 options change cascade behavior, for different range shapes

[ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) cascades when the dependent's published range no longer admits the new version. That rule takes the *declared range*, not a caret specifically, so the comparison has to run across range shapes. Against a dependency at `0.1.3`, where strict semver produces `1.0.0` for a breaking change and zero-major produces `0.2.0`:

| Dependent's declared range | admits `0.2.0` | admits `1.0.0` | effect of zero-major |
|---|---|---|---|
| `^0.1.3`, `~0.1.3`, exact `0.1.3`, `0.1.x` | no | no | none — both cascade |
| `>=0.1.3`, `*` | yes | yes | none — neither cascades |
| `^0`, `0.x`, Cargo's bare `"0"` | **yes** | no | **under-releases** — strict cascades, zero-major does not |
| `>=0.1.3 <1.0.0` | **yes** | no | **under-releases** |
| `^0.1.3 \|\| ^1.0.0` | no | **yes** | over-releases |

**So "cascade-neutral" is true only for the tight forms in ADR-0010's own table.** A **library** dependent whose range is bounded solely at `1.0.0` accepts `0.2.0` and rejects `1.0.0`, so zero-major suppresses a cascade that strict semver fires. Delivery artifacts are exempt — [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md) cascades them whatever the range says. Cargo reads a bare `foo = "0"` as `^0`, which puts this in an ecosystem oakum targets rather than in theory.

Whether that suppression is *wrong* is a separate question: an author who wrote `^0` said they accept any `0.x`, and `0.2.0` is any `0.x`, so not releasing them follows the declaration. But it is a behavior change, and calling it neutral hides it.

**Feature-to-patch breaks the common case instead.** `0.1.3` becomes `0.1.4`, which `^0.1.3` still covers — and `^X.Y.Z` is the default range form in both ecosystems. So it suppresses cascades for most dependents rather than for a minority shape.

That difference in blast radius is one input. The reason from the spec is stronger and independent: semver's FAQ recommends incrementing the minor for each release while in `0.y.z`, which is where breaking-to-minor lands and where feature-to-patch does not.

## Conclusions

**Below 1.0.0, mapping a breaking change to a minor bump is safe in the format dimensions** — it produces ordinary semver, both registries accept it, and tags are unaffected. It is **not** cascade-free: **library** dependents declaring `^0`, `0.x`, or any range bounded only at `1.0.0` stop being cascaded, while delivery artifacts cascade regardless under [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md). See [ADR-0022](../decisions/0022-zero-major-versioning.md).

**Mapping a feature to a patch is a different decision** and should not ride along with it. It suppresses cascades for `^X.Y.Z`, the default range form, so its blast radius is most dependents rather than a minority.

**Build metadata is not safely supportable across both ecosystems.** Offering it would mean a Cargo package that keeps it and an npm package that loses it, with nothing reported. If it is ever built, the precondition is to refuse to plan a `+` version for a package with an npm publish target, not to warn after the fact.

**Epoch semver needs nothing from oakum to be compatible** — the strings it produces are ordinary semver. Automating it needs a way to express "this is an epoch change", and [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) leaves no room for a new bump level.

## Implications / actions

- Nothing may assume a major component below 1000, in parsing, formatting, or display. That is the zero-cost move that keeps epoch encoding reachable.
- A publish precondition should reject build metadata for npm targets rather than let the registry disagree with the manifest.

## Open questions

- Whether other registries oakum might reach through a custom publish command — JSR, a private registry — strip or preserve build metadata. Only npm, pnpm, and crates.io were tested.
- Whether pnpm's silence is deliberate or an oversight worth reporting upstream. It strips in both `pack` and `publish` while npm strips only in `publish` and says so, which reads more like a gap than a choice.

**Closed:** whether the strip happens in the CLI or at the registry. It is the CLI — npm prints the correction under `--dry-run`, which never contacts a registry.
