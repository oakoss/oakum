---
oakum: minor
---

### Changed

The workflow `init` and `migrate` print for an npm workspace runs the repository's own pin: `pnpm install --frozen-lockfile --ignore-scripts`, then `pnpm exec oakum`, in place of `npm i -g @oakoss/oakum@<version>`, so the `@oakoss/oakum` devDependency is the one install pin and no install script runs in a job. When the root `package.json` lists no `@oakoss/oakum`, both commands name the `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@<version>` that adds it, whatever else pins oakum. Every printed job but `release` checks out with `persist-credentials: false`.
