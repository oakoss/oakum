---
oakum: patch
---

A release `release` creates no longer takes GitHub's Latest from another package or from a newer version: it takes Latest only when no release holds it, or the holder is an earlier version of the same package, and never as a prerelease. In a multi-package repository, a library release, or a hand-made tag `release` resumes, used to become Latest by GitHub's default, breaking `releases/latest/download/…` URLs such as a binary's installer. When Latest stays elsewhere, `release` says so on stderr with the `gh release edit` command that moves it. When the current holder cannot be read, the release is still created, without taking Latest, and `release` says that too.
