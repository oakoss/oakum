# What cargo-dist's npm installer actually ships

- Date: 2026-08-18
- Author: Jace Babin
- Scope: whether "a fetcher, not a bundle" describes the npm package [ADR-0021](../decisions/0021-distribute-through-three-channels.md) plans to publish

## Question

[ADR-0021](../decisions/0021-distribute-through-three-channels.md) sends oakum to npm so a JavaScript repository can pin it like any other dev dependency, and [ADR-0018](../decisions/0018-own-the-plan-engine.md) forbids a Node runtime dependency. Those hold together only if the npm package is inert plumbing. Is it?

## Sources

- `dist --version` → `cargo-dist 0.32.0`, the version installed locally
- `axodotdev/cargo-dist`, `cargo-dist/templates/installer/npm/`, listed and read through the GitHub API, 2026-08-18

## Findings

### The package is four JavaScript files, about 13.8 KB

| File | Size |
|---|---|
| `binary-install.js` | 10,247 bytes |
| `binary.js` | 3,324 bytes |
| `install.js` | 78 bytes |
| `run.js.j2` | 175 bytes |

### Every invocation goes through a Node shim

`run.js.j2` is the template for the package's entry point. It is three lines: `const { run } = require("./binary");` followed by a call to `run(<bin>)`. The generated `run-<bin>.js` is what the package's `bin` field points at, so `npx oakum` and any `PATH`-resolved call start Node, load the shim, and spawn the real binary from there. The binary is never on `PATH` directly.

### `binary-install.js` is not trivial

Proxy support is the largest single concern (28 references, including `https_proxy`), followed by tar extraction (8), platform and architecture detection, and HTTP redirect following.

## Conclusions

**"Fetcher, not bundle" is accurate** in the sense that matters for package size and build topology: the platform binary is downloaded at install time rather than vendored for every target, so one build feeds every channel and the tarball stays small.

**"Contains no JavaScript" is false.** There is a resident wrapper on the hot path of every invocation, plus 10 KB of download-and-extract logic that runs once at install.

**The download makes the npm channel the most network-dependent of the three.** It needs the npm registry *and* whatever host serves the release artifact — two different origins. An environment with an internal npm mirror but no route to the artifact host installs the package and then fails in `postinstall`, which is a more likely failure than a fully offline machine.

## Implications / actions

- The shim is the surface to watch. Nothing that computes a version, resolves a range, or reads a manifest may ever move into it — that is how a distribution wrapper becomes a second implementation.
- If the artifact host ever needs to be configurable for mirrored environments, that is a cargo-dist question rather than an oakum one.

## Open questions

- Whether `binary.js` performs libc-family or glibc-version detection. `binary-install.js` was read for capabilities; `binary.js` (3,324 bytes) was not read in full, and the distinction matters only for musl targets, which are not in oakum's target list yet.
- Whether the install-time download honors an npm-configured proxy in every case, or only the environment variables.
