---
oakum: minor
---

`check` recognizes the npm channel: an exact `@oakoss/oakum` in `package.json`, a versioned `npm i` / `npm install` / `npm add` / `pnpm add` / `pnpm install` / `pnpm dlx` / `npx @oakoss/oakum@x` workflow line, and an `npm:@oakoss/oakum` mise pin count as install pins; a bare `@oakoss/oakum` is `unverified` as unversioned.
