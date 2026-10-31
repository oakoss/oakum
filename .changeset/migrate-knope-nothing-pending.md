---
oakum: major
---

### Changed

`oakum migrate` from knope compares plans when nothing is pending. knope's dry run prints nothing and exits 0 then, which migrate read as the tool being unavailable, ending unverified (exit 2) on the usual time to migrate, right after a release. When the workflow it ran has a `PrepareRelease` step (or `knope.toml` declares none, so knope's own defaults run), that silence is now an empty before-plan: nothing pending on either side is reported as such and exits 0, and a bump file oakum plans that knope does not is a finding (exit 1). Silence from a workflow with no `PrepareRelease` step, which knope gives whether or not anything is pending, stays unverified and says why, as does output oakum does not recognise.
