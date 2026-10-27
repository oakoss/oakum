---
oakum: minor
---

With conventional commits, a commit that asks for a release (`feat`, `fix` or breaking) and changes no package's files, under a scope that names no workspace package such as `fix(gamma)` or `fix(alpha,beta)`, is now reported instead of dropped without a word. A merge under such a scope is reported too, since its paths are not read. A commit with no scope, or a non-conventional message, is not reported. `status` and `ci pr-status` list it under "Released by a commit but reaching no package", `status --json` and the title and commit-message templates carry it as `unresolved_scopes` (`commit`, `scope`), and `check`, `version` and `generate` name it on stderr. Exit codes do not change, `--strict` included. A commit that releases nothing is not reported, and its ambiguous scope (a Cargo crate and an npm package sharing the name) now falls back to its paths instead of refusing the run.
