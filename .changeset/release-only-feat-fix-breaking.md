---
oakum: major
---

Conventional commits now release only for `feat` (minor), `fix` (patch) and a breaking change (`!` or a `BREAKING CHANGE` footer, any type: major), as knope and release-please read the convention. Every other type (`docs`, `chore`, `ci`, `refactor`, `test`, …) planned a patch and now releases nothing: it covers the packages it names, as a `name: none` bump file does, so `check --strict` does not report them as changed with no covering intent.

This applies to commits-only intent (`status`, `check`, `version`) and to `generate`, which writes a `name: none` entry for a package only such commits touched, and leaves their summaries out of a note that releases something. A non-releasing commit to a package the config does not version is not reported as "named by a commit but not version-managed". A message that is not conventional still releases a patch for the packages its files touch.
