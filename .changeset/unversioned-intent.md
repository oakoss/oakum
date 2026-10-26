---
oakum: major
---

`add` refuses, before writing, a bump file that releases a package the config does not version: one `include`/`exclude` leaves out, or a private package without `private-packages.version = true`. `status`, `check` and `version` all refused that file, so it could never release; the refusal now comes while the author is still there, naming the setting. A `name:none` entry is still written, since it releases nothing. `check` now accepts such an entry too: it refused one that `status` and `version` accepted.

With commits-only intent, a commit whose scope names a package the config keeps but cannot version is reported rather than dropped without a word. `status` prints "Named by a commit but not version-managed". `status --json`, the `commit-message` and `title` templates, and the version pull request's body carry it as `named_unmanaged`, and the `ci pr-status` comment lists it. `check`, `version` and `ci version-pr` print it on stderr without gating, `--strict` included, whether or not anything releases. A package already listed as changed but not version-managed is not repeated, and an excluded package stays silent.
