# pnpm discovery fixtures

Real package trees for `discover_pnpm` tests. Call pnpm rather than inventing
list JSON so the stray-ancestor probe and list shape match production.

Do not run `pnpm install` here; discovery must stay read-only.
