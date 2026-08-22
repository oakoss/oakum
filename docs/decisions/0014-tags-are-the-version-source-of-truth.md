# Read the current version from tags; write manifests as output

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Computing the next version requires knowing the current one. Three places claim to hold it: the package manifest, the git tag history, and a dedicated version file. They disagree exactly when it matters — inside an open version pull request, on a half-merged branch, after a bump that never shipped. Which one does oakum read?

## Decision Drivers

- The question being asked is "what shipped?", and only one of the three candidates records that
- [ADR-0011](0011-stop-at-the-tag.md) already makes the tag the boundary oakum owns
- Prerelease channels are out of scope for v0 ([ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)) but should not be made expensive later
- The failure this project exists to catch — a version bumped that never shipped — has to be detectable

## Considered Options

- **Manifests** — read the version from `package.json` or `Cargo.toml`, compute the next, write it back. What changesets and bumpy do.
- **Git tags** — derive the current version by scanning tags, write manifests as output. What knope does, in `PackageVersions::from_tags`.
- **A dedicated version file** — release-please's `.release-please-manifest.json`, one version per path.

## Decision Outcome

Chosen option: **git tags**. Manifests become output rather than input.

A manifest can be hand-edited, half-merged, or sitting in an unmerged pull request. A tag is the record of what actually shipped, which is the question being asked.

**Read tags reachable from `HEAD`, not every tag in the repository.** `git tag --merged` is the whole difference, and it is one qualifier on one query. Reading all tags means a maintenance branch sees the tags from `main` and computes a version from a release line it is not on. Reading reachable tags means a branch's own history *is* its release line, which is correct on trunk and correct on a maintenance branch without a branching model being built for it. Choose the unqualified query and [idea 0007](../ideas/0007-maintenance-release-branches.md) becomes unreachable without a rewrite.

### Consequences

- Good, because drift detection is the natural invariant rather than a feature: a manifest version above the highest reachable tag means something bumped without shipping
- Good, because prerelease channels stay cheap. The target comes from change files, the counter from the highest matching tag, and nothing derived is ever committed — the design that gives changesets its `pre.json` footguns is committed state, and this has none
- Good, because it deletes a manual step in the release-train workflow: the pull request that bumps `main` to the next minor after cutting a release branch exists only because the version lives in a manifest
- Bad, because it forces a precondition on every consumer. A shallow clone has no tags, so `check` must verify full history and fail loudly rather than concluding "never released" and computing `0.1.0`. As of 2026-08-20 this repository has five `actions/checkout` steps: two set `fetch-depth: 0` (`ci.yml` static-analysis and secret-scan); the other three — `ci.yml` tests, `audit.yml`, and `codeql.yml` — check out shallow and would need full history before they could run `check`. (When this ADR was written the count was six, including a since-deleted MSRV job; [ADR-0025](0025-support-one-rust-version.md) removed that job.)
- Neutral, because manifests are still written and committed. They stop being *read* as authority, which is invisible until the two disagree

### Empty tag history (amended 2026-08-21)

A successful look that finds zero reachable tags is **never released**. Current version is none. A brand-new repository and a long-lived repository that never tagged are the same observable; there is no third "green-field" outcome.

This is not the shallow-clone case. A clone that did not look remains **unverified** (the precondition above). Never collapse the two.

Do not read the manifest as current because tags are empty. That is the changesets / cargo-release family this ADR already rejected. knope's file overlay is a later hybrid; it is not this decision.

**First `version` writes `0.1.0`.** That is the number this ADR already used as what the never-released path would compute. [ADR-0022](0022-zero-major-versioning.md) does not pick the bootstrap number; it only decides how later bumps work below `1.0.0`. Init tools start at `0.1.0` (Cargo) or `1.0.0` (npm), not `0.0.1` — sources in [empty tag history](../research/empty-tag-history.md).

**Exception:** if the manifest already claims a version **above** `0.1.0` (SemVer 2.0 precedence, build metadata ignored), `version` does not write. `check` fails and names the fix: tag the version you meant. Placeholder manifests `0.0.0` and `0.1.0` are not this case.

Grounded in [empty tag history](../research/empty-tag-history.md).

### Confirmation

It would have caught the review-cycle 0.14.0 state observed the morning this was decided: version bumped, release drafted, no tag. Under a manifest-as-truth model that state is indistinguishable from a completed release.

## Pros and Cons of the Options

### Manifests

- Good, because it is what the two closest reference tools do, so it is the least surprising
- Bad, because the authority is a file anyone can edit, and the tool cannot tell an intentional hand-edit from a bad merge
- Bad, because prerelease support then requires committed state to track the channel and counter

### A dedicated version file

- Good, because it separates the authority from the manifest, so hand-edits to `package.json` cannot confuse it
- Bad, because it is committed state with all of the manifest's failure modes and none of its other uses
- Bad, because it is a file oakum would own and users would have to understand, which [ADR-0004](0004-derive-facts-configure-preference.md) argues against when the fact is already derivable

## More Information

- [idea 0004](../ideas/0004-tags-as-the-version-source-of-truth.md) — the exploratory note this promotes, kept for its prerelease-channel design notes
- [ADR-0010](0010-derive-cascade-from-declared-ranges.md) reasons about the dependent's "published" range. Under this decision that means the range as of the last reachable tag, not as of the working tree — and those differ inside an open version pull request
- [ADR-0007](0007-pin-the-tool-version-in-config.md) — the self-hosting circularity: oakum's own version must come from a tag it has not yet cut
- knope's `PackageVersions::from_tags` is the reference implementation
- [empty tag history](../research/empty-tag-history.md) — knope, semantic-release, changesets, cargo-release, and release-plz on a full clone with no tags (`okm-coc`)

**Settled 2026-08-21:** how reachable tags are parsed is [ADR-0030](0030-derive-read-tag-shapes.md).

**Settled 2026-08-21 (`okm-coc`):** a successful look with zero tags is never released. See *Empty tag history* above. The remaining work is implementation: `version` must not write when the manifest is already above `0.1.0`; `check` must fail and name that case; `okm-tur` must stop treating untagged-and-ahead as "not drift."
