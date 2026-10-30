---
oakum: minor
---

### Changed

`oakum ci version-pr` no longer opens a version pull request whose only change is deleting bump files that release nothing, such as a `none`-only bump file from a tests-only change. It prints `nothing to version`, naming how many pending bump files release nothing, and leaves them for the next real release to consume. A version pull request an earlier plan opened is still refreshed to match, so it stops offering a release whose intent left the default branch; a plan that also bumps a package opens the pull request as before.
