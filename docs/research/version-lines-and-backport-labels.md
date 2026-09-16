# Version lines and backport labels: what the big projects actually write down

- Date: 2026-09-16
- Author: Jace Babin
- Scope: how SemVer, Cargo, Rust, Go, Node.js, CPython, Kubernetes, Git, Changesets, Keep a Changelog and Conventional Commits (a) tell a patch from a minor, (b) label an issue by target version or line, and (c) decide when a fix reaches a released patch line rather than the next minor. Not how any of them number a release.

## Question

Oakum's backlog now carries `0.4.0`, `0.5.0` and `patch` labels, and no written rule for who gets which, or for when a `patch` bead becomes a `0.3.x` release rather than a line in 0.4.0's Fixed section. Before writing that rule into `contributing/`, what do projects with a written policy do, and what does SemVer itself require below 1.0.0?

## Sources

All fetched 2026-09-16. Quotes are verbatim.

- SemVer 2.0.0: <https://semver.org/spec/v2.0.0.html>
- Cargo SemVer compatibility: <https://doc.rust-lang.org/cargo/reference/semver.html>
- Rust forge, backporting and issue triaging: <https://forge.rust-lang.org/release/backporting.html>, <https://forge.rust-lang.org/release/issue-triaging.html>; rustc-dev-guide diagnostics: <https://rustc-dev-guide.rust-lang.org/diagnostics.html>
- Go minor releases: <https://go.dev/wiki/MinorReleases>; Go 1 compatibility: <https://go.dev/doc/go1compat>
- Node.js `doc/contributing/backporting-to-release-lines.md`, `collaborator-guide.md`, `using-internal-errors.md` on `main`; `nodejs/Release` README on `main`
- CPython devguide development cycle and labels: <https://devguide.python.org/developer-workflow/development-cycle/>, <https://devguide.python.org/triage/labels/>; PEP 387: <https://peps.python.org/pep-0387/>
- Kubernetes `sig-release/cherry-picks.md`, `sig-release/release.md` (kubernetes/community, `master`); `label_sync/labels.md` (kubernetes/test-infra, `master`)
- Git `Documentation/howto/maintain-git.adoc` on `master` (the `.txt` path is gone)
- Changesets `docs/command-line-options.md`, `docs/decisions.md`, `packages/types/src/index.ts` on `main`
- Keep a Changelog 1.1.0: <https://keepachangelog.com/en/1.1.0/>
- Conventional Commits 1.0.0: <https://www.conventionalcommits.org/en/v1.0.0/>

## What the spec requires below 1.0.0: nothing

SemVer's patch and minor rules are scoped to a non-zero major. §6: "Patch version Z (x.y.Z | x > 0) MUST be incremented if only backward compatible bug fixes are introduced." §7: "Minor version Y (x.Y.z | x > 0) MUST be incremented if new, backward compatible functionality is introduced to the public API." §4: "Major version zero (0.y.z) is for initial development. Anything MAY change at any time."

Cargo makes the 0.x reading concrete: "Initial development releases starting with "0.y.z" can treat changes in "y" as a major release, and "z" as a minor release." So for a 0.x crate the compatible slot is `z` and Cargo draws no line inside it. For a binary it declines to draw any line at all: "The potential breaking and compatible changes to an application are too numerous to list, so you are encouraged to use the spirit of the SemVer spec … or at least document what your commitments are."

Conventional Commits ignores the phase: its 0.x FAQ says "We recommend that you proceed as if you've already released the product." Changesets declines to define anything finer: "We use semver for specifying the change. When selecting the kind of change your package is, we do not specify any change types beyond `major`, `minor`, or `patch`." Its frontmatter does carry a fourth value, `export type VersionType = "major" | "minor" | "patch" | "none"`, which is why ADR-0028 could adopt `name: none` unchanged; `--empty` is the no-package no-op, "usually only required if you have CI that blocks merges without a changeset." Keep a Changelog ties no section to a bump level.

**Settled:** oakum's patch-versus-minor line at 0.x is a self-imposed contract. ADR-0022 already makes `0.y` the breaking-or-feature slot; what a *patch* may contain has to be written by us, and Cargo says as much for a binary.

## Is a changed message or a new warning a patch?

The one place with a written answer each way:

- Cargo, for libraries: "Minor: introducing new lints … This should generally be considered a compatible change", with one caveat: "Beware that it may be possible for this to technically cause a project to fail if they have explicitly denied the warning, and the updated crate is a direct dependency."
- Node.js: "changes to error messages result in broken code in the ecosystem. For that reason, Node.js has considered error message changes to be breaking changes." The collaborator guide lists "Adding or removing errors" as breaking, and "Changing error messages for errors without error code" and "Emitting a runtime warning" as breaking changes that are merely exempt from the deprecation cycle.

Rust (forge, rustc-dev-guide diagnostics page), Go (`go1compat`), CPython (devguide, PEP 387) and Kubernetes do not address message text.

