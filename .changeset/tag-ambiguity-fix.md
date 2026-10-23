---
oakum: patch
---

The `leftover tag ambiguity` refusal from `check`, `version` and `release` now says how to resolve it, on a line after the tags it names:

- Two versions of one package tagged on one commit: keep the tag for the version that commit's manifest names and delete the other, on the remote too if it was pushed.
- A bare `v<version>` tag with several packages tagged: use `<package>/v<version>`.
- A version-like tag in a shape oakum does not read: rename it to one it does.
- A tag naming a package the workspace no longer has, as after a rename: delete it, or tag the release with a current package's name.
- A tag whose package name exists in two ecosystems: rename the package it did not release.

The refusal and its exit code are unchanged, and so is the `summary` of its `check --json` refusal, which keeps the first line only.
