---
oakum: patch
---

The workflow `init` and `migrate` print skips `oakum check --strict` on the version pull request only when its author and the event sender are `github-actions[bot]`, the account behind the workflow's `secrets.GITHUB_TOKEN`, instead of any bot. Another bot with write access, such as Renovate or a second App, no longer skips the coverage gate by pushing to `oakum/version-packages`. A workflow that opens the version pull request with an App token names the App's `<slug>[bot]` login there; `ci version-pr` says so on stderr when the pull request's author is another bot. Workflows already pasted are unchanged.
