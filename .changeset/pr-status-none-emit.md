---
oakum: minor
---

### Added

`oakum ci pr-status --emit-comment DIR` with `pr-status = "none"` no longer refuses. It writes no comment or job summary, removes any `oakum-pr-comment.md` an earlier run left in `DIR`, and exits 0, so a trusted job posting the emitted comment can remove the old one after switching to `none`.
