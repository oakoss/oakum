# Changelog

All notable changes to the `review-cycle` plugin will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).




## 0.15.0
<sub>2026-08-19</sub>

- [#25](https://github.com/oakoss/claude-plugins/pull/25)  *(minor)*
  `review-sentinel mark` now refuses with exit 3 unless a review cycle is actually running, and the unguarded write moves to a new `accept-state` verb. The old shared verb let any agent declare its own work reviewed: `mark` in one Bash call and `git commit` in the next sailed through, because the PreToolUse gate exits on any command containing no `git commit` and so never saw the pair. `mark` now requires `.claude/.review-in-progress`, which only `cycle-start` writes and only a real cycle runs. The test is presence rather than freshness, but the Stop gate deletes markers past its 60-minute TTL, so a cycle that outruns it reaches Phase 8 with no marker and exits 3; the summary should say so rather than the cycle re-running `cycle-start` to recreate its own evidence. The commit gate's chained pass-through accepts `accept-state && git commit` only — the guarded verb cannot ride the one path that skips the sentinel check. `/review-cycle:review-pr` gets its own marker via new `pr-cycle-start` and `pr-cycle-end` verbs: it reviews a PR head in a throwaway worktree and never reads the working tree, so its marker holds the Stop gate open without licensing a `mark` over local changes nothing reviewed. Re-run `/review-cycle:init`, or add `.claude/.review-pr-in-progress` to `.gitignore` yourself.

  SessionStart now revokes an in-progress marker once it passes the same 60-minute TTL the Stop gate applies, on every startup path including the legacy-sentinel migration. Without that, a session that died mid-review left a marker that licensed the next `mark` over exactly the unreviewed work the gate exists for — the re-seed that used to clear it is skipped in that case by design. The revocation is deliberately stale-only, and `seed` no longer retires the marker at all: `startup` also fires when a second session opens in a repo where the first is mid-cycle, and deleting a live cycle's fresh marker would strand its Phase 8. Retiring a marker now belongs to the verbs that conclude a cycle — `mark`, `accept-state`, and `cycle-end` — plus the two TTL owners, the Stop gate and SessionStart.

  What you do differently: `/review-cycle:accept` and any script that wrote the sentinel by hand must call `accept-state` instead of `mark`. `/review-cycle:review` is unchanged, since Phase 3 already runs `cycle-start`. This raises the bar rather than closing it. `cycle-start` is itself unguarded, so `cycle-start` followed by `mark` still clears the gate, as does `accept-state`. What changes is that the shortest path no longer looks like routine plumbing: `accept-state` names itself in a transcript, and a cycle that declares itself started and then marks without reviewing is a claim someone can check.

## 0.14.0

<sub>2026-08-18</sub>

- [#23](https://github.com/oakoss/claude-plugins/pull/23)  *(minor)*
  The Codex review leg now receives the intent brief. `codex review`'s scope flags reject a prompt argument, so the brief is passed as a config override (`-c developer_instructions="<brief>"`) — verified additive to `AGENTS.md` on Codex v0.147.0, with no file written into the review target. Codex reviews now know what the change is trying to accomplish, closing the gap where the CLI leg reviewed blind while every Claude-side reviewer got the brief. On a Codex version that drops the undocumented key, the leg degrades to an unbriefed review instead of failing.
- [#23](https://github.com/oakoss/claude-plugins/pull/23)  *(minor)*
  New `/review-cycle:review-pr` skill: single-pass, report-only review of a GitHub pull request, run locally. It fetches the PR head into a disposable detached worktree — the working checkout is never touched — runs the same reviewer fan-out as the review cycle (Codex leg included, briefed, with `--base` against the PR's base branch), and reports findings with explicit per-reviewer coverage so "no findings" is never mistaken for "nobody looked". On request it posts the findings back to the PR as a single COMMENT review whose comments — inline and body-level alike — carry fingerprints that deduplicate across re-runs. It never fixes code, never approves, and never touches the review sentinel.

## 0.13.0

<sub>2026-08-18</sub>

- [#15](https://github.com/oakoss/claude-plugins/pull/15)  *(minor)*
  Aligned the bundled de-slopify skill with the new `prose` plugin's Google-developer-style rules, so cycle cleanup and ad-hoc prose cleanup apply the same standard. The quick reference gains wordiness swaps ("in order to" → "to", "leverage" → "use", "enables you to" → "lets you"), instruction mechanics (active voice, present tense, condition before instruction, second person), and timeless-docs rules ("currently"/"soon" → delete). Claims and punctuation rules join the quick reference too: superlatives and "ensures"/"guarantees" become verifiable claims or "helps", "and/or" and slashed alternatives are spelled out, dramatic ellipses go, and "see" replaces "check out" for links. Generation-stable structural tells round it out — copula avoidance ("serves as" → "is"), sentence-final "-ing" significance trailers, and Claude-era phrases ("You're absolutely right") — with a note that vocabulary tells rotate by model generation while structural patterns persist. The don't-overcorrect floor grows: keep function words like "that" and "then" when they aid parsing, keep articles even in headings, and kept emdashes take no surrounding spaces. Details live in a new "Wordiness and Instruction Patterns" section of the prose-patterns reference.

## [Unreleased]

## [0.12.0] - 2026-08-17

Makes reviewers prove their claims instead of inferring them.

### Added

- **Reviewers are instructed to verify empirically.** Five of the reviewer agents (code-reviewer, silent-failure-hunter, pr-test-analyzer, type-design-analyzer, spec-conformance-analyzer) now carry a "Verify empirically" section: when a finding rests on a claim a command can settle — an exit code, a build output, whether a test actually covers a path, whether the typechecker actually rejects an invalid construction — run the command and report what happened rather than reasoning about what should happen. Findings are labeled verified (with what was run) or inferred from reading, and verification must never disturb the review target — repros and probe files go in a disposable directory. The two exclusions are deliberate: cleanup originates no findings, and maintainability-auditor's structural suggestions are speculative by design — a probe adds cost without changing a verdict already labeled speculative. Field experience drove this: the findings that mattered came from agents that ran things, and this plugin's own release-breaking `codex login status` bug was caught only by an empirical probe no prompt had asked for.
- **A local eval suite** (`evals/`, run with `claude plugin eval` — see `evals/README.md`). Two cases cover the 0.11.0 behavior that only exists as skill prose: a codex shim exiting 127 must yield a completed Claude-only cycle whose summary names `skipped (not installed)` and never launches `codex review`; a working shim plus a 2-line diff must produce a `codex review` invocation carrying `-c model_reasoning_effort="low"` and a summary reporting `participated (effort: low)`. Shims keep eval runs off any real Codex account. Local-only for now — CI has no `claude` CLI on the runner.
- **Phase 6 verifies facts introduced by fixes, not just the fixes themselves.** The self-check confirmed a finding was addressed but never that new prose was true, so a fix could resolve a finding by asserting something false — surviving until the next fan-out caught it, or shipping. A fix that adds or rewords a factual claim now gets the claim itself checked (run the command it describes, read the code it characterizes, confirm the name or version it cites) before the loop proceeds.

## [0.11.0] - 2026-08-17

Makes the Codex review leg optional, so the cycle runs anywhere Claude Code does, and scales its review depth to the diff.

### Added

- **Codex review depth now follows the diff tier.** Every review ran at the user's globally configured reasoning effort, so a two-line `.gitignore` fix cost the same as a large refactor. Light-tier diffs now pass `-c model_reasoning_effort="low"` when that is a reduction — a config already at `low`, `minimal`, or `none` is left alone; full-tier diffs pass no override and inherit the user's `~/.codex/config.toml`. With `multi_agent = true` the setting reaches Codex's internal review agents, so the reduction compounds across them. The final summary reports which applied.
- The adjustment is **one-directional**: the tier can lower effort but never raise it. A globally configured `medium` is a deliberate choice, and a large diff is not grounds to overrule it.
- Effort, not model name, is the tuning axis: `codex review` exposes neither `--model` nor `--profile` (only `-c key=value`), model names churn often enough that Codex maintains a `[notice.model_migrations]` table, and a name pinned in the skill would rot into an error or a silent downgrade on another user's account. The skill emits the literal `low` and never interpolates a value — Codex does not validate `-c` values locally, so a bad one would surface only as an API failure mid-review.

### Changed

- **Codex is now an optional review leg rather than a hard prerequisite.** Phase 1 probed `codex --version` and stopped the entire cycle when it failed, which made the plugin unusable on any machine or CI runner without an authenticated Codex — including the PR-review contexts it is otherwise well suited to. The probe now records the leg's status and the cycle continues Claude-only when Codex is absent.
- **The Codex leg's status is always named in the final summary** (`Codex leg: participated | skipped (…) | failed (…)`). Degraded coverage stays visible instead of silently halving the review.
- **Absence and mid-run failure are distinguished.** A Codex that never passed the preflight probe is a `skipped` environment, not an error. A Codex that passed the probe and then exited nonzero or returned unusable output during `codex review` is reported as `failed` — the cycle still completes on the subagent findings, but the breakage is surfaced as a regression to look at.
- **`/review-cycle:init` reports Codex as an available upgrade, not a warning.** A missing CLI prints what installing it would add and marks the multi_agent and auth steps not-applicable rather than flagging them.
- **Auth is no longer a gate on the Codex leg.** Only `codex --version` decides participation. `codex login status` reports on a *stored* session only, so it exits nonzero with `Not logged in` whenever Codex is authenticated by environment variable (`OPENAI_API_KEY`) with no login on disk — the standard CI arrangement, and precisely the environment an optional Codex leg exists to serve. It now serves only to enrich a later failure message (`failed (… — no stored session, try codex login)`), and `skipped (not authenticated)` is gone as a status. Caught by this release's own dogfood run.

### Fixed

- **The Codex leg's status is read from the completion notification**, which carries the shell's exit code, rather than inferred from the output file — where a crashed run and a clean run both look like a file with no findings in it. `participated` is also now clearly an outcome recorded after the run, distinct from Phase 1's `eligible` precondition; the two shared a name, which made the launch gate read as depending on its own result.
- **`codex --version` failing no longer always means "not installed".** Exit 127 does; a broken install, a non-executable binary, or a `$PATH` the tool shell can't see does not, and telling those users to `npm install -g` something already installed helps nobody. The probe now classifies on exit code and quotes the actual stderr.
- **Auth state is three-valued and written down when observed.** `no stored session` and `unknown (probe unsupported)` were collapsed, so a user whose CLI simply lacks `login status` was sent to run `codex login` to fix something that wasn't broken. The state is also recorded into the summary draft at Phase 1 rather than recalled at Phase 9 — the loop ends turns and re-wakes across up to four iterations, and it isn't re-derivable later.
- **The light-tier effort lookup reads the root table only.** A plain grep also matched `model_reasoning_effort` under `[profiles.*]`, so a root `minimal` beside an unused profile `medium` read as `medium` and got "lowered" to `low` — raising the user's actual effort, the exact thing one-directionality forbids.
- **A failed Codex leg retries once, not every iteration**, and any iteration's failure sticks in the summary instead of being overwritten by a later success — the iteration Codex missed is usually the one that had the findings.
- **The stall watchdog no longer drops a reviewer on an unrelated wake.** "Hasn't reported by the next wake" counted any wake, including one triggered by other agents milliseconds later, so a nudged reviewer could be discarded while mid-reply. A wake now has to carry information about that reviewer — it idled again, or every other reviewer has since reported.
- **The watchdog's set-relative drop test no longer applies on the light tier.** "Every other reviewer has since reported" is vacuously true when `code-reviewer` is the only Claude-side reviewer, so the light tier could nudge and drop its sole reviewer on any wake — losing the entire Claude side of the review on the tier least able to afford it. Only that reviewer's own notification counts there.
- **Review subagents must be spawned unnamed, in both fan-outs.** Passing `name:` turns a background agent into a persistent addressable teammate that parks as `idle` awaiting messages instead of completing and returning its report, so its findings never arrive and the watchdog spends its one nudge on an agent that was never going to deliver. The rule is now global rather than stated only in the Phase 3 fan-out — Phase 7 spawns agents too, and has no watchdog to notice. Found the hard way: the dogfood run lost all three Claude-side reviewers to this.
- **Phase 2's canonicalization notes have somewhere to land.** A missing formatter or an erroring typecheck was to be "noted", but the summary had no field for it, so "the typecheck tool was missing" and "the typecheck passed" looked the same. The summary template now carries a `Canonicalization:` line, and an abort before Phase 8 prints an abbreviated status instead of reporting nothing at all.

## [0.10.0] - 2026-08-17

Cuts the cycle's stall-and-ceremony overhead — the wall-clock cost that field feedback measured at roughly half of each cycle — without dropping any finding-catching mechanism.

### Added

- **The Stop gate no longer blocks while a review cycle is running.** The cycle writes `.claude/.review-in-progress` at fan-out (new `review-sentinel cycle-start`/`cycle-end` subcommands); while the marker is fresh (under 60 minutes) the Stop hook lets turns end, so the agent awaits background reviewers via completion notifications instead of sleep-loop workarounds. `mark`/`seed` clear the marker; the Stop gate removes stale copies from crashed cycles.
- **Stall watchdog for background reviewers.** A reviewer that goes idle without delivering gets one nudge, then the cycle proceeds without it and names it under "reviewers dropped (stalled)" in the summary.
- **Diff tiering.** Phase 1 classifies the diff once: light diffs — docs-only, or ~25 changed lines or fewer regardless of file type — get Codex + code-reviewer with a 2-iteration default cap and inline cleanup, so a two-line `.gitignore` fix no longer triggers the full apparatus; everything else keeps the full conditional fan-out (default cap 4). The tier is named in the final summary.
- **Reviewers receive an intent brief.** The fan-out prompts were context-free ("review the uncommitted changes"), leaving every reviewer to guess what the change was trying to do — a major source of beside-the-point findings. Phase 3 now composes a 2–4 sentence intent summary plus the changed-file list, passed to each subagent. Codex runs unbriefed: its scope flags reject a prompt argument (found by this release's own dogfood run). Intent only, never expected verdicts.
- **`review-sentinel status`: a diagnostic that answers "why is the gate blocking me".** Prints the resolved root, clean-tree verdict, the Stop-gate markers when present (in-progress age vs TTL, block-once record), stored mark (anchor + hash), the current hash computed from the stored anchor, and a one-line verdict; exit codes mirror `check`. Field feedback showed users bisecting gate behavior with `check` vs `match` (which disagree by design — the clean-tree fast-path) and drawing wrong conclusions; one subcommand now shows the whole picture.

### Changed

- **The Stop gate blocks once per drift state.** It records the state hash it blocked on (`.claude/.review-stop-block`); a later stop on the identical state soft-passes with a warning instead of re-blocking, so a user-directed "keep going, review at the end" batches work naturally. This relaxes only *when* review happens — the commit gate still prevents unreviewed commits, unchanged.
- **Fix verification is a self-check, not an automatic re-fan-out.** After applying fixes, the cycle verifies each against the findings list; a fresh reviewer iteration runs only when some fix was substantive (new logic, edits beyond the flagged lines), and then scoped to Codex plus the reviewers whose domain those fixes touched. Iterations that used to exist purely to confirm mechanical fixes landed are gone.
- **Cleanup only spawns an agent when it pays for itself.** Purely size-based: diffs under ~150 changed lines get the comment-policy and de-slopify pass inline; the cleanup subagent is reserved for larger diffs, whatever the tier — a large docs-only diff is light-tier for fan-out but is exactly the prose volume the agent spawn is for.
- Both new marker files are excluded from the sentinel hash and clean-tree check, and `review-sentinel paths` (which `/review-cycle:init` feeds into `.gitignore`) now lists them. Existing installs: re-run `/review-cycle:init` once so the new marker paths land in your project `.gitignore`.
- **The comment-slop hook now demands the fix instead of suggesting it, and catches narration by arithmetic.** The PostToolUse hook's context was "consider removing or rewriting" — hedged phrasing models shrug off, which is why comment cleanup kept needing repeat requests. It now directs an immediate follow-up Edit, including compressing kept WHY-comments to a line or two. It also gained a comment-density check on the exact text just written (4+ comment lines and ≥30% of the edit): narrating WHAT-comments mostly dodge the pattern greps, but they can't dodge the ratio. The count is careful about what a comment is — C dereferences (`*p = 1;`), `#include` directives, `#[attributes]`, and shebangs don't count, a Write payload's leading header block is exempt (a new file's legitimate header is not an "edit"), and the history-flavored grep is case-insensitive. Comment-carried config formats (`.yml`, `.toml`, …) are exempt from the density check, and prose files (`.md`, `.txt`, …) now skip the hook entirely — `#` is a heading there, and the pattern greps would false-positive on lines like a `# Note:` heading; prose belongs to the de-slopify/cleanup lens. The hook now has its own bats suite (26 tests) covering the pattern greps, the density thresholds across all three tool payload shapes, and every silent exit path.
- **History-flavored comments are flagged and banned by policy.** Comments narrating a prior state of the code ("previously", "no longer", "as it did while X") accrete during review passes — each pass explains the previous one, so reviewed comments grow instead of shrink. The hook greps for them, and the comment policy (skill, cleanup agent, reference snippet) now names the pattern: state the current invariant, put the story in the commit message. The review skill additionally forbids a review pass from ever lengthening a comment.
- **The cleanup agent inherits the session model** instead of pinning sonnet — it now only spawns on large diffs, where the stronger model's comment judgment is worth the cost.

### Fixed

- **Gate rejection messages no longer imply the review didn't happen.** Both the PreToolUse deny and the installed git hook said "run /review-cycle:review first" for every drift — misdirecting when a review *had* happened and the state drifted afterward (commit-time formatter, hook manager restoring the index after a rejection, edits since the mark). The messages now distinguish the two cases, name the common causes, note that a hook-manager rejection may have unstaged files, and point at `review-sentinel status`. A new README troubleshooting entry covers the same ground, including what is *not* the cause: staging order and multi-commit batches are both invariant under the hash design (re-verified against clean-room repros this release).
- **Codex now reviews the same diff as the subagents on scoped runs.** `against <ref>` scoped the subagents to `<ref>..HEAD` but Codex was still invoked with `--uncommitted`; the skill now uses `codex review --base <ref>` when a base is given.

## [0.9.0] - 2026-08-03

### Added

- **`review-sentinel install-hook`: opt-in commit-time enforcement.** Installs a git pre-commit hook that runs `review-sentinel check` inside the commit — closing the two structural blind spots of PreToolUse-time evaluation: chains that re-drift the tree after the check (`sed -i … && git commit -a`), and agent commits from outside the session's Bash tool. Only agent sessions are gated (`CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT` env guard — humans are never blocked); the kill-switch and per-project opt-outs are honored with the same precedence as the PreToolUse gate; a missing binary or check error fails open. Manager-aware installation: lefthook repos get a guarded helper script plus a lefthook job (config auto-edited only when it has no `pre-commit` key, append failures reported), pre-commit and simple-git-hooks repos get the helper plus a printed snippet (committed config never auto-edited), husky/`core.hooksPath` setups are handled implicitly via `git rev-parse --git-path hooks`. Under plain git a pre-existing hook is relocated to `pre-commit.local` and chained first with its exit status preserved — appending would swallow its failures or sit dead behind an `exit 0` — while a git-tracked hook file (husky commits `.husky/`) is never rewritten: helper plus snippet instead, so no machine-specific path lands in committed files. The embedded binary path is quote-escaped so unusual install paths can't break the hook. `review-sentinel uninstall-hook` reverses everything, restoring a relocated hook. Worktree commits are covered. The PreToolUse gate stays active as the zero-setup default everywhere; the git hook is added depth, never a dependency. New `install-hook.bats` suite (41 tests).
- **SessionStart notes missing commit-time enforcement, context-only.** When the gate is active and no commit-time hook is installed, session-init emits one line into model context describing `install-hook` — explicitly marked not to be suggested unprompted.

### Changed

- **Lexical analysis extracted to `hooks/lib/command-parse.sh`.** The commit-detection regex, commit counting, sanctioned-mark-chain decision, and `cd`/`-C` extraction now live in one sourceable lib of pure string functions, unit-tested without git repos (new `command-parse.bats`, 23 tests). `commit-gate.sh` shrinks to orchestration. The policy rationale moved next to the code that implements it. Marketplace repos' version-bump gates can source the same lib so twin definitions cannot drift.
- **Commit detection runs on the joined command view.** A backslash-continued `git \<newline>commit` is one invocation to bash and is now one invocation to the gate; previously the per-line entry grep missed it entirely.

## [0.8.2] - 2026-08-03

### Fixed

- **Chained `review-sentinel mark && git commit` is no longer denied.** The commit-gate hook fires before the whole Bash chain executes, so it evaluated the sentinel against the pre-mark state and blocked the chained form of the `/accept` flow, forcing mark and commit into separate Bash calls. The pass-through is strict about what qualifies: the command must contain exactly one `git commit`, and a bare `mark` (no `--root`) at a command position, with the binary name at a path boundary, must be joined to it by `&&` with nothing in between — so the commit only runs if the mark actually succeeded. `;`, `||`, and newline separators stay denied (they'd let a failed or short-circuited mark's commit through — including `||` anywhere before the mark), as do commands between mark and commit, a mark after the commit, and extra commits after a marked one. Shell comments are stripped before the prefix match, while the commit count is taken from the raw bytes so a quoted `#` (e.g. `-m "fix #12"`) can never hide a later commit; the single-commit requirement keeps quoted or heredoc text containing the phrase from standing in for the real commit. New `commit-gate.bats` suite (57 tests) pins both directions.
- **`git -C <repo> commit` (and other global-option forms like `git -c k=v commit`) no longer slip past the gate.** The commit detection only matched `commit` immediately after `git`, so the natural cd-free shape `git -C <path> commit` — and subshell/backtick forms like `(git commit …)` — bypassed the sentinel check entirely. Detection now skips global options in every real shape — quoted values with spaces (`-c user.name='A B'`, `-C "/my repo"`), separate-argument long options (`--git-dir <path> --work-tree <path>`) — and recognizes `(`/backtick command openers. The `-C` path feeds project-root resolution the way a leading `cd` already did: extracted only when the command holds a single commit invocation (prose mentioning `git -C` cannot steer the gate), last `-C` wins to match git, it outranks `cd` (the `-C` decides where the commit lands), and relative paths resolve against the cd target or payload cwd.

## [0.8.1] - 2026-06-14

### Fixed

- **A reviewed *new* file no longer reads as drift once `git add`-ed.** An untracked file was hashed via a different representation (a `--UNTRACKED:` content dump) than its staged form (a `git diff --cached` patch), so any `git add`/`git add -A` between marking the sentinel and committing (e.g. routine bead bookkeeping that touches new files) flipped the gate to false drift and blocked the commit. Untracked files are now intent-to-added (`git add -N`) into a throwaway copy of the index, and the index→working-tree segment is diffed against that scratch index, so untracked, staged, and committed forms of the same new file hash identically. The real index and working tree are never mutated, and the index/worktree divergence check that catches the Codex P1 bypass is unaffected (the scratch index covers only the worktree segment). New regression tests cover the untracked→staged and untracked→staged→committed transitions.

## [0.8.0] - 2026-06-05

Closes the loop on a class of spurious review re-triggers: a pre-commit hook that reformats files at commit time leaves working-tree churn the gate reads as fresh unreviewed drift, forcing a needless second review.

### Added

- **Phase 2 "Canonicalize the working tree."** `/review-cycle:review` now brings the tree to the project's canonical state *before* the reviewer fan-out — running the project's own auto-fixers (formatters, `lint --fix`) and fast read-only checks (typecheck, affected tests), sourced from definitions the agent already has (`CLAUDE.md`/`AGENTS.md`, the pre-commit config, `package.json` scripts, `justfile`/`Makefile`/`Taskfile`/`mise`, CI). Reviewers see clean code; the marked state matches what the commit-time hook produces, so a formatter re-running at commit no longer strands changes the gate reads as drift. Auto-fixers are scoped to the changed fileset; slow suites are surfaced rather than run; the whole phase is fail-open. The later phases renumber accordingly (Fan-out → Phase 3, … Stop → Phase 10).
- **README note on pre-commit hooks and review re-triggers** (Troubleshooting). The leave-a-clean-tree principle: a hook that mutates files at commit time must fold those edits into the commit (scope formatters to staged files) or the residue re-fires the gate. Names the common `cargo fmt --all` vs. staged-scoped footgun.

### Changed

- **Agent task-state and IDE exclusions now match at any depth, not just the repo root.** `.beads/`, `.trekker/`, `.vscode/`, `.idea/`, `.zed/`, `.cursor/`, and `.fleet/` are excluded from the sentinel hash wherever they appear, so a monorepo that keeps `.beads/` in a subpackage no longer trips the gate when a `bd`/lefthook pre-commit hook re-exports that subpackage's `issues.jsonl`. Each directory now carries both a root-anchored (`.beads/**`) and an any-depth (`**/.beads/**`) pathspec; the root form is retained because older git pathspec parsers treat a leading `**/` as one-or-more directories rather than zero-or-more. Reverses the deliberate root-only scoping documented in 0.6.2. Test X7 flipped accordingly, with new hash-path and regression coverage (X7b/X7c).

## [0.7.0] - 2026-05-28

Adds two report-only reviewers and consolidates the plugin to a single `review` command, informed by Cursor's `thermo-nuclear-code-quality-review` and Matt Pocock's two-axis `review` skill.

### Added

- **`maintainability-auditor` agent** — an ambitious structural lens: "code-judo" moves (restructurings that delete whole categories of complexity), file-size sprawl (~1000-line threshold), spaghetti-branch growth, weak seams, and testability regressions. Runs once per review as a **report-only** reviewer — its speculative restructurings are surfaced in a "Structural suggestions" section for you to action by prompting, never auto-applied, because high-blast-radius rewrites must not be applied unsupervised in a loop.
- **`spec-conformance-analyzer` agent** — a **report-only** spec axis: checks the diff against its originating issue/task/PRD and reports missing or partial requirements, scope creep, and implemented-but-wrong, each quoted against the spec line. Reported separately from quality findings, because a change can follow every standard while building the wrong thing.

### Changed

- **`/review-cycle:review` is the single command for the whole cycle.** The auto-fix reviewers (Codex, code-reviewer, tests, error handling, type design) run in the fix loop; the two report-only reviewers and the de-slopify cleanup run once *after* the loop converges, against the final post-fix state — so the expensive opus maintainability pass runs a single time rather than on every iteration.
- **Arguments are natural language, not flags** — `against <ref>` and `max <n>` instead of `--base` / `--max-iter`; bare `/review-cycle:review` covers the common case. Flags don't autocomplete in Claude Code and are awkward to dictate.
- **`pr-test-analyzer` fires on any source change**, closing the blind spot where a feature that shipped zero tests skipped coverage analysis entirely.
- **`type-design-analyzer` catches type-boundary smells** — needless optionality, escape-hatch `any` / un-narrowed `unknown`, and casts that paper over a boundary — anywhere in the diff, not only on type declarations.
- **The cleanup agent removes redundant comments decisively.** A comment that restates the code is removed on sight; only a comment that may encode a constraint the agent cannot verify is kept, and it is flagged in the summary for a human to confirm.

### Removed

- **`/review-cycle:inspect` and `/review-cycle:cleanup` skills.** The maintainability lens that `inspect` hosted now runs inside `/review-cycle:review` (report-only), and `/review-cycle:de-slopify` covers ad-hoc prose cleanup; the cleanup *agent* still runs automatically in the cycle. `review` is now the only review command.

## [0.6.3] - 2026-05-13

### Fixed

- **`write_sentinel` correctly reports `printf` failures.** The previous form `err=$(printf '%s\n' "$content" > "$tmp" 2>&1)` intercepted stdout via the file redirect before `2>&1` could merge stderr into the capture, so `err` was always empty and the failure branch was unreachable. The function now checks `printf`'s exit code directly. Functional impact is small (the existing `mv` step catches most downstream failures), but the function now fails honestly on a `printf` error.

### Changed

- **Built-in-exclusion test split per directory.** The single parameterized X4 test became seven small tests so bats can identify which exclusion regressed if one of them breaks.

## [0.6.2] - 2026-05-13

Fixes the bug where edits confined to agent task-state (e.g. `.beads/`) forced a review. Also closes a self-exclusion bypass found by `/review-cycle:review` against an in-progress patch.

### Changed

- **Gate ignores non-code state by default.** Agent task tracker directories (`.beads/`, `.trekker/`) and IDE state directories (`.vscode/`, `.idea/`, `.zed/`, `.cursor/`, `.fleet/`) are now excluded from the sentinel hash. Changes confined to those paths no longer trip the Stop or commit-gate hooks. `/review-cycle:review` still works manually for users who want a review pass anyway. Exclusion is anchored at the repo root; a nested `subproject/.beads/` is still hashed.
- **New `.claude/review-cycle.json` config.** Schema: `{"disabled": bool, "ignore": [string]}`. `disabled: true` opts the project out of all gates. `ignore: [...]` extends the built-in exclusion list with project-specific pathspec-glob patterns. Requires `jq`. The legacy `.no-review-gate` marker is still honored indefinitely; there is no auto-migration, because the old marker was typically gitignored (local-only opt-out) while the new file is meant to be committed (team-wide), and silently converting one to the other could publish an opt-out unintentionally. Users who want to consolidate can write the new file themselves and remove the marker manually.

### Added

- **Pipeline fail-closed on git/jq errors.** A malformed pathspec, missing sha tool, or any mid-pipeline git failure now returns drift (1) instead of silently producing `sha256("")` and passing the gate. Added a smoke-test using the assembled pathspec before hashing.

### Security

- **Self-exclusion bypass closed.** The previous in-progress draft excluded the user-provided ignore file from its own hash, so an unreviewed edit adding `src/**` plus an unreviewed `src/app.ts` change could pass the gate without ever being reviewed. The new config file is **not** in the default excludes AND is force-included in the hash regardless of user `ignore` patterns. Editing `.claude/review-cycle.json` always forces a review pass before its rules take effect, even if the user added patterns that would otherwise match it (e.g. `**`, `.claude/**`).
- **Malformed-pathspec bypass closed.** Earlier draft of the smoke-test returned exit 1 on git rejection, which `check` mapped to exit 2 (internal error, hooks fail-open). A user with a broken `ignore` pattern could therefore disable the gate until the config was fixed. The smoke-test now returns 2 directly, which `check` maps to drift (hooks block).
- **Explicit `disabled: false` honored over stale legacy marker.** `gate_project_opted_out` now honors the config's `disabled` key exclusively when present, so a hand-written `{"disabled": false}` cannot be silently overridden by a leftover `.no-review-gate`.

## [0.6.1] - 2026-05-12

Security and robustness fixes for issues found by running `/review-cycle:review` against the 0.6.0 release before broader adoption. **Users on 0.6.0 should upgrade immediately** — 0.6.0 contained a gate bypass.

### Security

- **Staged-content bypass closed.** The hash now captures both `git diff --cached <anchor>` (anchor → index) and `git diff` (index → working tree) per file, sorted by path. The 0.6.0 implementation used only `git diff <anchor>` (anchor → working tree), so a user could stage unreviewed content, restore the working tree to the reviewed state, and then commit — slipping unreviewed bytes past the gate. Per-path iteration also keeps the hash byte-stable across staging-state changes, so moving reviewed content between staged and unstaged does not drift.

### Fixed

- **`match` subcommand exit codes distinguish error from no-match.** Now exits 2 on real errors (missing sha tool, not in work tree, pipeline failure) and 1 only on actual no-match. The 0.6.0 implementation collapsed all failures to exit 1, breaking `session-init`'s ability to detect a misconfigured environment.
- **Silent failure in legacy-hash migration.** `session-init` now emits a stderr warning when the 0.5.x→0.6.0 migration cannot compute the legacy hash (e.g., missing sha256sum/shasum). Previously the failure was swallowed and the user was left permanently gated with no breadcrumb.
- **Anchor type validation.** Sentinel anchors are now checked via `git cat-file -t` and must resolve to a commit or tree object. The previous `cat-file -e` accepted any object (including blobs and tags), which would have produced a meaningless hash.
- **Pipefail and PIPESTATUS checks** on the hash compute pipeline. A mid-pipeline `git diff` crash now surfaces as a compute error instead of silently producing a valid-looking but wrong hash.

## [0.6.0] - 2026-05-12

> ⚠️ **Withdrawn**: this release contained a gate bypass via staged content. Upgrade to 0.6.1.

### Fixed

- **Multi-commit drift after a single review.** Previously every `git commit` advanced HEAD and shrank the diff the sentinel hashed against, so the gate flagged drift even when no unreviewed content had been introduced. Reviewing a batch and then splitting it into N commits required N reviews. The sentinel now pins (anchor SHA, diff-from-anchor hash) instead of (diff-from-HEAD hash), so committing already-reviewed content does not invalidate the sentinel — the cumulative anchor→working-tree diff stays the same regardless of how many of the reviewed hunks have been committed.

### Changed

- **Sentinel format is now two lines:** `anchor:<40-hex>` (HEAD SHA at mark time, or the empty-tree SHA `4b825dc6…` for unborn HEAD) and `sha256:<64-hex>` (hash of `git diff <anchor>` plus untracked file contents). Migration from 0.5.x is automatic via `session-init` on next startup, with lossless upgrade when the working tree still matches the previously-reviewed state.
- **New `match` subcommand on `bin/review-sentinel`.** Used by `session-init` to decide whether to advance the anchor; differs from `check` in that it does not treat a clean tree as a pass. `check` and `match` together replace the prior pattern of comparing `current-hash` output against the raw sentinel file.
- **`current-hash` output is now two lines** (`anchor:` then `sha256:`) to match the on-disk format. Anyone scripting against the old single-line output will need to update.

### Migrated

- **Single 0.5.x → 0.6.0 migration block** in `session-init.sh` replaces the previous 0.5.0 → 0.5.1 block. Detects any pre-0.6.0 sentinel (bare hex or `sha256:`-prefixed), computes the legacy hash against current state, and re-seeds in the new format only when they match (lossless upgrade). When they don't match, the old sentinel is preserved so the gate fires on the unreviewed drift.

### Behavior unchanged

- The four hooks (`session-init`, `stop-gate`, `commit-gate`, `posttool-slop`) keep their existing semantics. Only `session-init` changed; the others just call `review-sentinel check`.
- Clean-tree fast-path is preserved: `check` still exits 0 on a working tree with no changes regardless of stored sentinel content.

## [0.5.2] - 2026-05-12

### Changed

- **`review`, `cleanup`, and `inspect` are now model-invocable.** The commit-gate hook is the actual boundary against unreviewed commits, so blocking model invocation on these skills only created an incoherent flow: the Stop hook would tell Claude to invoke `/review-cycle:review`, and Claude couldn't. `accept` (gate bypass) and `init` (meta-setup) remain user-only.

## [0.5.1] - 2026-05-11

### Changed

- **Gate state is now factored into a shared CLI (`bin/review-sentinel`) and a sourced lib (`hooks/lib/gate.sh`).** The four hooks (`session-init`, `stop-gate`, `commit-gate`, `posttool-slop`) each shrink to their actual decision logic; preconditions and sentinel I/O live in one place. `/review-cycle:accept` and Phase 7 also call the CLI instead of re-implementing hash computation inline. The sentinel path (`${PROJECT_ROOT}/.claude/.review-mark`) is unchanged; existing sentinels self-heal on the next `startup` session.

- **Hash now captures content changes, not just file-level state.** Previously the sentinel hashed `git status --porcelain --untracked-files=all` only, so editing an already-modified file (without adding new files) didn't update the hash — the gate would pass when it shouldn't. The new computation concatenates porcelain status, `git diff --cached --binary`, `git diff --binary`, and the contents of untracked files. Splitting staged+unstaged (vs. `git diff HEAD`) covers repos without an initial commit; staged content in unborn repos now correctly contributes to the hash. Subsequent edits to the same file also correctly drift the sentinel.

- **`session-init` re-seeds on `startup` only when the prior state was reviewed.** Re-seeds when the sentinel is missing (first install; pre-existing WIP becomes the baseline) or when the sentinel matches the current state (idempotent refresh). If the sentinel disagrees with the current state, the previous session left unreviewed work; `session-init` keeps the old sentinel and lets Stop/commit gates do their job. `/clear`, `/compact`, and resume events are not `startup` events and don't fire this hook. Trade-off: dependency bumps or IDE edits between sessions now require a one-time `/review-cycle:accept` or `/review-cycle:review` to re-baseline, but quit-and-restart with WIP no longer silently absorbs unreviewed changes.

- **Clean working tree always passes `check`.** The sentinel CLI exits 0 on a clean tree regardless of the stored hash, eliminating the post-commit re-block loop where the user would have to run `/accept` after every commit just to clear the gate.

### Fixed

- **One-time migration from 0.5.0 sentinel format.** 0.5.0 wrote a bare 64-char hex hash; 0.5.1 writes `sha256:<hex>`. On the first 0.5.1 `startup` session that finds an old-format sentinel, `session-init` re-seeds it. This restores self-heal for the upgrade path without absorbing in-session unreviewed work on subsequent restarts.
- **`hooks/posttool-slop.sh`: comment-slop findings rendered with literal `\n` instead of newlines.** Pre-existing bug from 0.5.0 — the `FINDINGS` variable used `"\n\n"` inside double quotes (which doesn't interpret escapes) and jq propagated those as `\\n` into Claude's `additionalContext`. Switched to `$'\n'` so the rendered context is actually newline-separated and readable.
- **`hooks/posttool-slop.sh`: now bails when the modified file is outside any git repo**, matching the scope of the other three hooks. Previously it would inject context for orphan files.
- **`bin/review-sentinel`: defense-in-depth git work-tree check** in `compute_current_hash`. If a refactor ever calls it with a non-repo path, it now returns nonzero instead of silently producing the empty-tree hash and reporting "clean".
- **`bin/review-sentinel`: `read_sentinel` warns to stderr and returns nonzero** on malformed content. Callers can now distinguish missing from corrupted (`check` still treats corrupted as drift; the warning surfaces in the next hook output).
- **`bin/review-sentinel`: `write_sentinel` forwards underlying error to stderr.** Previously `2>/dev/null` swallowed permission/disk-full/path errors silently. The mkdir, write, and rename now each capture stderr and emit a specific message before returning. Temp file is cleaned up on write or rename failure.
- **`hooks/session-init.sh`: strict re-seed.** Only re-seeds when the sentinel exactly matches the current hash (idempotent refresh) or is missing (first install). Previously the conditional piggybacked on `check`'s clean-tree exit-0, which let a transient `git stash` or `git checkout` overwrite a prior-session sentinel with the empty-tree hash. The strict version preserves the prior sentinel as evidence whenever current state diverges.

### Added

- **`/review-cycle:init` now preflights `jq`, `git`, and a sha256 tool** (`sha256sum` or `shasum`). Previously a machine missing any of these would silently fail-open at every hook — the gate would appear to be doing nothing for no obvious reason. Each missing tool now surfaces a clear install hint in the init summary.

- **Bats smoke suite** at `tests/`. Covers the sentinel CLI (seed/mark/check/paths, clean-tree, drift detection, format validation, exit codes) and the gate lib (kill-switch, opt-out marker, project-root resolution chain, composite check). Run with `tests/run.sh`, which wraps bats with a post-suite cleanup to work around a known hang on macOS.

## [0.5.0] - 2026-05-11

### Added

- **PostToolUse comment-slop detector** (`hooks/posttool-slop.sh`). Fires after `Write`, `Edit`, or `MultiEdit` and scans the modified file for high-confidence comment-slop patterns. When detected, returns `hookSpecificOutput.additionalContext` so Claude addresses them on the next turn. Does NOT block — the write already happened; this is informational reinforcement of the comment policy in real time.

  Patterns flagged:
  - Section markers (`// ===== HELPERS =====`)
  - Restate-the-code verbs at start of comment (`// fetches the user`)
  - AI-flavored phrasings (`// Here we ...`, `// Let's ...`, `// This function does ...`)
  - Hedge prefixes (`// Note:`, `// Important:`, `// NB:`)
  - TODO/FIXME without ticket reference (skipped if `#123`, `ABC-123`, or URL follows)
  - Hedge words in comments (`obviously`, `basically`, `simply`, `just`, `actually`)

  Limits: skips binary/lock/build-artifact paths and files over 1MB. Respects the global kill-switch and per-project opt-out marker like the other hooks. Catches the comment patterns Opus 4.7 most often introduces mid-implementation — supplements the cycle's end-of-cycle cleanup with real-time intervention.

## [0.4.2] - 2026-05-10

### Fixed

- commit-gate now correctly resolves the project root when the Bash command is `cd <path> && git commit ...`. Previously, the hook ran `git rev-parse --show-toplevel` from the session cwd (not the cd target), so if the user ran from `$HOME` and cd'd into a project inline, `PROJECT_ROOT` resolved empty and the hook exited 0 fail-open instead of blocking. Now the hook parses a leading `cd <path>` from the command, expands `~`, falls back to the hook input's `cwd` field, then to `CLAUDE_PROJECT_DIR`, then to the shell cwd. Confirmed end-to-end: `git commit` from a different cwd now correctly produces the documented deny.
- Verified that bypassPermissions mode does NOT override hook deny decisions (was an earlier incorrect hypothesis — proven false by a hard-deny test).

## [0.4.1] - 2026-05-10

### Fixed

- commit-gate hook now produces output matching the documented PreToolUse schema. The hook was using the deprecated top-level `decision: "block"` / `reason` fields, which PreToolUse no longer honors (silently treated as "no decision," letting `git commit` through). Switched to `hookSpecificOutput.permissionDecision: "deny"` per the current docs. The hook now actually blocks unreviewed commits — confirmed in isolation with a realistic JSON input. Same class of bug as the Stop hook schema fix in 0.1.2.

## [0.4.0] - 2026-05-10

### Added

- Embedded 4 subagents from Anthropic's pr-review-toolkit at `agents/`:
  - `code-reviewer.md`
  - `silent-failure-hunter.md`
  - `type-design-analyzer.md`
  - `pr-test-analyzer.md`

  Invoked under the plugin namespace as `review-cycle:<agent-name>`. Copied verbatim from `anthropics/claude-plugins-public`; license preserved at `LICENSE-pr-review-toolkit`; attribution in `NOTICE`. The `code-simplifier` and `comment-analyzer` agents are intentionally not migrated (see NOTICE for reasoning).

- New `cleanup` subagent at `agents/cleanup.md`. Preloads the bundled de-slopify skill via the `skills` frontmatter and applies both the comment policy and de-slopify methodology in a single pass. Edits files directly; returns a structured summary.

- New `/review-cycle:cleanup` skill — thin wrapper around the cleanup subagent for `/`-invocable ad-hoc tidy-ups.

- New `/review-cycle:accept` skill — updates the review sentinel to mark the current state as reviewed without running the full cycle. Per-state escape hatch for "I've manually reviewed, let me commit" flows.

### Changed

- Cycle Phase 2 fan-out now spawns each pr-review-toolkit-style subagent directly via the Agent tool with `run_in_background: true`, instead of invoking `/pr-review-toolkit:review-pr all parallel`. Conditional dispatch (code-reviewer always; test/error/type analyzers based on diff scope) moves into the cycle skill's prose. No external slash-command dependency for review agents.
- Cycle Phase 6 cleanup now spawns the `cleanup` subagent instead of invoking the de-slopify skill directly. The cleanup agent owns both the comment policy and the de-slopify application in a single phase.
- Inspect Phase 2 mirrors the same direct-Agent-invocation pattern.

### Notes

- This release drops the runtime dependency on the pr-review-toolkit plugin. The Codex CLI is still required (already true since 0.2.0). The plugin is now fully self-contained for its review work.
- Roadmap remaining: v0.5.0 — PostToolUse hook for real-time comment-slop intervention (optional).

## [0.3.2] - 2026-05-10

### Changed

- Tightened the `/review-cycle:init` summary output. Replaced the bracketed two-column status format (`[✓|⚠|✗] Codex CLI: ...`) with single-glyph leading status (`✓ Codex CLI: ...`). Avoids wrapping in narrow terminals and reads more scannably.

## [0.3.1] - 2026-05-10

### Added

- `/review-cycle:init` skill — one-time setup helper. Verifies Codex CLI and `multi_agent` config, optionally appends the comment + fix-vs-defer policies to `~/.claude/CLAUDE.md` and/or `./CLAUDE.md`, and updates project `.gitignore` to exclude the per-project sentinel files (`.claude/.review-mark`, `.claude/.no-review-gate`). Idempotent — safe to run multiple times. Replaces the manual setup steps previously documented in the README.

## [0.3.0] - 2026-05-10

### Added

- Bundled the `de-slopify` skill at `skills/de-slopify/` (full skill including `references/` subdir). Invokable as `/review-cycle:de-slopify` for ad-hoc prose cleanup, or invoked automatically by the cycle's Phase 6.
- Source remains at [oakoss/agent-skills](https://github.com/oakoss/agent-skills); the bundled copy is a snapshot synced on each plugin release. Cross-agent skills.sh distribution stays at agent-skills; the plugin's copy makes review-cycle self-contained for Claude Code users.

### Changed

- Comment policy in the embedded skill bodies and `reference/policies.md` softened from "default to NO comments, only add when WHY is non-obvious" to "comments are fine; keep them clean and minimal." Same set of bad patterns flagged, but the default action shifts from "remove" to "trim/rewrite" for accurate-but-verbose cases. Aligns with how Opus 4.7 should actually write comments, not just how to suppress them.
- Cycle Phase 6 now invokes the bundled `/review-cycle:de-slopify` directly rather than relying on a user-level `de-slopify` installation.

### Notes

- If you have a user-level `de-slopify` skill installed at `~/.claude/skills/de-slopify/`, you can remove it after upgrading to this version — the plugin's namespaced copy supersedes it. Or keep both; they don't conflict.

## [0.2.0] - 2026-05-10

### Changed

- Codex review is now invoked directly via the `codex review --uncommitted` CLI rather than through the `/codex:review` slash command. The Codex Claude plugin is no longer a dependency — only the Codex CLI binary needs to be installed and authenticated. This simplifies the dependency graph and avoids edge cases around invoking skills with `disable-model-invocation: true` from inside other skills.
- Codex preflight check changed from `/codex:status` slash command to direct `codex --version` invocation.

### Notes

- This is the first step in the dependency-reduction roadmap. Subsequent versions will embed de-slopify (0.3.0) and migrate pr-review-toolkit subagents into this plugin (0.4.0).

## [0.1.2] - 2026-05-10

### Fixed

- Stop hook output no longer includes `hookSpecificOutput`, which is not a valid field for Stop hooks per Claude Code's runtime schema (only `PreToolUse`, `UserPromptSubmit`, `PostToolUse`, and `PostToolBatch` accept `hookSpecificOutput`). Directive content moved into the top-level `reason` field, with a short label in `systemMessage`. Previously the hook produced JSON that failed schema validation at runtime with "Hook JSON output validation failed".

## [0.1.1] - 2026-05-10

### Changed

- Renamed the main action skill from `cycle` to `review` to align with the Anthropic convention used by `pr-review-toolkit:review-pr` and improve discoverability in the `/` autocomplete. Invocation changed from `/review-cycle:cycle` to `/review-cycle:review`. All hook directives, documentation, and policy references updated accordingly.

## [0.1.0] - 2026-05-10

### Added

- Initial release.
- `/review-cycle:cycle` skill — full automated review loop with parallel Codex + pr-review-toolkit fan-out, fix-vs-defer policy, up to 4 iterations, and final de-slopify cleanup.
- `/review-cycle:inspect` skill — read-only inspection pass for sanity checks or pre-commit review.
- SessionStart hook to seed the per-project review sentinel idempotently on fresh session starts.
- Stop hook to gate turn-end on uncommitted-and-unreviewed changes.
- PreToolUse (Bash) hook to block `git commit` when the sentinel doesn't match the current state.
- Per-project opt-out via `.claude/.no-review-gate` and global kill-switch via `~/.claude/.disable-review-gate`.
- Embedded comment and fix-vs-defer policies inside the skills, with standalone copies in `reference/policies.md` for optional CLAUDE.md installation.
