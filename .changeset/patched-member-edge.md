---
oakum: major
---

### Changed

A Cargo dependency the workspace root's `[patch.crates-io]` or `[patch."<url>"]` points at a workspace member, while the member's version meets the declared range, now counts as that member's dependent, as a path dependency does. cargo builds the member in the registry or git crate's place only while it meets the range, and `version` bumped such a member past it and left the range alone (`helper = "0.1"` beside a `helper` 0.2.0), so cargo silently built the registry's `helper` instead. `version` now retargets the range (`helper = "^0.2.0"`), or leaves a git declaration with no `version` as it is, and bumps the dependent with it; `check` reaches the dependent from a change to the member, so an edit to the member that passed `check --strict` now asks the dependent for a bump file. A workspace whose `[patch]` closes a cycle with a member's dev-dependency now fails discovery, as any dependency cycle does. A patch cargo leaves unused because `Cargo.lock` holds another version it picks instead, which cargo warns about on every build, still counts; a `[patch]` keyed by a registry's name, or in `.cargo/config.toml`, is not recognized.
