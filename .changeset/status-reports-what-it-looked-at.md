---
oakum: minor
---

### Added

`status` reports a changed package the config keeps but cannot version — a private package where `private-packages.version` is not set, so no bump file could ever cover it. It appears in the rendered summary and as an `unmanaged` array in `--json`, and it does not change the exit code. `check` stays silent on that state on purpose: `status` reports, `check` decides, and there is nothing here to decide.

`status --json` reports coverage for real. It emitted `uncovered: []` for every repository, whatever had changed, because the command never ran the coverage look — only `check` and `ci pr-status` did. `schema_version` is deliberately unchanged: the new fields are additive and an old parser sees the shape it always saw, but a reader that treated an empty `uncovered` as a clean result should now check `coverage_checked`, which says whether the look ran at all. Where git cannot diff the tree, `status` says so in the rendered summary as well as the JSON, since stderr does not reach a step summary or a pull-request body.

`status` distinguishes a repository with nothing pending from one whose config manages no package on either axis — every selected package private, with the opt-in unset. Both printed `No packages planned.` and exited 0; the second now says so, and carries `manages_nothing` in its JSON. A selection that `include`/`exclude` empties is not covered by this and stays silent, in `status` as in `check`, because an empty selection is a decision someone wrote down. `ci pr-status` reports the same facts, and no longer stays silent on such a repository — it planned nothing and covered nothing, so the early return swallowed the one thing worth saying.
