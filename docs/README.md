# Documentation

## Layout

| Directory | Holds | Numbered |
|---|---|---|
| [`decisions/`](decisions/) | Architecture decision records — what was chosen, what was rejected, and why | yes |
| [`specs/`](specs/) | The contract for a feature area: interface, behavior, edge cases | no |
| [`research/`](research/) | What was verified about external systems, with sources | no |
| [`ideas/`](ideas/) | Exploratory notes that are not decisions yet | yes |
| [`guide/`](guide/) | User-facing documentation | no |
| [`contributing/`](contributing/) | Agent and contributor process (indexed from root `AGENTS.md`) | no |

Each of the first four has a `0000-template.md`. Use it.

## Which one am I writing?

**A decision** when something was chosen over an alternative and the reasoning would otherwise be re-litigated. Status is `proposed` until it is actually agreed — a written ADR is not the same as a decided one.

**A spec** when a feature area has a contract others must code against. Specs cite the decisions that drive them.

**Research** when an external system was examined and the findings would be expensive to re-derive. Every claim carries its source: a file path, a command and its output, or a link. Findings whose sources have since changed are worse than no findings, so date everything.

**An idea** when it might matter and nothing has been settled. Messy is fine.

**A guide** when a user needs to do something. Guides describe behavior that exists; where it does not exist yet, say so at the top.

**Contributing** when the audience is someone changing this repository (human or agent): process, invariants, PR shape, beads. Not product behavior.

## Decisions

| | Status |
|---|---|
| [0001 — Name the project oakum](decisions/0001-name-oakum.md) | accepted |
| [0002 — Stay a single crate until the first I/O dependency](decisions/0002-single-crate-until-io.md) | accepted |
| [0003 — Write only the files the invoked command owns](decisions/0003-write-only-what-a-command-owns.md) | accepted |
| [0004 — Derive facts from the repository; configure only preference](decisions/0004-derive-facts-configure-preference.md) | accepted |
| [0005 — Write only the changeset-format intersection](decisions/0005-write-the-changeset-format-intersection.md) | accepted |
| [0006 — Templates render; they do not execute](decisions/0006-no-command-execution-in-templates.md) | accepted |
| [0007 — Pin the tool version in config and refuse to run on mismatch](decisions/0007-pin-the-tool-version-in-config.md) | accepted |
| [0008 — Cascade only along runtime edges](decisions/0008-cascade-only-along-runtime-edges.md) | accepted |
| [0009 — Delivery artifacts always cascade from a runtime dependency](decisions/0009-delivery-artifacts-always-cascade.md) | accepted |
| [0010 — Read the declared range as the cascade preference](decisions/0010-derive-cascade-from-declared-ranges.md) | accepted |
| [0011 — Stop at the tag; do not roll back](decisions/0011-stop-at-the-tag.md) | accepted |
| [0012 — Scope v0 to version math and the GitHub layer](decisions/0012-scope-v0-to-version-math-and-the-github-layer.md) | accepted |
| [0013 — Compile in the adapters; extend at the process boundary](decisions/0013-no-plugin-runtime.md) | accepted |
| [0014 — Read the current version from tags; write manifests as output](decisions/0014-tags-are-the-version-source-of-truth.md) | accepted |
| [0015 — Layer the pull-request status channels; gate on the exit code alone](decisions/0015-layer-the-pr-status-channels.md) | accepted |
| [0016 — Emit release state as data, render it as text, never deliver it](decisions/0016-emit-release-state-render-it-never-deliver-it.md) | accepted |
| [0017 — Ship an agent skill that teaches orchestration, not derivation](decisions/0017-ship-a-thin-agent-skill.md) | accepted |
| [0018 — Own the plan engine rather than depending on changesets](decisions/0018-own-the-plan-engine.md) | accepted |
| [0019 — Accept both change files and conventional commits, each fully disableable](decisions/0019-both-change-files-and-commits-each-disableable.md) | accepted |
| [0020 — Run one precondition path; `check` stops where `release` continues](decisions/0020-one-precondition-path.md) | accepted |
| [0021 — Distribute through crates.io, Homebrew, and npm, with npm as a fetcher](decisions/0021-distribute-through-three-channels.md) | accepted |
| [0022 — Default to zero-major versioning below 1.0.0](decisions/0022-zero-major-versioning.md) | accepted |
| [0023 — Name every verb and the files it owns](decisions/0023-name-every-verb-and-what-it-owns.md) | accepted |
| [0024 — Make the extracted `plan` crate `no_std` with `alloc`](decisions/0024-no-std-plan-crate.md) | accepted |
| [0025 — Support exactly one Rust version](decisions/0025-support-one-rust-version.md) | accepted |
| [0026 — Depend on `js-semver` for npm ranges; path-linked edges always cascade](decisions/0026-js-semver-and-path-linked-cascade.md) | accepted |

