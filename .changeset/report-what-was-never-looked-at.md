---
oakum: major
---

### Changed

`check` refuses a bump file that names a package it cannot version, matching what `status` and `version` already did. It asked a narrower question than the pipeline — it let an `include`/`exclude` exclusion through — so a repository could pass a green `check` and then have its version PR fail on the same file. If your `check` is green today on a bump file naming an excluded package, it will now fail, and so would the release that followed it. The refusal names every offending package and prints before returning, so a failure in an earlier look, such as a missing install pin, no longer erases it.

The coverage look refuses a shallow clone instead of reporting a clean result. At depth 1 the default base resolves to the checked-out commit itself, so the diff came back empty and every changed package looked covered. `actions/checkout` clones that way unless told otherwise, which made this the common CI shape rather than an edge case. `check` already refused such a clone through its tag look; `ci pr-status` did not, and now does, naming `fetch-depth: 0`. `status` reports it rather than refusing, because `status` is not a gate.

`check` stays silent on a changed package the config keeps but cannot version — an unpublishable package where `private-packages.version` is not set. That is a choice someone made, not a fault, and gating on it would turn every private-package change red until they wrote config; [ADR-0027](https://github.com/oakoss/oakum/blob/main/docs/decisions/0027-private-packages-version-opt-in.md) records that silence as something a changesets migratee keeps without a config change. `status` reports it instead — see the Added note.

`migrate` no longer calls an empty plan comparison a match. With nothing pending on either side it printed `0 package(s) planned by bumpy and by oakum; match`, which reads as verification of a transform that nothing exercised. That is the common case rather than the rare one, since a repository is usually migrated right after a release.
