---
oakum: major
---

### Changed

A manifest below its highest tag, such as 0.1.0 after `v0.2.0` shipped, no longer plans a version already released or below one. `check` fails naming both versions and the fix ("N package(s) below their highest tag"), `version` and `ci version-pr` refuse before any write (exit 1), and `status`, `status --json` (`hand_set`) and the `ci pr-status` comment show the plan as blocked. To tell, these read tags whenever the plan bumps a package oakum tags, tagged yet or not, so in a shallow clone they now exit 2 (`unverified`) and ask for full history; a fresh `0.0.0` or `0.1.0` package with no earlier changelog section still versions there.
