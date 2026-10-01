---
oakum: patch
---

`oakum check`, `oakum release` and the other commands that verify the install pin no longer hang when a pin file is a FIFO. The look opened `package.json`, `.mise.toml`, `mise.toml` and `.github/actions/*/action.yml` with a blocking open, so a FIFO nobody wrote to stalled the command forever. It now opens them non-blocking, as oakum's other file readers do, and refuses as `unverified` (exit 2), naming the file that is not a regular file. A missing file and a dangling symlink still read as no pin there. A `cargo` or `pnpm` reached through a mise shim reads `mise.toml` itself, so on such a PATH a FIFO there can still block before oakum looks.