**Settled:** the two written answers disagree, and the disagreement is about what carries the contract. Node's errors carry per-error codes (`ERR_*`), so the text is free only where a code identifies the error; without one, text is the contract. Oakum's exit code identifies only the outcome class (ADR-0034: "A per-variant code was rejected as precision nobody asked for"), so a caller telling tag drift from an uncovered package has no code to key on. The `status --json` document (ADR-0016) names the plan and the coverage findings, not every `check` finding: ADR-0035 records that in a mixed run "the unverified outcome leaves the machine-readable channel and survives only in prose", and `okm-404.58` is the channel that would carry it. No decision record promises stderr wording. So "a new stderr line is patch" rests on two things being unchanged, the exit code and that document, with the gap ADR-0035 names left open until 404.58 closes it.

## When a fix reaches a released line

Go, Rust and Kubernetes start from *no*; CPython and Node.js start from risk class:

- Go: "Our default decision should always be to not backport, but fixes for **security issues**, **serious problems with no workaround**, and **documentation fixes** are backported … A "serious" problem is one that prevents a program from working at all."
- Rust: "Backports of PRs to the beta branch are usually only done to fix regressions." Whether a stable backport becomes a release is separate: "`T-release` will decide on a case by case basis if a stable backport will warrant a point (.patch) release".
- Kubernetes: "Only the following types of changes are expected to be backported: Security fixes; Regression fixes; Critical bug fixes (loss of data, memory corruption, panic, crash, hang); Prerequisite changes for critical dependency updates; Test-only changes to stabilize failing / flaky tests". "A fix for an issue that only occurs when an off-by-default alpha feature is enabled does not qualify as a critical bug fix and is **not** eligible for backport."
- CPython: "*Low-risk* changes (bug fixes, test improvements, and documentation edits) may be backported without debate. *Higher-risk* changes (new features, semantic changes, and performance improvements) … are not backported as a matter of course." Feature requests "do not need version labels; it is implicit that features are added to the `main` branch only."
- Node.js: Current "Should incorporate most of the non-major (non-breaking) changes"; Maintenance lines take "Critical bug fixes and security updates"; LTS lines "require commits to mature in the Current release for at least two weeks before backporting."
- Git: "Maintenance releases are numbered as vX.Y.Z (0 < Z) and are meant to contain only bugfixes for the corresponding vX.Y.0 feature release". Bugfixes land on `maint` and merge upward; when a point release is cut is "at some point".

**Settled:** no project keys the decision on size. The core every policy shares is regression, security, or a defect that stops the program with no workaround, judged against the released line; Go, Kubernetes and CPython also admit documentation, test-stabilization or other low-risk fixes. CPython states outright that features are not backported, and the other lists have no entry a feature could fit. Rust and Go record cutting the point release as a decision separate from accepting the fix, made by whoever owns the release.

## How the target line is recorded

Three shapes, from lightest to heaviest:

1. **A routing label on the change** (CPython): `needs backport to 3.N` "used to indicate which branches the PR should be backported to. Once the PR is merged, `miss-islington` will automatically attempt to create backport PRs". Features carry no version label.
2. **A nomination-then-acceptance pair** (Rust, Go, Kubernetes): `beta-nominated` "needs attention from the appropriate team to decide", `beta-accepted` once "the team thinks it should be backported"; Go's `CherryPickCandidate` → `CherryPickApproved` on a child issue titled "package: title [1.17 backport]"; Kubernetes' cherry-pick PR "will immediately get the `do-not-merge/cherry-pick-not-approved` label" until a release manager approves.
3. **A per-line state machine** (Node.js): `backport-requested-vN.x` → `backport-open-vN.x` → `backported-to-vN.x`, with `backport-blocked-vN.x` and `dont-land-on-vN.x` for the two ways out.

Regressions get their own class everywhere it is tracked: Rust's `regression-from-stable-to-{stable,beta,nightly}` and `regression-untriaged`; Kubernetes' `kind/regression` ("related to a regression from a prior release").

**Settled:** in every shape quoted, the label names a *line* (`3.N`, `vN.x`, `1.17`), not a future patch number, and the labels shown exist for routing a backport rather than for planning. Rust, Go and Kubernetes separate nomination from acceptance; CPython's routing label carries no acceptance state. Where a regression class exists (Rust, Kubernetes) it is a label of its own.

## What this settles for oakum

- The patch/minor rule at 0.x is ours to write, and the honest place for it is beside ADR-0022. The proposed contract surface is the exit code, which classifies the outcome (ADR-0034), and the `status --json` document, which names the findings (ADR-0016); no decision record promises stderr wording. On that basis a prose-only diagnostic that leaves behavior, the exit outcome and the structured output unchanged is patch: Cargo's reading rather than Node's, because oakum's outcome identity lives in the exit class and the document rather than in message text, with the findings ADR-0035 leaves in prose as the known gap. A diagnostic that accompanies a new refusal or a changed document is classified by those, not by stderr.
- A `patch` bead is a class, not a plan. It ships in whatever releases next unless it meets the proposed backport bar, the shared core above: a regression from the last tag, a security issue, or a defect with no workaround. Features and new refusals do not backport.
- A line label (`0.3.x`, CPython's routing shape) would be the lightest way to assemble a point release. **Not adopted:** the rules in [contributing/task-tracking.md](../contributing/task-tracking.md#merging-releases-and-hotfixes) keep one line in flight, ship a minor early rather than branch off a tag, and mark a regression from the last tag on the bead so a hotfix is one query without a line label.
- Cutting the point release is a decision separate from merging the fix, as Rust and Go record it; here that is merging the version PR after the hotfix branch.
