# Version private packages only when opted in

- Status: accepted
- Date: 2026-08-21
- Deciders: Jace Babin

## Context and Problem Statement

Should a package that is not registry-publishable (`private: true`, Cargo `publish = false`) still get a version bump, changelog, and tag by default? changesets and bumpy skip those packages unless configured otherwise; oakum's own deployables sometimes want the opposite.

## Decision Drivers

- Switching from changesets or bumpy should not surprise authors: private means "out of the release set" unless they opt in
- Publishability is a repository fact ([ADR-0004](0004-derive-facts-configure-preference.md)); whether an unpublishable package is *versioned* is preference
- finance-tracker-style repos that want tags without a registry must still be able to opt in — they are not the default audience for a migration

## Considered Options

- Version private packages by default; exclude for opt-out
- Align with changesets/bumpy: skip version/changelog/tag for unpublishable packages unless opted in
- Always version everything; gate only registry publish

## Decision Outcome

Chosen option: **align with changesets/bumpy — unpublishable packages are not versioned (bump, changelog, or tag) unless the repository opts in**, because migration friction outweighs oakum's personal-repo default, and opt-in still covers finance-tracker.

**Derived every run:** `Package.publishable` from the manifest — Cargo `publish` null means publishable anywhere, `[]` (and `publish = false`) means nowhere; npm `private: true` means not publishable. Never a falsy check on the Cargo field ([workspace discovery research](../research/workspace-discovery.md)).

**Configured preference:** `private-packages = { version = false, tag = false }` in `_config.toml` (axes independent). Discovery still records `publishable`; `version` / `release` / `check` apply the opt-in when they consume it.

**include/exclude** remains the general package-selection preference for any package (public or private). It does not replace this opt-in: exclude is "do not bump, changelog, or tag this package"; private-packages opt-in is "treat unpublishable packages like public ones for version/tag." Dependency pin rewrites in unmanaged manifests still run when a managed dependency releases — leave-alone is about the package's own version surface, not freezing every byte of its manifest.

### Consequences

- Good, because a changesets migratee keeps the same private-package silence without a config change
- Good, because publishability stays a derived fact and does not become a second graph
- Bad, because finance-tracker and similar must set the opt-in (or use include carefully) before oakum tags them — the opposite of [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)'s earlier "test private-no-publish first" emphasis
- Neutral, because registry publish (post-v0) still reads `publishable` and never this opt-in alone

### Confirmation

Revisit if migrate from changesets still requires a config rewrite for the common case, or if most oakum adopters are private-only deployables and the opt-in becomes universal boilerplate.

## More Information

- Driving bead: `okm-8nu.2`
- Prior art: changesets `privatePackages` default `{ version: false, tag: false }`; bumpy the same shape ([bump-file tool interfaces](../research/bump-file-tool-interfaces.md))
- Amends the emphasis in [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md): private-no-publish remains a first-class *path* (opt-in), not the default for unpublishable packages
