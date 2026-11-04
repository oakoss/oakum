---
oakum: patch
---

The `.changeset/README.md` that `init` and `migrate` write no longer says every changed package needs a bump file under `check --strict`: with conventional commits on, a package whose branch commits all release nothing is covered without one, and the README links the coverage guide. It no longer calls naming a dependent in a bump file a bug: a dependent releases without being named, and naming it gives it more than the cascade, such as a binary at `minor` for a library feature its users see. The stale advice about knope reading `.changeset/` is gone.
