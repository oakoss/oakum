# How release tools pin their own version, and what their configs do with unknown keys

- Date: 2026-08-18, revised 2026-08-19
- Author: Jace Babin
- Scope: How eight release tools prevent their own behavior from changing without a commit in the user's repository.

## Question

A release tool's version determines bump math, changelog output, and manifest writes. If CI resolves "latest" every run, behavior can change with no commit in the repository. How do existing tools handle this, and what happens to config keys the tool no longer understands?

## Sources

`action.yml` files, entrypoint scripts, and config-parsing source for changesets, release-please, knope, release-plz, semantic-release, cargo-dist, bumpy, and nx. bumpy 1.18.1 read from the published registry tarball (`npm pack @varlock/bumpy@1.18.1`). `@changesets/config` 3.1.1 and 4.0.0, and `packages/cli/CHANGELOG.md` from `changesets/changesets` `main`, read 2026-08-19.

## Findings

### Pinning

| Tool | Pinned by | SHA-pinning the action pins the tool? |
|---|---|---|
| **cargo-dist** | mandatory exact `cargo-dist-version` in config, baked into the generated workflow | n/a — no action |
| **release-plz** | version default hardcoded in `action.yml`, installed with `cargo-binstall` | **yes** |
| **release-please** | tool ncc-bundled into a committed `dist/index.js` | **yes** |
| **changesets** | the repository's own `@changesets/cli` dependency; the action resolves it with `require.resolve` | n/a — action carries no CLI |
| **knope** | `version:` input; **empty default resolves latest** | **no** |
| **bumpy** | recommended workflow reads the version out of `package.json` with `jq`; the simple example is unpinned `bunx` | n/a |
| **semantic-release** | official guidance is `npx semantic-release@25` — major only; its own workflow pins `npx semantic-release@25.0.1` exactly, Renovate-managed | n/a |
| **nx** | devDependency plus lockfile | n/a |

knope's README is candid: the pinned form is labeled recommended, the unpinned one carries *"You will eventually experience breaking changes if you do this."*

### Unknown config keys

| Tool | Runtime behavior |
|---|---|
| **release-plz** | `#[serde(deny_unknown_fields)]` on root and nested structs — **hard error** |
| **changesets** | valibot `object()`, which *"removes unknown entries"* — silently dropped |
| **bumpy** | schema declares `additionalProperties: false`; `loadConfig` is `readJsonc` plus a spread — **never validates** |
| **release-please** | no JSON-Schema validator among its dependencies — read-if-known |
| **knope** | no `deny_unknown_fields` — silently ignored |
| **semantic-release** | cosmiconfig load then a plain spread; no option allowlist — silently ignored |
| **nx** | schema sets `additionalProperties: false` on the `release` block; runtime enforcement unverified |

Only release-plz enforces at runtime. A shipped JSON Schema is editor decoration unless the binary validates independently — bumpy is the proof.

### The failure this produces, concretely

`@changesets/config@3.1.1` validates a `prettier` option at runtime — *"The `prettier` option is set as ... when the only valid values are undefined or a boolean"*. In `4.0.0` it is gone: `schema.json` declares 14 properties and `prettier` is not among them, and `additionalProperties` is unset, so the schema does not reject it either. Those are one choice seen twice: the runtime strips because valibot `object()` strips, and `schema.json` is generated from those same valibot schemas.

**Upstream documented the change; the runtime still says nothing.** [#1994](https://github.com/changesets/changesets/pull/1994) removed the option in favor of `format`, which takes `"auto"`, `"prettier"`, `"oxfmt"`, `"deno"`, `"dprint"`, or `false`, and the changelog spells out the migration: *"If you previously used `prettier: false`, migrate to `format: false` or remove the option to use automatic formatter detection."* So the defect is not a key vanishing unannounced. A user who did not read a major-version changelog carries `"prettier": false` forward, gets **no error and no warning** at runtime or in the editor, and formatting changes underneath them. A `deny_unknown_fields` binary turns that into a one-line failure naming the key.

Silent dropping is deliberate policy elsewhere too, in a narrow form: [#1879](https://github.com/changesets/changesets/pull/1879) *"Removed warning messages about using v1 configs. They will now be silently ignored"* — v1 configs specifically, not unknown keys in general. (`packages/cli/CHANGELOG.md`, read 2026-08-19. The pull request's own title is about replacing the prompt library; the quoted line is its changeset entry.)

Separately, changesets PR [#1744](https://github.com/changesets/changesets/pull/1744) ("Prettier v3") bumped the Prettier version used *"in the absence of the local installation"* to v3, changing changelog formatting with no commit in any repository that does not install Prettier itself.

semantic-release #2140 is the incident: v18 shipped a new Node floor and broke unpinned pipelines. The resolution was a documentation change. The maintainer's position in discussion #3955 is the honest statement of the trade-off — *"when you pin to any degree, that pin grows stale."*

### cargo-dist's model, in four parts

1. **Config declares an exact version, mandatorily** — a Cargo `Version`, explicitly *not* a `VersionReq`, so there is no resolution step to drift.
2. **CI is generated with the version baked in**, so the pin is a reviewable artifact. `[dist.github-action-commits]` SHA-pins third-party actions from config too.
3. **The binary refuses to run on mismatch, in both directions.** `MismatchedDistVersion` fires on exact inequality with the remedy in the message; `dist generate --check` fails CI on drift.
4. **The version is stamped into the output.** `dist-manifest.json` carries `dist_version`, and readers classify unknown-future versions explicitly as `Format::Future`.

Every other tool writes an unversioned "generated by X" link, so a bad release carries nothing identifying which version produced it.

### `$schema` placement

changesets writes a version-pinned unpkg URL, which freezes at init and validates against a version you no longer run. release-plz points at an unversioned `latest.json`, current with `main` rather than with your binary. bumpy and nx point at a local path inside the installed package, so the schema tracks the installed version.

### Migration

nx is the only tool with a generic automated config migration: `nx migrate` runs versioned scripts that rewrite config into a reviewable diff. `knope --upgrade` is a weaker opt-in variant — its own documentation scopes it to updating *"from any deprecated (but still supported) syntax to the newer syntax"* (knope `command-line-arguments.md`), and that phrase is how silent reinterpretation returns.

**bumpy's "automatic migration" is not a version-to-version migrator.** It is a one-time changesets adoption path inside `init` that renames `.changeset/` to `.bumpy/` and maps config fields. There is no mechanism for migrating a bumpy config across bumpy majors.

## Conclusions

cargo-dist's is the only model where behavior provably cannot change without a commit. It needs two additions: runtime rejection of unknown keys, and migration that produces a reviewable diff or an error — never a third path where old config parses with new meaning.

## Implications / actions

- Config carries an exact `tool-version`. The binary refuses to run on mismatch in either direction, naming the upgrade command.
- `upgrade` is the one command exempt from that gate. It validates against the old schema, migrates, writes the new version, regenerates the schema, and reports what changed — writing nothing if migration fails.
- `deny_unknown_fields`, enforced by the binary. Ship a schema as well, pointed at a local generated file so it tracks the installed version.
- Stamp the version into the version-PR body and the changelog footer.
- Oakum does not generate CI. `check` instead parses the workflow for the tool's own invocation and compares the version it finds, reporting three states — matching, mismatched, and **not found**.

## Open questions

- An exact pin means every patch release of oakum forces an `upgrade` commit in every consuming repository. dist users accept this; whether the churn is worth it across five-plus repositories is a judgment call, not a finding.