0008 through 0010 are the reason this project exists. Read them together: 0008 decides which edges are eligible, 0010 decides when an eligible edge fires, and 0009 is the override that makes the whole thing correct for binaries.

## Research

Each carries its own date and sources; the first eight came out of the design work that preceded the first commit, and the rest were written as questions arose.

- [Changeset file format](research/changeset-file-format.md) — what the JS and knope parsers each tolerate, and the narrow subset both accept
- [Workspace discovery](research/workspace-discovery.md) — asking the package manager, and the ancestor `pnpm-workspace.yaml` that silently returns the wrong packages
- [Registry publish semantics](research/registry-publish-semantics.md) — npm and crates.io error shapes, stale reads, and how seven tools handle a publish that fails halfway
- [Downstream handoff](research/downstream-handoff.md) — whether a tag-to-workflow handoff can be verified, or should be removed
- [Tool version pinning](research/tool-version-pinning.md) — how eight tools stop their own behavior changing without a commit
- [Templating prior art](research/templating-prior-art.md) — how release text is customized elsewhere, and why a template must not execute
- [Implementation stack](research/implementation-stack.md) — which crates rewrite a hand-formatted manifest without damaging it, and where they still bite
- [GitHub's release path](research/github-release-path.md) — the four ways a tag push silently triggers nothing
- [Bump-file tool interfaces](research/bump-file-tool-interfaces.md) — bumpy's CLI surface and three-phase propagation, as the primary reference for oakum's own commands
- [cargo-dist's npm installer](research/cargo-dist-npm-installer.md) — what the npm package actually contains, and why "fetcher" is right but "no JavaScript" is not
- [Changelog lint collision](research/changelog-lint-collision.md) — a generated changelog failing the repository's own linter, and the two formatters that disagree with each other
- [Version-format constraints](research/version-format-constraints.md) — which version strings survive npm, Cargo, and git unchanged; pnpm strips build metadata without saying so, and crates.io preserves it
- [no-std plan feasibility](research/no-std-plan-feasibility.md) — what a crate boundary restricts, what `no_std` restricts, and why neither is sufficient alone
- [Renovate rule matching](research/renovate-rule-matching.md) — why the rule keeping the Rust pin out of automerge fires, and the two ways a copied rule matches nothing without erroring
- [cargo metadata edge shapes](research/cargo-metadata-edge-shapes.md) — which fields distinguish two dependency entries onto the same package, and which manifest constructs are erased before oakum sees them
- [npm ranges versus Cargo's VersionReq](research/npm-range-vs-cargo-versionreq.md) — which npm forms `semver::VersionReq` rejects or misreads, and why path-only edges need a bounds-free arm

## Specs

- [Bump files](specs/bump-files.md) — draft
- [init](specs/init.md) — draft
- [migrate](specs/migrate.md) — draft

## Ideas

Not decisions. Each names what would have to be answered before it became one.

- [0001 — Declarative version writes outside a manifest](ideas/0001-declarative-extra-files.md)
- [0002 — An agent skill that teaches orchestration only](ideas/0002-agent-skill.md) — promoted to [ADR-0017](decisions/0017-ship-a-thin-agent-skill.md)
- [0003 — `check` as a pre-push hook](ideas/0003-check-as-a-git-hook.md)
- [0004 — Tags as the version source of truth](ideas/0004-tags-as-the-version-source-of-truth.md) — promoted to [ADR-0014](decisions/0014-tags-are-the-version-source-of-truth.md); kept for its prerelease-channel notes
- [0005 — Structured release state](ideas/0005-structured-release-state.md) — promoted to [ADR-0016](decisions/0016-emit-release-state-render-it-never-deliver-it.md)
- [0006 — Upstream experiment and abort contingency](ideas/0006-build-versus-contribute.md) — build is decided (ADR-0012/0018); keeps the abort path and an unrun bumpy upstream experiment
- [0007 — Maintenance release branches](ideas/0007-maintenance-release-branches.md) — the release-train workflow from work, out of v0 scope
- [0008 — Custom version formats](ideas/0008-custom-version-formats.md) — epoch semver and build metadata, neither settled

0004 through 0006 were recovered from the 2026-08-18 session transcript after nearly being lost — none of the three appears in the design-decisions record. Two of them turned out to be settled decisions that the recovery pass mis-filed as open recommendations, and they now carry the ADRs above. [0006](ideas/0006-build-versus-contribute.md) is no longer an open "build or not" question: it holds the abort contingency and an upstream experiment. [0002](ideas/0002-agent-skill.md) was promoted alongside them, but it predates the recovery and was never lost.

## Guide

- [Writing bump files](guide/bump-files.md)
- [Running oakum in GitHub Actions](guide/github-actions.md)
