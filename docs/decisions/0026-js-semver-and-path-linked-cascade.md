# Depend on `js-semver` for npm ranges; path-linked edges always cascade

- Status: accepted
- Date: 2026-08-20
- Deciders: Jace Babin

## Context and Problem Statement

`DeclaredRange` stored plain bounds as Cargo's `semver::VersionReq`. That grammar
rejects ordinary npm forms (`||`, space conjunction, hyphen ranges) and misreads
bare and partial versions as carets. How should oakum represent npm bounds under
ADR-0024, and what happens on a Cargo path dependency that declares no version?

## Decision Drivers

- ADR-0010 forbids treating unreadable ranges as satisfied
- ADR-0024: `plan` stays `no_std` + `alloc`
- Prefer not maintaining npm grammar by hand when a small correct crate exists
- Prefer a crates.io dependency over vendoring when attribution / ownership of
  third-party source is undesirable
- Path-only edges are common in Cargo workspaces; refusing them is poor UX

## Considered Options

**Range representation**

- Hand-roll ecosystem-aware `Bounds` on workspace `semver`
- Depend on `js-semver` (`default-features = false`)
- Vendor a snapshot of `js-semver` in-tree

**Path-only / no `version` key**

- Always cascade
- Refuse to plan
- Defer

## Decision Outcome

Chosen option: **depend on `js-semver` 0.3.x with `default-features = false`**, and
**always cascade** on bounds-free path-linked edges.

`js-semver` implements npm / node-semver range semantics, compiles under
`no_std` + `alloc`, and has zero default dependencies. Oakum peels
`workspace:` / `catalog:` protocols itself; the published bounds those protocols
carry (and plain npm ranges) are `Bounds` via `js-semver`. Cargo plains stay on
`semver::VersionReq`. Pinning is by compatible caret on 0.3; API churn is
accepted as the cost of not owning the grammar.

Path-linked edges with no declared version cannot use ADR-0010's range gate:
metadata reports `req=*` for both omitted and authored stars. Discovery records a
bounds-free arm; the planner cascades whenever that dependency releases.
`--explain` states the reason. Nudges to add an explicit `version` for range-gating
belong in `check` / migrate later, not as a hard plan failure.

### Consequences

- Good, because npm peers with `||` and bare pins get correct satisfaction checks
- Good, because `plan-no-std` can depend on the same crate without `std`
- Good, because everyday Cargo path-only workspaces keep planning
- Bad, because `js-semver` is 0.x and may break on minor upgrades; pin carefully
- Bad, because oakum carries two version types (`semver::Version` and
  `js_semver::Version`) and must convert at the boundary
- Neutral, because protocol expansion and catalog resolution remain oakum code

### Confirmation

`plan-no-std` builds with `js-semver` default features off. Unit tests cover npm
bare/partial/`||`/space/hyphen against known versions, and PathLinked → always
cascade. Revisit if `js-semver` becomes unmaintained or fails a real peer range
from a target repository.

## Pros and Cons of the Options

### Hand-roll

- Good, because zero new deps and full control
- Bad, because oakum maintains npm grammar forever

### `js-semver` dependency

- Good, because small, correct, `no_std`, and we do not vendor or attribute in-tree
- Bad, because 0.x and a second `Version` type

### Vendor snapshot

- Good, because no crates.io churn
- Bad, because the project did not want in-tree credit / ownership of third-party
  source; rejected for that preference (MIT-0 would not have required a NOTICE
  for the library alone)

### Path-only: refuse

- Good, because loud when the range gate cannot run
- Bad, because common Cargo manifests fail until every path edge grows a version

## More Information

- [npm ranges versus Cargo's VersionReq](../research/npm-range-vs-cargo-versionreq.md)
- [ADR-0010](0010-derive-cascade-from-declared-ranges.md), [ADR-0024](0024-no-std-plan-crate.md),
  [ADR-0009](0009-delivery-artifacts-always-cascade.md) (delivery-artifact Always is
  separate; path-linked Always is this decision)
- Follow-up: `okm-tnp` consumes satisfaction against these bounds; relative
  `workspace:../foo` still needs an explicit arm or discovery refusal
