# Task tracking

Use `bd` (beads). Run `bd prime` for the command reference and session protocol.

Run `mise run setup` after cloning. A fresh clone has no beads database — `.dolt/` and `*.db` are gitignored — so until it runs, every `.beads/hooks/*` hook exits 3 and skips itself with one line on stderr. Setup runs `bd bootstrap`, which clones the task graph from the `sync.remote` in `.beads/config.yaml`; the graph lives on a Dolt remote, not in this repository.

Prefer `bd bootstrap` to `bd init` generally. Bootstrap leaves `core.hooksPath` unset and `AGENTS.md` untouched; `bd init` sets the former — making git bypass lefthook silently — rewrites the latter, and commits with a message `cog verify` rejects. `core.hooksPath` must stay unset: `lefthook.yml` calls `.beads/hooks/*` directly, and `mise run check` fails if the setting reappears.

Do not run `bd setup codex`. This repository is worked in Claude Code; that command writes a `.codex/` directory and an `.agents/` skill nobody here reads. Both `bd init` and `bd setup codex` also append a managed block to `AGENTS.md` — strip it and keep the thin index.
