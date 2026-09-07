# What cargo-dist's npm installer actually ships

- Date: 2026-08-18, revised 2026-08-19; published-package check 2026-09-07
- Author: Jace Babin
- Scope: whether "a fetcher, not a bundle" describes the npm package [ADR-0021](../decisions/0021-distribute-through-three-channels.md) publishes

## Question

[ADR-0021](../decisions/0021-distribute-through-three-channels.md) sends oakum to npm so a JavaScript repository can pin it like any other dev dependency, and [ADR-0018](../decisions/0018-own-the-plan-engine.md) forbids a Node runtime dependency. Those hold together only if the npm package is inert plumbing. Is it?

## Sources

- `dist --version` → `cargo-dist 0.32.0`, the version installed locally (2026-08-18)
- `axodotdev/cargo-dist`, `cargo-dist/templates/installer/npm/`, listed and read through the GitHub API, 2026-08-18; sizes and `binary.js` re-read 2026-08-19
- Published `@oakoss/oakum@0.1.4` on the npm registry, 2026-09-07: `npm view @oakoss/oakum@0.1.4 --json`, `npm pack @oakoss/oakum@0.1.4 --dry-run`, and reading `install.js` / `binary.js` / `binary-install.js` from the unpacked tarball
- This repo's [dist-workspace.toml](../../dist-workspace.toml): `installers` / `publish-jobs` include `npm`, `npm-scope = "@oakoss"`

## Findings

### Published `@oakoss/oakum@0.1.4` is a fetcher

Measured 2026-09-07:

| Fact | Observed |
|---|---|
| Registry name | `@oakoss/oakum` |
| `bin.oakum` | `run-oakum.js` |
| `scripts.postinstall` | `node ./install.js` |
| `artifactDownloadUrls` | `https://github.com/oakoss/oakum/releases/download/v0.1.4` |
| `preferUnplugged` | `true` |
| Runtime dependency | `detect-libc` only |
| `npm pack --dry-run` package size | 8.9 kB (11 files, unpacked 24.7 kB) |

Tarball contents (no platform binaries): `.gitignore`, `CHANGELOG.md`, `LICENSE`, `README.md`, `binary-install.js` (10.2 kB), `binary.js` (3.3 kB), `install.js` (78 B), `npm-shrinkwrap.json`, `package.json`, `run-oakum.js` (72 B).

That matches ADR-0021: the registry package is plumbing. Reading the published JS confirms the download: `install.js` calls `install(false)` from `binary.js`; `binary.js` builds `url` as `` `${artifactDownloadUrl}/${platform.artifactName}` `` (so `https://github.com/oakoss/oakum/releases/download/v0.1.4/…`) and `Package.install` downloads that URL (`binary-install.js`).

### Upstream templates (cargo-dist 0.32.0)

Still useful for what cargo-dist generates before publish:

| File | Size |
|---|---|
| `binary-install.js` | 10,247 bytes |
| `binary.js` | 3,324 bytes |
| `install.js` | 78 bytes |
| `run.js.j2` | 175 bytes |

### Every invocation goes through a Node shim

`run.js.j2` is the template for the package's entry point. It renders to three lines: `const { run } = require("./binary");` followed by a call to `run(<bin>)`. The generated `run-oakum.js` is what the package's `bin` field points at, so `pnpm exec oakum` / `npx @oakoss/oakum` start Node, load the shim, and spawn the real binary from there. The binary is never on `PATH` directly from the npm package.

### `binary-install.js` is not trivial

Proxy support is the largest single concern: 14 distinct spellings — tokens containing the substring, not all of them identifiers — covering all six env var forms — `http_proxy`/`HTTP_PROXY`, `https_proxy`/`HTTPS_PROXY`, `no_proxy`/`NO_PROXY` — plus `connectThroughProxy`, `getProxyForUrl`, and `noProxyList`. Counting the same way, tar extraction has 3 (`tar` the `spawnSync` argument, `tarballs` in a comment, `untarring` in an error string) and redirect following 2 (`maxRedirects`, `redirects`). A raw grep for `tar` returns 8, but three of those are `target` inside the proxy code.

### `binary.js` does both kinds of libc detection

It resolves a target triple before fetching, and the Linux branch is three-way: `libc.familySync() == "musl"` selects `unknown-linux-musl-dynamic`; `libc.isNonGlibcLinuxSync()` warns *"Your libc is neither glibc nor musl; trying static musl binary instead"* and selects `unknown-linux-musl-static`; otherwise it compares the host's `libc.versionSync()` against a `glibcMinimum` baked in at build time and, on a mismatched major or an older minor, warns *"Your glibc isn't compatible; trying static musl binary instead"* and falls back to static musl again.

oakum's [dist-workspace.toml](../../dist-workspace.toml) builds `x86_64-unknown-linux-musl` among its targets, so the shim's musl fallback can resolve for that triple. A host that needs `unknown-linux-musl-static` (or another triple not in `targets`) still fails with a platform-unsupported message that does not name glibc.

## Conclusions

**"Fetcher, not bundle" is accurate** for the published package: no platform binaries in the npm tarball; install downloads from the release artifact host.

**"Contains no JavaScript" is false.** There is a resident wrapper on the hot path of every invocation, plus ~10 KB of download-and-extract logic that runs once at install.

**The download makes the npm channel the most network-dependent of the three.** It needs the npm registry *and* the GitHub release host — two different origins. An environment with an internal npm mirror but no route to the artifact host installs the package and then fails in `postinstall`.

## Implications / actions

- The shim is the surface to watch. Nothing that computes a version, resolves a range, or reads a manifest may ever move into it — that is how a distribution wrapper becomes a second implementation.
- Watch surface: cargo-dist upgrades (`cargo-dist-version` / regenerated npm artifacts) and review of published `@oakoss/oakum` contents — not a separate CI gate invented for this invariant.
- If the artifact host ever needs to be configurable for mirrored environments, that is a cargo-dist question rather than an oakum one.

## Open questions

- Whether the install-time download honors an npm-configured proxy in every case, or only the environment variables.
- Whether to add `unknown-linux-musl-static` (or other shim fallback triples) to `targets` if old-glibc hosts matter in practice.
