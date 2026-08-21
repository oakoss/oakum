# Foreign-parser fixtures (`okm-x4u`)

Bodies are generated in-test by `oakum::changeset::write`, then fed to:

- knope's [`changesets`](https://crates.io/crates/changesets) crate (version in
  `crates/oakum/Cargo.toml` `[dev-dependencies]`)
- [`@changesets/parse`](https://www.npmjs.com/package/@changesets/parse)
  (version in this directory's `package.json`). Format gate behind
  `@changesets/cli`; workspace membership is not asserted here.

Assertions require the intended package names, not mere `Ok` / exit 0. Quoted
keys that knope retains are the silent-skip failure mode ADR-0005 guards against.

The integration test copies this directory's `package.json`, lockfile, and
`parse.mjs` into `CARGO_TARGET_TMPDIR` and runs `pnpm install --frozen-lockfile`
there; never into the checkout. Node is mise-pinned (`.mise.toml`).
