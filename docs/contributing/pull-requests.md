# Pull Requests

PR bodies follow [`.github/PULL_REQUEST_TEMPLATE.md`](../../.github/PULL_REQUEST_TEMPLATE.md): a `Summary` (problem first, then the change and why this approach), plus a `Notes` section only when something non-obvious earned it — rejected alternatives, accepted limitations, dismissed review findings with their evidence, follow-up beads.

- Squash-merge uses the PR title and body as the mainline commit message, so the body is permanent history — write it commit-worthy and delete the template's guidance comments. The canonical don't-include list lives in the template.
- The PR title is the commit subject: `type(scope): summary`.
- No Claude session links in PR bodies — this overrides the default Claude Code PR-body footer. Put a `Claude-Session` trailer in branch commit messages instead (reachable via the PR's commits tab; squash keeps it out of mainline).
- Do not invent a Test plan checklist. Verification is `mise run check` / `mise run test` and the PR Checks tab — not unchecked markdown.
