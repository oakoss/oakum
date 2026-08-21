# Write only the changeset-format intersection

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Oakum reads and writes `.changeset/*.md` so adopting it costs no migration. Two other parsers read those same files in repositories we care about — `@changesets/cli` and knope's `changesets` crate. Which parts of the format are safe to write, and can the format be extended?

## Decision Drivers

- During a shadow period, the existing tool must keep releasing from the same directory
- A silent no-op is worse than a hard error, because nothing surfaces it

## Considered Options

- Extend the frontmatter with reserved keys and richer values
- Adopt the parts of the format both parsers agree on, and nothing else

## Decision Outcome

Chosen option: **write only the intersection** — first line exactly `---`; one `name: patch|minor|major` per line with an unquoted key; no blank lines; no duplicate keys; closing `---`; no preamble; no BOM.

Four extensions were considered and all four fail against a three-parser intersection. `$`-prefixed reserved keys and object-valued entries are **fatal in `@changesets/parse`**, which maps every frontmatter key to a package release and rejects non-string values. `none` as a bump type and empty bump files are **fatal or silently wrong in knope**: `none` becomes a custom change type, which means a patch bump whose summary is discarded, and an empty frontmatter block errors outright.

**Amended 2026-08-21 by [ADR-0028](0028-releaseless-bump-files-like-bumpy.md).** Oakum still writes only the intersection for `patch` / `minor` / `major` when the file must survive knope. It may also write `none` and empty frontmatter for `--none` / `--empty`, matching bumpy and `@changesets/cli`. Those shapes remain knope-unsafe; they are oakum features, not intersection claims.

The full case matrix is in [changeset-file-format.md](../research/changeset-file-format.md).

**One case has no intersection at all.** A scoped npm name (`@scope/pkg`) must be quoted to be valid YAML — `@` is a YAML reserved indicator — and must be unquoted to be visible to knope. Oakum quotes it, which is correct for `@changesets/cli` and invisible to knope. That is acceptable only because the two parsers share a directory solely when migrating from knope, whose packages are crates and never scoped. Where both a scoped package and a `knope.toml` are present, refuse rather than write a file one reader silently ignores.

### Consequences

- Good, because a `patch` / `minor` / `major` file oakum writes is read identically by both other parsers
- Good, because tool configuration moves to non-`.md` files inside `.changeset/` (for example `_config.toml`), which every parser skips on extension. Releaseless coverage uses ordinary `.md` shapes per [ADR-0028](0028-releaseless-bump-files-like-bumpy.md), not a second extension
- ~~Bad, because "this change ships no release" cannot be expressed in the frontmatter and needs its own mechanism~~ **Superseded 2026-08-21 by [ADR-0028](0028-releaseless-bump-files-like-bumpy.md)** — releaseless intent uses normal `.md` shapes; knope safety for those shapes is declined
- Neutral, because the one remaining extension channel — prose before the opening `---`, silently discarded by the JS regex — is unusable anyway, since knope's opening delimiter is anchored to line 1

### Confirmation

Pin the `patch` / `minor` / `major` intersection with tests against both parsers, not just against oakum's own. The constraints only bind during migration, which is exactly when nobody is looking for them. Empty frontmatter and `none` are covered by oakum (and JS) tests per ADR-0028; they are not knope Confirmation inputs.

## More Information

Two migration hazards follow from the same research and belong in the migration path rather than here:

- `@changesets/cli` writes **quoted** keys, and knope does not strip quotes, so every file the JS tool writes is a silent no-op under knope.
- `.changeset/README.md`, created by `changeset init`, aborts every knope run. In a knope repository it is already breaking releases.
