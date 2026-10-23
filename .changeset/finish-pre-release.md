---
oakum: patch
---

`version` finishes a pre-release instead of skipping past it. A patch bump on `2.0.0-rc.1` now gives `2.0.0` rather than `2.0.1`; a minor bump on `2.1.0-rc.1` gives `2.1.0`, and a major bump on `2.0.0-rc.1` gives `2.0.0`. A bump that leaves the line the pre-release was heading toward still moves past it: a minor bump on `2.1.3-rc.1` gives `2.2.0`. This matches node-semver's `inc`, which changesets uses. Under the default `versioning = "zero-major"`, a major below 1.0.0 is still read as a minor first, so a major bump on `0.3.0-rc.1` gives `0.3.0`; under `versioning = "semver"` it gives `1.0.0`.
