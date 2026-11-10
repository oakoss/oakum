---
oakum: minor
---

### Added

`init` and `migrate` take `--token <SECRET>`, which wires the printed workflow's writing steps and the release checkout to `${{ secrets.<SECRET> || github.token }}`, and `--app`, which prints a GitHub App token minted in each writing job from the `APP_CLIENT_ID` and `APP_PRIVATE_KEY` secrets, with the same fallback and a release job that tags as the App's bot. Under either, the version pull request is skipped by `check --strict` by its branch and repository, since oakum cannot know the account that opens it, so the workflow runs as printed. Each names, as a next step, what the workflow needs that oakum cannot check. The two cannot be combined, and an already-initialized or already-migrated rerun, which prints no workflow, refuses either.
