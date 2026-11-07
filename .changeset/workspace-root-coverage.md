---
oakum: major
---

### Changed

A change to a Cargo workspace's root `Cargo.toml` (`[workspace.dependencies]`, inherited `[workspace.package]` fields, profiles and other workspace settings) or to resolved registry and git entries in `Cargo.lock` now counts toward each versioned package whose runtime dependencies it reaches, so `check --strict` refuses it until covered and `generate` releases from it. The report names what moved, as in `tm-cli (cargo): changed (via tm-core, tm-store; Cargo.lock: rusqlite)`, and `--json` carries it as `through`. A change reaching only development dependencies, `[workspace.metadata]`, or the version `version` writes still asks nothing; a root file that does not parse on either side leaves coverage `unverified` (exit 2).
