# Task tracking

Use `bd` (beads). Run `bd prime` for the command reference and session protocol.

Run `mise run setup` after cloning. A fresh clone has no beads database — `.dolt/` and `*.db` are gitignored. Setup runs `bd bootstrap` only when `bd` is on PATH and `bd list` reports `no beads database found`, then installs lefthook. When `BD_SYNC_REMOTE` is set in `.beads/.env`, bootstrap clones that remote; without it, it creates a fresh local database. Missing `bd`, or a `bd list` failure that is not that miss, still installs lefthook.

Prefer `bd bootstrap` to `bd init` generally. Bootstrap leaves `core.hooksPath` unset and `AGENTS.md` untouched; `bd init` sets the former — making git bypass lefthook silently — rewrites the latter, and commits with a message `cog verify` rejects. `core.hooksPath` must stay unset: `lefthook.yml` calls `.beads/hooks/*` directly, and `mise run check` fails if the setting reappears.

Do not run `bd setup codex`. This repository is worked in Claude Code; that command writes a `.codex/` directory and an `.agents/` skill nobody here reads. Both `bd init` and `bd setup codex` also append a managed block to `AGENTS.md` — strip it and keep the thin index.

## Versions, branches, and order

Four facts about a bead live in four places: a version label says what it does to a user, a parent says which branch it rides, a dependency says what comes first, and priority says how urgent it is. Rationale and sources: [research/version-lines-and-backport-labels.md](../research/version-lines-and-backport-labels.md).

### Version labels

The contract oakum keeps is the exit outcome ([ADR-0034](../decisions/0034-exit-two-for-unverified.md)) and the `status --json` document ([ADR-0016](../decisions/0016-emit-release-state-render-it-never-deliver-it.md)). Stderr wording is not promised, and `check` has no `--json` ([ADR-0035](../decisions/0035-a-finding-outranks-an-unverified-look.md); okm-404.58 would add one), so its finding wording is unpromised with it; that channel, once it exists, joins the surfaces the `patch` row holds unchanged. SemVer requires nothing below 1.0.0, so this line is oakum's own; [ADR-0022](../decisions/0022-zero-major-versioning.md) makes `0.y` the breaking-or-feature slot.

| Label | Means | Bump file |
|---|---|---|
| `patch` | a fix, or a stderr-only diagnostic; every exit outcome and every stdout/`--json` byte unchanged on inputs that work today. A correction to a `--json` field that has never appeared in a release is patch too ([ADR-0016](../decisions/0016-emit-release-state-render-it-never-deliver-it.md)); a shipped field is not | `oakum: patch` |
| `0.4.0` (the next minor) | something new that breaks nothing: a new refusal on an input that failed before, a new look, a new stdout or `--json` field, a new command or flag | `oakum: minor` |
| `0.4.0` (the next minor) | a breaking change: a changed exit outcome, a removed or renamed field, a refusal on an input that worked. The file level stays `major`; zero-major renders it as the same minor version | `oakum: major` |
| `0.5.0` (the minor after next) | minor- or major-class work held because its trigger (a reader, a consumer, a dependency) has not fired | as above, later |
| none | internal, tests, CI, docs, wording | `oakum: none` when anything under `crates/oakum` changes; `check --strict` asks for intent regardless |
| `regression` (beside a version label) | worked in the last tag, measured against it; the description names the tag | — |

This repository runs `versioning = "zero-major"` (`.changeset/_config.toml`): a `major` bump file still names a breaking change and still lands under `### Changed`, but below 1.0.0 it advances the minor component, so breaking and compatible changes share one version label ([ADR-0022](../decisions/0022-zero-major-versioning.md)). Graduating is a config edit in the pull request that cuts 1.0.0, per package where a workspace needs it (ADR-0022, *Graduating to 1.0.0*); the trigger is SemVer's, a contract users depend on, and the call is an ADR amendment. From 1.0.0 the `major` row gets its own version label, the patch and minor rows become SemVer's own rules rather than oakum's, and the contract surface is unchanged: stderr wording stays unpromised.

Label at triage; re-label when a decision picks the costlier outcome. A review finding that would change output becomes a bead with a label, not an inline fix. Epics and branch parents carry no version label; their children do. `bd create --parent` copies a parent's labels onto the child and re-parenting copies nothing, so a label on a container leaks one way only. A `none` file's note never reaches the changelog: write it for the reviewer, not the release. Never let a `patch` note describe a new refusal.

### Branches and parents

A branch is the unit of review and of history: one theme, sized so a reviewer reads one thing, squash-merged as one commit. Name it `<type>/okm-<id>-<theme>` with the conventional type (`feat`, `fix`, `perf`, `refactor`, `test`, `chore`, `ci`, `docs`).

A branch that carries several beads is a **parent bead** (`bd create --type=task`, members re-parented with `bd update <id> --parent=<parent>`); the parent's id is the one in the branch name, `bd ready --parent <parent>` is the branch's checklist, and its description grows into the PR body. A single-bead branch needs no parent. Reserve `epic` for a finding set or a rollout, not for a branch.

Follow-ups go to the standing successor, never to the branch in flight: a claimed parent takes no new children (a convention; `bd create --parent` does not refuse). A parent closes at ship only when `bd list --parent <parent> --status open,in_progress,blocked,deferred --limit 0` is empty (`open` alone hides the other three); anything left is re-parented to the successor first.

The type says what kind of change the branch is; the version label says what it does to a user. A `fix/` branch can need a minor bump.

### Order and urgency

- **Dependencies** say which branch comes first (`bd dep add <later> <earlier>`, parent to parent). `bd ready` then shows the next branch rather than the whole backlog; the earlier parent closes at ship, before its merge, which is what makes the later one ready. When a deferred bead's trigger is itself a bead, that is a dependency, not `bd defer`.
- **Priority** is urgency across the backlog, not order within a branch: P0 a hotfix now; P1 the next branch opened; P2 must ship in the version it is labeled for; P3 scheduled under a parent; P4 may slip a version.
- **Decision beads** close on a pointer: the ADR amendment or spec line that recorded the answer.

### Merging, releases, and hotfixes

- Patch and `none` branches merge whenever green; neither opens a minor window. A patch rides the pending patch release, a `none` adds nothing to it.
- **Merging the first minor branch opens the window for that minor.** From then on every merge rides it, and the window closes by merging the version PR. Open it on purpose, when the rest of the minor is close.
- A version accumulates as many branches as merge before it ships; a branch never mixes a patch note with a minor one, so each stays separately shippable.
- **A hotfix is a patch bead that is a regression from the last tag, a security issue, or a defect with no workaround (`regression` labels the first), on its own `fix/` branch, merged ahead of the queue, followed by merging the version PR.** With minor work already merged, ship the minor early instead; the rest of its plan rolls forward. A maintenance line off a tag is not planned until a user cannot leave a release; the shape it would take is sketched in [ideas/0007](../ideas/0007-maintenance-release-branches.md).
- When a minor ships, its label retires: open beads still carrying it move to the next minor, and closed ones keep it as the record of what shipped.

  ```sh
  unfinished=open,in_progress,blocked,deferred
  bd list --label 0.4.0 --status $unfinished --limit 0 --json | jq -r '.[].id' \
    | xargs -I{} bd update {} --add-label 0.5.0 --remove-label 0.4.0
  ```

### At ship

Close the branch's beads and its parent (no open children), `mise run check && mise run test`, `bd dolt push`, then the publish commit carries the audit line. Beads close at ship, not at merge.
