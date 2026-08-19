# Name the project oakum

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

The project needs a name for the GitHub repository, the crate, the binary, and the npm fetcher package. What should it be called?

## Decision Drivers

- Available on crates.io and npm, since cargo-dist publishes to both
- Short enough to type constantly
- Not tied to one subsystem, because the scope spans version math, changelogs, tags, releases, and cascade
- Searchable — a common word makes the project unfindable

## Considered Options

- `polybump`, `depbump`, `multibump` — functional coinages naming the mechanism
- `releasesmith`, `versionsmith` — functional, matching the `-smith` house style
- `colophon`, `quoin`, `ferry` — metaphor names
- `oakum`

## Decision Outcome

Chosen option: **oakum**. Oakum is the tarred fiber driven into the seams between a ship's planks to keep the hull watertight. It names the failure the tool exists to prevent — releases leaking away unnoticed — rather than the mechanism it currently uses, which means it stays accurate as the scope grows past version bumping.

The functional coinages were the closer fit for this ecosystem's naming convention, where `bumpy` derives from "bump" and `release-plz` from "release". They lost on two counts: every one of them anchors on version bumping and would read oddly once tagging, release creation, and handoff verification carry equal weight, and `polybump` in particular reads as derivative of the tool it replaces.

`colophon`, `quoin`, and `ferry` were all taken on crates.io.

### Consequences

- Good, because it is free on crates.io, npm, and `oakoss/oakum`, with no meaningful GitHub collisions
- Good, because five characters types cleanly in `oakum check --explain`
- Bad, because the meaning is opaque until explained; the README opens with the explanation for that reason
- Neutral, because it starts with "oak", which suits the `oakoss` umbrella without binding the project to it if it ever graduates
