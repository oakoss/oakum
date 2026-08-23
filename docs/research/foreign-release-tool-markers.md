# Config files that mean another release tool is present

- Date: 2026-08-23
- Author: Jace Babin
- Scope: for `init` (`okm-0s5`), which on-disk markers identify the seven tools in [specs/init.md](../specs/init.md), and which other versioning tools would still be invisible.

## Question

`oakum detect-release-tools` matches a table of paths and a few parsed keys. Are any documented config names for those seven tools missing, and which other versioning tools would still look like a greenfield `init`?

## Sources

- [semantic-release configuration](https://semantic-release.org/usage/configuration/), fetched 2026-08-23
- [release-plz config](https://release-plz.dev/docs/config), fetched 2026-08-23
- [release-please manifest releaser](https://github.com/googleapis/release-please/blob/main/docs/manifest-releaser.md) and [troubleshooting](https://github.com/googleapis/release-please/blob/main/docs/troubleshooting.md), fetched 2026-08-23
- [knope packages / config file](https://knope.tech/reference/config-file/packages/) and [default config](https://knope.tech/reference/default-config/), fetched 2026-08-23
- [changesets config file](https://changesets.dev/guide/config) and [`packages/config/src/index.ts`](https://github.com/changesets/changesets/blob/main/packages/config/src/index.ts) `read()`, fetched 2026-08-23
- [bumpy configuration.md](https://github.com/dmno-dev/bumpy/blob/main/docs/configuration.md), fetched 2026-08-23
- [nx.json reference](https://nx.dev/docs/reference/nx-json), fetched 2026-08-23
- [cargo-release reference](https://github.com/crate-ci/cargo-release/blob/HEAD/docs/reference.md), fetched 2026-08-23
- [cargo-dist config](https://axodotdev.github.io/cargo-dist/book/reference/config.html), fetched 2026-08-23
- release-plz discussion [#1019](https://github.com/release-plz/release-plz/discussions/1019) (config lives in `release-plz.toml`, not `Cargo.toml` metadata), fetched 2026-08-23

## Findings

### The seven tools in the spec

| Tool | Documented on-disk markers (2026-08-23) | Detector |
|---|---|---|
| knope | `knope.toml` only. `--generate` writes that name. Default config exists **without** a file. | `knope.toml` |
| changesets | `.changeset/config.json` only (`read()` joins that path). A closed PR for `config.cjs` did not land. | `.changeset/config.json`, plus orphan bump files |
| bumpy | `.bumpy/_config.json` | `.bumpy/_config.json` |
| release-please | `release-please-config.json` and `.release-please-manifest.json` (JSON). Action inputs can point at other paths. Older Action-only setups have **no** JSON files. | both default JSON names |
| release-plz | `release-plz.toml` **or** `.release-plz.toml` next to root `Cargo.toml`. Config is optional; defaults apply with **no file**. `[workspace.metadata.release_plz]` is **not** documented as a config source; maintainers said they would take a PR for `Cargo.toml` metadata. | both toml names, plus the spec's Cargo metadata table |
| semantic-release | cosmiconfig `release`: `.releaserc` plus `.yaml`/`.yml`/`.json`/`.js`/`.ts`/`.cjs`/`.mjs`; `release.config.(js\|ts\|cjs\|mjs)`; `package.json` `release` key. GitHub `HEAD` docs omit `.ts`; the live site includes it. | `.releaserc` / `.releaserc.*`, `release.config.{js,cjs,mjs,ts}`, `package.json` key |
| nx release | `release` on root `nx.json`. Project-level `release` can live in `project.json`. Zero-config `nx release` needs **no** `release` key. | `nx.json` `release` key only |

### Misses for those seven

1. **No file, but the tool is in use.** knope without `knope.toml`, release-plz with no toml (defaults), nx release with no `release` key, semantic-release with only CLI flags in CI. File-only detection cannot see these.
2. **GitHub Actions as the only marker.** release-please v4 often has the JSON pair, but older workflows pass `release-type` in YAML and never write those files. A scan of `.github/workflows` is out of this slice.
3. **Custom paths.** release-please `--config-file` / Action `config-file`; cargo-release `--config`. We only look at defaults at the repo root.
4. **nx `project.json` `release`.** Official, but per-package. Detecting every `project.json` is a different walk than the spec table.

`[workspace.metadata.release_plz]` is a spec marker, not a documented release-plz input. Keeping it is a false-positive risk against a crate that stored unrelated metadata under that key; dropping it would miss nothing the tool reads.

### Tools not in the spec

A repository that only uses these would still look like a greenfield `init`:

| Tool | Marker | Why it matters |
|---|---|---|
| cargo-release | `release.toml`, `[workspace.metadata.release]` / `[package.metadata.release]` | Common Rust versioner; not in the init table |
| cargo-dist | `dist-workspace.toml`, `dist.toml`, `[workspace.metadata.dist]` | ADR-0012's downstream. It reacts to tags; it does not plan versions. `init` writing oakum config next to dist is fine. |
| release-it / standard-version / auto | `.release-it.json`, `.versionrc*`, `.autorc` | Not in the surveyed user repos for v0 |

## Conclusions

Default files for the seven named tools are covered, including `release.config.ts` and `.release-plz.toml`. Remaining misses are a missing file (defaults or CI-only) and non-default paths.

If `init` should refuse any in-use versioner, the next hole is cargo-release (`release.toml`), not another semantic-release extension.

## Implications / actions

- Do not add cargo-dist as a migrate target: it is not a competing planner.
- If CI-only release-please / semantic-release must be caught, that is a workflow scan, not another filename.
- Spec vs release-plz docs: keep or drop `[workspace.metadata.release_plz]` as an explicit preference; it is not what release-plz reads.

## Open questions

- Whether `init` should read `.github/workflows/*.yml` for `release-please-action`, `release-plz/action`, `semantic-release`, and `changesets/action`.
- Whether cargo-release belongs in the init table for v0 (no surveyed target repo uses it).
