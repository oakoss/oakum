---
oakum: major
---

`oakum version` no longer bumps past a version someone set by hand. When the plan would bump a package, cascades included, whose manifest version has no tag and no changelog section, such as a `0.1.1` or `0.0.5` someone typed, `version` and `ci version-pr` exit 1 before writing anything and name every such package. The fix is to tag the version you meant, or to run `git fetch --tags` if its tag exists on a remote; `check` now gives that second option too. A hand-set version the plan leaves alone does not stop the run; `check` still reports it. A placeholder `0.0.0` or `0.1.0` is still bumped from, and an untagged version that has its changelog section (one `version` wrote and `release` has not tagged yet) is still stacked on. When the changelog or the tags cannot be read for such a package, `version` exits 2 rather than guess, so a shallow or no-tags clone now stops it whenever a package it bumps has no changelog section for its current version.
