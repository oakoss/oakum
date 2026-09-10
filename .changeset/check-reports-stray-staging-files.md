---
oakum: major
---

`check` names every `.<name>.oakum-write.*` staging file an unfinished oakum write left in `.changeset/`, at the repository root, beside a package manifest, or beside a declared extra file, and exits `unverified`; `init` and `migrate` name one in `.changeset/` on every run, the already-initialized and already-migrated paths included. Nothing removes it, since a run still in progress could own it.
