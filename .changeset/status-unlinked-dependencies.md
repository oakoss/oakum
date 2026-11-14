---
oakum: minor
---

### Added

`oakum status` lists, under "Named like a member, installed from the registry", each pnpm dependency named like a workspace member whose range, or catalog range, the member's version does not satisfy. pnpm installs those from the registry, so oakum cascades nothing along them and rewrites none of their ranges; write `workspace:` where the member is meant. `status --json` carries the same list as `unlinked`. Left out are a declaration pnpm sets aside for another section's, a key a `pnpm-workspace.yaml` override points at the workspace, a peer under `autoInstallPeers: false`, an optional dependency `ignoredOptionalDependencies` names, and a package the config excludes. Pull request comments and job summaries do not show it, since a deliberate registry pin would be reported on every run.
