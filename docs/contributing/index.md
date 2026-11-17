# Contributing

Agent and contributor process for this repository. Root `AGENTS.md` always-loads the invariants and project defaults, then indexes these pages for the rest.

## Getting started

1. Install [mise](https://mise.jdx.dev/getting-started.html). On Windows, install Git for Windows too: every task runs under its `bash`.
2. In the clone, `mise trust` and then `mise install`, which installs the pinned Rust toolchain, pnpm and the linters.
3. `mise run setup` installs the lefthook git hooks and, when `bd` is on your `PATH`, a beads database ([task-tracking.md](task-tracking.md)).
4. `mise run check && mise run test`. Both should pass before you change anything: on a fresh clone with the tools installed, `check` took 19 s and `test` 123 s.

A pull request runs more than these two: root `AGENTS.md` lists what CI runs and says when a change needs a bump file.

| Doc | Holds |
|---|---|
| [invariants.md](invariants.md) | Rules that override agent and tool defaults |
| [structure.md](structure.md) | Workspace layout and when to split crates |
| [documentation.md](documentation.md) | Where docs belong; ADR and research discipline |
| [task-tracking.md](task-tracking.md) | Beads bootstrap, version labels, branch parents, order, and the ship checklist |
| [pull-requests.md](pull-requests.md) | PR bodies and titles |
| [conventions.md](conventions.md) | Commits, branches, push/PR authority |
| [self-hosting.md](self-hosting.md) | How this repository runs its own oakum, locally and in CI |

Product docs (decisions, specs, research, ideas, guide) stay under [`docs/README.md`](../README.md).
