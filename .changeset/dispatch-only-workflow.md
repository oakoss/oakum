---
oakum: major
---

### Fixed

`release` no longer refuses to tag when a workflow runs only on `workflow_dispatch` and no workflow listens for tags. Nothing in the repository says such a file, a review bot's or a manual chore's, belongs to the release, yet `release` refused with `downstream .github/workflows/pullfrog.yml … is workflow_dispatch; a tag push will not start it` and left the release untagged. It now tags, and names the file as one a tag push does not start, so a dispatch-only workflow that builds the release can be dispatched with the new tag.

### Changed

`release` refuses before tagging when the release workflow dist generated for `dispatch-releases = true` is present, recognized by dist's header on its first line, its `workflow_dispatch` trigger and its `gh release create` in this repository (not in the other repository a `github-releases-repo` names), since that workflow creates the tag and GitHub release itself and a tag oakum pushes first collides with it. The refusal names the workflow and says to dispatch it with the tag instead. This includes dist's default output, which also runs on `pull_request`, and a repository where another workflow listens for tags; both tagged before. A workflow whose dist header was edited away is not recognized, nor one from dist before 0.21.1, which wrote no header; regenerate it with a current `dist init`.
