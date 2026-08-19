# Implementation stack: manifest editing, HTTP, and commit parsing

- Date: 2026-08-18
- Author: Jace Babin
- Scope: which crates oakum should use for the jobs it cannot avoid — rewriting user manifests without damaging them, talking to the GitHub API, and parsing conventional commits

## Question

Oakum rewrites files people wrote by hand. A version bump that reformats an untouched part of `package.json` produces a diff nobody asked for and erodes trust in every subsequent release. Which crates preserve formatting exactly, and where are the traps?

## Sources

- `jsonc-parser` and `toml_edit` crate documentation and source, probed against constructed fixtures 2026-08-18
- knope, release-plz, cargo-dist source
- crates.io metadata for dependency counts

## Findings

### JSON: `jsonc-parser` with the `cst` feature

Measured byte-identical round-trip including comments, CRLF line endings, a trailing newline, and the nested `plugins[].version` shape that `marketplace.json` uses. Zero extra transitive dependencies. Maintained by dprint, which is already in this repository's toolchain.

The alternative is what knope does: parse to `serde_json` with `preserve_order`, then re-emit with `to_string_pretty`. That preserves key order but **normalizes all whitespace and drops the trailing newline** — every release would produce a whole-file diff on any hand-formatted JSON.

### TOML: `toml_edit` 0.25.x

Cargo itself depends on it, which is the strongest available signal for Cargo manifest fidelity.

**The trap:** `*item = value(v)` **resets the decor**. Decor is the crate's term for the trivia around a value — surrounding whitespace and trailing comments. Assigning through it deletes a trailing comment and collapses padding, silently. Clone `decor()` before assigning and restore it after.

### GitHub API: hand-rolled `reqwest` + `serde_json`, not `octocrab`

No major Rust release tool uses octocrab. release-plz deliberately removed it, and its commit message says the removal "removes 18 dependencies". Oakum needs a handful of endpoints — create a commit via GraphQL, push a tag, create a release, poll check runs — which is well under the threshold where a client library pays for itself.

### Conventional commits: `git-conventional` 1.1.0

The ecosystem default. Note that its comparisons are **case-insensitive**, which matters when deciding whether `Feat:` and `feat:` are the same type for changelog grouping.

### Templates

Covered separately in [templating prior art](templating-prior-art.md). The short version: minijinja 2.x with `UndefinedBehavior::SemiStrict` — **not `Strict`**, which errors on `{% if undefined %}` and would force `is defined` guards through every user template.

## Conclusions

Formatting fidelity is a crate-selection problem, not an implementation problem. Both chosen crates preserve input exactly; both alternatives lose something quietly. The `toml_edit` decor reset is the one place where the right crate still produces the wrong result if used naively.

## Implications / actions

- Any manifest-writing code path needs a round-trip test on a hand-formatted fixture: comments, CRLF, trailing newline, unusual indentation. A diff on an untouched region is a bug of the same severity as a wrong version.
- Wrap `toml_edit` assignment in a helper that preserves decor, so the trap is handled once rather than at each call site.
- Do not add octocrab later "for convenience" without re-measuring the dependency cost.

## Open questions

- Whether `jsonc-parser`'s CST API covers every edit shape needed for release-please-style `extra-files` with jsonpath, or only the simple key-replacement case.
