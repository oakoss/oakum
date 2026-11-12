---
oakum: patch
---

### Fixed

`oakum migrate` skips a hook git would not run because the current user may not execute it, as git does. A hook whose only execute bits belong to other users made `migrate` exit 2 reporting it unreadable (`Permission denied`) at mode `0001`, or list it as a gate at mode `0645`, though git ignores it.
