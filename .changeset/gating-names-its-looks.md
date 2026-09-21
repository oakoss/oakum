---
oakum: patch
---

The `check --json` scope field `gating_coverage` is now `gating`, and carries the names of the looks `--strict` decides rather than a yes or no. `--strict` gates two looks as of this release — coverage, and the notes look that refuses a bump file note reaching no changelog — so a consumer that could only ask about coverage could not tell which of them refused; the names match `looks[].look`, so the two can be joined. The field says what `--strict` decided, not what the looks then managed: a look that could not run is still named, and reports `unverified` in its own row. An empty list means `--strict` was not asked for. A `patch` rather than a renamed field under the breaking row, and no `schema_version` bump: `check --json` has not appeared in a release — it was added after the v0.3.2 tag and its note is still pending — so no consumer can be holding the old name (ADR-0016 on a field that has never shipped).
