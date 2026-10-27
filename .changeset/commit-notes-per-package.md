---
oakum: minor
---

With commits-only intent, each commit's summary now reaches only the changelogs of the packages that commit names. Before, every package releasing from a range listed every commit in the range, prefixed with the package it belonged to, so `alpha`'s changelog carried `- beta: ...` lines. Each commit now lands in its own section by its own level: a `fix` under `### Fixed`, a `feat` under `### Added`, where before all of a package's commits shared the section of its highest level.

A changelog template's `file` field for a commit-derived change now holds the commit's hash, where it read `commits`.

A Cargo crate and an npm package with the same name in one directory now both release from an unscoped commit that touches it, as coverage already counted. Before, commits-only planning refused with "commit-derived package `<name>` matches more than one workspace package".
