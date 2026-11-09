---
oakum: minor
---

### Changed

The workflow `init` and `migrate` print for an npm workspace runs the repository's own pin: `pnpm install --frozen-lockfile --ignore-scripts`, then `pnpm exec oakum`, in place of `npm i -g @oakoss/oakum@<version>`, so the `@oakoss/oakum` devDependency is the one install pin and no install script runs in a job. When the root `package.json` lists no `@oakoss/oakum`, both commands name the `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@<version>` that adds it, whatever else pins oakum. Every printed job but `release` checks out with `persist-credentials: false`, and every printed action is pinned by commit SHA with its tag as a comment (`actions/checkout@<sha> # v7.0.1`), the commit its latest release tags, at one more GitHub read per action. The pull-request comment is split across two jobs, as the GitHub Actions guide's "Posting the comment from another job" describes: `check` runs `ci pr-status --emit-comment` with a read-only token and uploads the comment, and a new `post` job checks out the base commit, installs oakum, and runs `ci post-comment` with the write token, skipping a fork.
