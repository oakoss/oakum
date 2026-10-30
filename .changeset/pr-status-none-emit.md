---
oakum: minor
---

### Added

`oakum ci pr-status --emit-comment DIR` with `pr-status = "none"` no longer refuses. It writes no comment or job summary, leaves the `oakum-pr-comment.none` marker in `DIR`, and exits 0 without calling GitHub, so the trusted job posting the emitted comment removes the old one after switching to `none`.
