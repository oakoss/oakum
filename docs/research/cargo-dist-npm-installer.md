# What cargo-dist's npm installer actually ships

- Date: 2026-08-18, revised 2026-08-19
- Author: Jace Babin
- Scope: whether "a fetcher, not a bundle" describes the npm package [ADR-0021](../decisions/0021-distribute-through-three-channels.md) plans to publish

## Question

[ADR-0021](../decisions/0021-distribute-through-three-channels.md) sends oakum to npm so a JavaScript repository can pin it like any other dev dependency, and [ADR-0018](../decisions/0018-own-the-plan-engine.md) forbids a Node runtime dependency. Those hold together only if the npm package is inert plumbing. Is it?

## Sources

- `dist --version` → `cargo-dist 0.32.0`, the version installed locally
- `axodotdev/cargo-dist`, `cargo-dist/templates/installer/npm/`, listed and read through the GitHub API, 2026-08-18; sizes and `binary.js` re-read 2026-08-19

## Findings

### The package is four JavaScript files, about 13.8 KB

| File | Size |
|---|---|
| `binary-install.js` | 10,247 bytes |
| `binary.js` | 3,324 bytes |
| `install.js` | 78 bytes |
| `run.js.j2` | 175 bytes |

### Every invocation goes through a Node shim

`run.js.j2` is the template for the package's entry point. It renders to three lines: `const { run } = require("./binary");` followed by a call to `run(<bin>)`. The generated `run-<bin>.js` is what the package's `bin` field points at, so `npx oakum` and any `PATH`-resolved call start Node, load the shim, and spawn the real binary from there. The binary is never on `PATH` directly.

### `binary-install.js` is not trivial

Proxy support is the largest single concern: 14 distinct spellings — tokens containing the substring, not all of them identifiers — covering all six env var forms — `http_proxy`/`HTTP_PROXY`, `https_proxy`/`HTTPS_PROXY`, `no_proxy`/`NO_PROXY` — plus `connectThroughProxy`, `getProxyForUrl`, and `noProxyList`. Counting the same way, tar extraction has 3 (`tar` the `spawnSync` argument, `tarballs` in a comment, `untarring` in an error string) and redirect following 2 (`maxRedirects`, `redirects`). A raw grep for `tar` returns 8, but three of those are `target` inside the proxy code.

### `binary.js` does both kinds of libc detection

It resolves a target triple before fetching, and the Linux branch is three-way: `libc.familySync() == "musl"` selects `unknown-linux-musl-dynamic`; `libc.isNonGlibcLinuxSync()` warns *"Your libc is neither glibc nor musl; trying static musl binary instead"* and selects `unknown-linux-musl-static`; otherwise it compares the host's `libc.versionSync()` against a `glibcMinimum` baked in at build time and, on a mismatched major or an older minor, warns *"Your glibc isn't compatible; trying static musl binary instead"* and falls back to static musl again.

So the shim already reaches for a target oakum's build has not committed to — no ADR fixes a target list, and this repository has no `dist-workspace.toml` yet — and it does so by *downgrading* to a static musl artifact, which only helps if that artifact was built. A musl target missing from the eventual `dist-workspace.toml` turns the fallback into a failed lookup on a machine whose glibc is merely too old — and the message names the wrong culprit: `Platform with type "Linux" and architecture "x86_64" is not supported by <name>`, never mentioning glibc.

## Conclusions

**"Fetcher, not bundle" is accurate** in the sense that matters for package size and build topology: the platform binary is downloaded at install time rather than vendored for every target, so one build feeds every channel and the tarball stays small.

**"Contains no JavaScript" is false.** There is a resident wrapper on the hot path of every invocation, plus 10 KB of download-and-extract logic that runs once at install.

**The download makes the npm channel the most network-dependent of the three.** It needs the npm registry *and* whatever host serves the release artifact — two different origins. An environment with an internal npm mirror but no route to the artifact host installs the package and then fails in `postinstall`, which is a more likely failure than a fully offline machine.

## Implications / actions

- The shim is the surface to watch. Nothing that computes a version, resolves a range, or reads a manifest may ever move into it — that is how a distribution wrapper becomes a second implementation.
- If the artifact host ever needs to be configurable for mirrored environments, that is a cargo-dist question rather than an oakum one.

## Open questions

- Whether the install-time download honors an npm-configured proxy in every case, or only the environment variables.
- Whether to build the static musl target purely as a fallback for old-glibc hosts, given the shim reaches for it unprompted.
