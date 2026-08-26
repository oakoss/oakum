# Task tracking

Use `bd` (beads). Run `bd prime` for the command reference and session protocol.

Run `mise run setup` after cloning. A fresh clone has no beads database — `.dolt/` and `*.db` are gitignored. Setup runs `bd bootstrap` only when `bd` is on PATH and `bd list` reports `no beads database found`, then installs lefthook. When `BD_SYNC_REMOTE` is set in `.beads/.env`, bootstrap clones that remote; without it, it creates a fresh local database. Missing `bd`, or a `bd list` failure that is not that miss, still installs lefthook.

Prefer `bd bootstrap` to `bd init` generally. Bootstrap leaves `core.hooksPath` unset and `AGENTS.md` untouched; `bd init` sets the former — making git bypass lefthook silently — rewrites the latter, and commits with a message `cog verify` rejects. `core.hooksPath` must stay unset: `lefthook.yml` calls `.beads/hooks/*` directly, and `mise run check` fails if the setting reappears.

Do not run `bd setup codex`. This repository is worked in Claude Code; that command writes a `.codex/` directory and an `.agents/` skill nobody here reads. Both `bd init` and `bd setup codex` also append a managed block to `AGENTS.md` — strip it and keep the thin index.
