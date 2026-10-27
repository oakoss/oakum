---
oakum: major
---

`oakum release` tags the commit that set each package's pending version, not `HEAD`: it walks back along `HEAD`'s first parents to where the manifest (or, for `version.workspace = true`, `[workspace.package]`) began declaring the version. A commit that lands between the version commit and a delayed `release` now stays after the tag and is planned in the next release, rather than shipping inside a tag no plan covered. A version a merge brought in is tagged at the merge, and a package created at its version is tagged where its manifest appeared.

`release` now refuses as unverified (exit 2) where it used to tag `HEAD`: when the walk reaches a manifest that names no version oakum can read before it finds where the version was set, when the newest commit to change the manifest declares another version, or when the commit that set the version lacks the changelog section `HEAD` has for it, as when a version was bumped by hand before adopting oakum. Tag the release by hand and run `release` again to publish it. A working tree holding changes is refused before the walk. The skip-ci refusal now says that a later commit does not move the tag: tag a clean commit by hand, or start the downstream workflow by hand.

The release body follows a symlinked changelog in the tagged commit rather than falling back to the title, and a `+build` release takes its body from a bare `## <version>` heading, as `check` already matches it. A link that loops or leaves the repository still falls back to the title, and the notice now names which.
