# Cascade only along runtime edges

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

When a package is released, which of its dependents must also be released? Every dependency edge is a candidate, but an edge that exists only at build time in the *dependent's* own toolchain cannot change what that dependent ships. Cascading along it produces releases nobody needs; not cascading along a real edge ships a fix that never reaches users. Which edge kinds propagate a release?

## Decision Drivers

- A dependent that ships changed behavior must be released, or the fix is undelivered
- A dependent whose shipped artifact is byte-identical must not be released, or every tooling bump releases the world
- The answer has to be derivable from manifests, not declared per change
- Surveyed tools disagree with each other, so consensus is not available as a shortcut

## Considered Options

- Cascade along every dependency edge
- Cascade along runtime edges only
- Match each ecosystem's dominant tool, whatever it does

## Decision Outcome

Chosen option: **cascade along runtime edges only**.

For npm that is `dependencies`, `peerDependencies`, and `optionalDependencies`; not `devDependencies`. For Cargo it is `dependencies` **and `build-dependencies`**; not `dev-dependencies`.

The Cargo line is three-way, and Cargo's own documentation draws it: `dev-dependencies` "are not used when compiling a package for building" and are "not propagated", while a build script cannot see `dependencies` and needs `build-dependencies` to do its work. A `build-dependencies` change therefore *can* change the compiled output, which makes it a runtime edge for release purposes even though its name suggests otherwise. No surveyed tool acts on this distinction: release-please merges all three kinds, and knope does not handle `build-dependencies` at all.

**Development edges stay in the graph anyway.** They never trigger a bump, but their version ranges are rewritten whenever the dependent is being released for some other reason. Dropping them from the graph would leave stale ranges in published manifests. changesets keeps them for exactly this reason.

**Rewriting a pin is universal; bumping is conditional.** These are separate operations and conflating them is a shipped-defect pattern, not a hypothetical: linesmith's ADR-0027 (`oakoss/linesmith`, `docs/adrs/0027-knope-for-release-automation.md`, accepted 2026-05-23) captures release-plz bumping `linesmith-core` 0.1.3 → 0.2.0 without rewriting the binary's pin, publishing a workspace that did not compile.

**Cycles are a precondition failure.** Detect them before planning and refuse, naming the packages. Keeping development edges in the graph makes cycles more likely, not less — release-please [#2452](https://github.com/googleapis/release-please/issues/2452) is a user with two eslint plugins that devDepend on each other, getting `found cycle in dependency graph: eslint-plugin-treekeeper -> eslint-plugin-node-specifier -> eslint-plugin-treekeeper` and no way forward. Refusing with the cycle named is the difference between that and a tool that hangs or picks arbitrarily.

### Consequences

- Good, because a tooling-only change stops releasing packages whose output did not change
- Good, because `build-dependencies` is handled correctly, which nothing else does
- Bad, because a library that *bundles* a development dependency into its published artifact will be under-released; that case is what [ADR-0009](0009-delivery-artifacts-always-cascade.md) and `resolves-dependencies-at` exist to cover
- Neutral, because keeping development edges in the graph means the graph is larger than the cascade set, and the two must not be confused in the implementation

### Confirmation

The planner must reproduce linesmith's release history with no false positives. A cascade fired along a `dev-dependencies` edge is a bug, not a preference.

## Pros and Cons of the Options

### Cascade along every edge

- Good, because it never under-releases
- Bad, because it releases constantly. Every repository surveyed has private `@repo/eslint-config` and `@repo/typescript-config` devDependencies shared by every package, so one lint-config bump would release the entire workspace.

### Match the dominant tool

- Good, because it is defensible by convention
- Bad, because the tools disagree. Verified behavior for "a private tsconfig devDependency bumps — does the published dependent bump?": changesets v2 and v3 **no**, bumpy **no**, knope **no** (it never cascades at all), release-please **yes**, and its yes is not a considered position. `always-link-local` is documented as limiting local dependency bumps to the declared SemVer range, but has had no effect since v17 — parsed from the manifest, carried through `ManifestOptions`, assigned in the `NodeWorkspace` constructor, and never read again (issue [#2876](https://github.com/googleapis/release-please/issues/2876), open, verified 2026-08-18). Without it there is no way to say "leave this package alone, its published range already covers the new version", which is exactly the gate [ADR-0010](0010-derive-cascade-from-declared-ranges.md) depends on.

## More Information

The canonical community ruling is Andarist on changesets [#921](https://github.com/changesets/changesets/issues/921): "A change of dev dependency shouldn't affect the release of the package… If a dev dep is a build tool and a change in it requires a new release of its dependants then it's recommended to create an explicit changeset for those dependants."

The counter-case — a library that bundles a development dependency into its published output — is real and unresolved upstream. changesets #944 has been open since 2022 with an approved-but-unmerged PR #1159; bumpy shipped it as `releaseTriggeringDevDeps`. Oakum's answer is the derived delivery-artifact rule rather than a per-edge declaration, because a per-edge list is a hand-maintained restatement of the dependency graph and will rot.

- [ADR-0009](0009-delivery-artifacts-always-cascade.md) — why range-gating this cascade is wrong for binaries
- [ADR-0010](0010-derive-cascade-from-declared-ranges.md) — when a runtime edge actually fires
