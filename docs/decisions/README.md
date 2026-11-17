# Decisions

Architecture decision records: what was chosen, what was rejected, and why. Start a new one from [`0000-template.md`](0000-template.md).

| | Status |
|---|---|
| [0001 — Name the project oakum](0001-name-oakum.md) | accepted |
| [0002 — Stay a single crate until the first I/O dependency](0002-single-crate-until-io.md) | accepted |
| [0003 — Write only the files the invoked command owns](0003-write-only-what-a-command-owns.md) | accepted |
| [0004 — Derive facts from the repository; configure only preference](0004-derive-facts-configure-preference.md) | accepted |
| [0005 — Write only the changeset-format intersection](0005-write-the-changeset-format-intersection.md) | accepted |
| [0006 — Templates render; they do not execute](0006-no-command-execution-in-templates.md) | accepted |
| [0007 — Pin the tool version in config and refuse to run on mismatch](0007-pin-the-tool-version-in-config.md) | superseded in part by 0038 |
| [0008 — Cascade only along runtime edges](0008-cascade-only-along-runtime-edges.md) | accepted |
| [0009 — Delivery artifacts always cascade from a runtime dependency](0009-delivery-artifacts-always-cascade.md) | accepted |
| [0010 — Read the declared range as the cascade preference](0010-derive-cascade-from-declared-ranges.md) | accepted |
| [0011 — Stop at the tag; do not roll back](0011-stop-at-the-tag.md) | accepted |
| [0012 — Scope v0 to version math and the GitHub layer](0012-scope-v0-to-version-math-and-the-github-layer.md) | accepted |
| [0013 — Compile in the adapters; extend at the process boundary](0013-no-plugin-runtime.md) | accepted |
| [0014 — Read the current version from tags; write manifests as output](0014-tags-are-the-version-source-of-truth.md) | accepted |
| [0015 — Layer the pull-request status channels; gate on the exit code alone](0015-layer-the-pr-status-channels.md) | accepted |
| [0016 — Emit release state as data, render it as text, never deliver it](0016-emit-release-state-render-it-never-deliver-it.md) | accepted |
| [0017 — Ship an agent skill that teaches orchestration, not derivation](0017-ship-a-thin-agent-skill.md) | accepted |
| [0018 — Own the plan engine rather than depending on changesets](0018-own-the-plan-engine.md) | accepted |
| [0019 — Accept both change files and conventional commits, each fully disableable](0019-both-change-files-and-commits-each-disableable.md) | accepted |
| [0020 — Run one precondition path; `check` stops where `release` continues](0020-one-precondition-path.md) | accepted |
| [0021 — Distribute through crates.io, Homebrew, and npm, with npm as a fetcher](0021-distribute-through-three-channels.md) | accepted |
| [0022 — Default to zero-major versioning below 1.0.0](0022-zero-major-versioning.md) | accepted |
| [0023 — Name every verb and the files it owns](0023-name-every-verb-and-what-it-owns.md) | accepted |
| [0024 — Make the extracted `plan` crate `no_std` with `alloc`](0024-no-std-plan-crate.md) | accepted |
| [0025 — Support exactly one Rust version](0025-support-one-rust-version.md) | accepted |
| [0026 — Depend on `js-semver` for npm ranges; path-linked edges always cascade](0026-js-semver-and-path-linked-cascade.md) | accepted |
| [0027 — Version private packages only when opted in](0027-private-packages-version-opt-in.md) | accepted |
| [0028 — Express releaseless changes in normal bump files](0028-releaseless-bump-files-like-bumpy.md) | accepted |
| [0029 — Plan from one intent artifact at a time](0029-plan-from-one-intent-artifact.md) | accepted |
| [0030 — Derive read-side tag shapes; refuse leftover ambiguity](0030-derive-read-tag-shapes.md) | accepted |
| [0031 — Write generated markdown at the genre intersection](0031-write-generated-markdown-genre-intersection.md) | accepted |
| [0032 — Write a synthesized Changed line for a cascaded bump](0032-synthesize-cascade-changelog-line.md) | accepted |
| [0033 — Declare version writes outside a manifest](0033-declarative-extra-files.md) | accepted |
| [0034 — Exit 2 when a verification could not be made](0034-exit-two-for-unverified.md) | accepted |
| [0035 — Carry the finding when several looks refuse at once](0035-a-finding-outranks-an-unverified-look.md) | accepted |
| [0036 — Report the whole `check` run as data, not just its verdict](0036-report-the-whole-check-run-as-data.md) | accepted |
| [0037 — Exit 1 when a write set leaves the tree changed](0037-a-half-written-tree-is-a-finding.md) | accepted |
| [0038 — Read oakum's version from the install pin; drop `tool-version`](0038-the-install-pin-is-the-version.md) | accepted |
| [0039 — Derive what a change reaches, and what covers it](0039-derive-what-a-change-reaches-and-what-covers-it.md) | accepted |
| [0040 — Dependency updates need no intent, and release only on request](0040-dependency-updates-need-no-intent.md) | accepted |

0008 through 0010 are the reason this project exists. Read them together: 0008 decides which edges are eligible, 0010 decides when an eligible edge fires, and 0009 is the override that makes the whole thing correct for binaries.
