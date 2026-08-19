# What if tags were the source of truth and manifests were output?

- Status: draft
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

Recovered from the design session of 2026-08-18, where it was recommended but never confirmed. It is foundational enough that [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md) and [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) both depend on the answer without stating it.

Three options were on the table for "what version are we at?":

- **Manifests** — read the current version from `package.json` / `Cargo.toml`, compute the next, write it back. What changesets and bumpy do.
- **Git tags** — derive the current version by scanning tags, compute the next, write manifests as *output*. What knope does, in `PackageVersions::from_tags`.
- **A dedicated manifest file** — release-please's `.release-please-manifest.json`, a version per path.

The recommendation was tags.

## Why it might matter

A manifest can be hand-edited, half-merged, or sitting in an unmerged pull request. A tag is the record of what actually shipped, which is the question being asked when computing the next version.

**It yields a drift check for free.** If a manifest version exceeds the highest tag, something bumped without shipping. That is not a special case under this model — it is the natural invariant. It would have caught the review-cycle 0.14.0 state observed the same morning: version bumped, release drafted, no tag.

**It makes prerelease channels nearly free later.** Target comes from change files, counter from the highest matching tag, nothing committed. Choosing manifests now means adding committed state to add channels later, which is precisely the design that gives changesets its `pre.json` footguns.

## Sketch

One precondition it forces, which is already satisfied here: a shallow clone has no tags, so `check` must verify the repository was fetched with full history and fail loudly rather than concluding "never released" and computing `0.1.0`. Every workflow in this repository already uses `fetch-depth: 0`.

Worth banking for the day channels are actually wanted, none of it to be built now: channel identity comes from the **branch**, not a committed mode file; the counter comes from the registry or from tags, so a reset cannot corrupt it; promotion is an ordinary merge with no special mode; and within a prerelease cycle inter-package dependencies are exact-pinned so a `@next` install is always coherent.

## Open questions

- How this interacts with the cascade rules. [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) reasons about "the dependent's **published** range" — under tags-as-truth, published means the range as of the last tag, not as of the working tree, and those differ inside an open version pull request.
- What happens in a repository with no tags at all, which is the bootstrap case oakum itself is in right now.
- Whether `oakum` is a delivery artifact whose own version must therefore come from a tag it has not yet cut — the same circularity [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) records for self-hosting.

## Related work

- knope's `PackageVersions::from_tags` — the reference implementation
- [ADR-0011](../decisions/0011-stop-at-the-tag.md) — oakum already treats the tag as the boundary it owns, which this would make the boundary it *reads from* as well
