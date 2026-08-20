# npm ranges versus Cargo's `VersionReq`

- Date: 2026-08-20
- Author: Jace Babin
- Scope: Which dependency range forms oakum must accept that `semver::VersionReq`
  cannot parse or silently misreads, and what that implies for `DeclaredRange`
  under `no_std` + `alloc`.

## Question

`DeclaredRange` stores plain bounds as `semver::VersionReq` (Cargo's grammar).
Discovery will read npm and Cargo manifests. Which legal forms fail that parser,
which parse with the wrong meaning, and which approaches fit ADR-0024?

## Sources

- `semver` **1.0.28** (workspace lockfile), probe at `/tmp/oakum_semver_probe`,
  2026-08-20: `VersionReq::parse` on the forms below
- npm CLI docs, package.json dependencies
  (<https://docs.npmjs.com/cli/v10/configuring-npm/package-json#dependencies>),
  read 2026-08-20 — bare `version` "Must match version exactly";
  `range1 || range2`; space-separated and hyphen examples in the same section
- node-semver (<https://github.com/isaacs/node-semver>) — npm's reference grammar
- pnpm workspaces (<https://pnpm.io/workspaces>) — `workspace:../foo` form
- [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md),
  [ADR-0018](../decisions/0018-own-the-plan-engine.md),
  [ADR-0024](../decisions/0024-no-std-plan-crate.md),
  [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)
- In-repo: `cargos_grammar_reads_a_bare_version_as_a_caret` in
  `crates/oakum/src/plan/workspace.rs`; path `req=*` shape in
  [cargo metadata edge shapes](cargo-metadata-edge-shapes.md)
- Scratch workspace `/tmp/oakum_path_req_probe`, `cargo metadata --no-deps`,
  2026-08-20: path-only vs `path` + `version = "*"`
- Survey of release-tool cascade implementations, 2026-08-20 (primary sources):
  changesets `assemble-release-plan` / `apply-release-plan` (`semver.satisfies`);
  `@varlock/bumpy` 1.18.1 dist `satisfies()`; knope / release-plz / release-please
  as noted under Findings
- Rust crate `js-semver` **0.3.0**:
  <https://github.com/ryuapp/js-semver/blob/v0.3.0/Cargo.toml>,
  <https://github.com/ryuapp/js-semver/blob/v0.3.0/src/lib.rs>,
  read 2026-08-20 — `#![cfg_attr(not(feature = "std"), no_std)]`,
  `extern crate alloc`, default feature `std`, categories include `no-std`,
  MSRV 1.85 (below oakum's 1.97.1)

## Findings

### Three npm forms `VersionReq` rejects

Against semver 1.0.28:

| Form | Example | Result |
| --- | --- | --- |
| Union `\|\|` | `^18 \|\| ^19` | ERR: expected comma after major, found `\|` |
| Space conjunction | `>=1.2.3 <2.0.0` | ERR: expected comma after patch, found `<` |
| Hyphen range | `1.2.3 - 2.3.4` | ERR: expected comma after patch, found `-` |
| Cargo comma (contrast) | `>=1.2.3, <2.0.0` | OK |

npm documents all three as ordinary dependency specs. Peer ranges routinely use
`||`. Discovery that drops a rejected range as "cannot check" under-releases
exactly as ADR-0010 forbids for unread `catalog:`.

`workspace:../foo` also fails `VersionReq::parse` (`unexpected character 'w'`).
That is a relative-path protocol form (pnpm), not a semver range; it must not
be fed to Cargo's parser. Existing `DeclaredRange` protocol arms cover
`workspace:*` / tracking and `catalog:`, not relative `workspace:…` paths; those
need an explicit arm or a discovery refusal, separate from the range-grammar
problem.

### Bare and partial versions: npm vs Cargo

Probe: `VersionReq::parse("1.5.0")` displays as `^1.5.0` and matches `1.6.0`;
`=1.5.0` does not. The unit test `cargos_grammar_reads_a_bare_version_as_a_caret`
pins this. npm docs: bare `version` must match exactly. ADR-0018 already names
the mismatch. Feeding npm text through `VersionReq::parse` widens the pin, the
range gate stays satisfied, and the cascade never fires.

Partial versions are a second silent widening. Against semver 1.0.28,
`VersionReq::parse("1.2")` displays as `^1.2` and matches `1.3.0`. npm's
`1.2` is an x-range (`>=1.2.0 <1.3.0`) and does **not** match `1.3.0`
(node-semver / npm package.json docs). Same failure mode as the bare pin:
Cargo's parse stays satisfied when npm's would not.

`DeclaredRange::Plain(VersionReq)` cannot tell "translated npm exact" from
"raw Cargo parse of bare text." Before ecosystem constructors, a doc comment
was the only guard.

### Path-linked edges with no declared version

Scratch metadata (`cargo metadata --no-deps`):

- `a = { path = "../a" }` → `req: "*"` (and a `path`)
- `a = { path = "../a", version = "*" }` → also `req: "*"`

Metadata alone cannot distinguish omitted version from an authored star. The
self-dev-dependency case in [cargo metadata edge shapes](cargo-metadata-edge-shapes.md)
already shows `req=*` for `path = "."`. Rewriting a path-only edge by inserting
a `version` key would be an unrequested edit (ADR-0003). Discovery must read
whether the TOML carried a `version` key; the planner needs a **bounds-free**
arm, not fabricated `Plain(*)`.

Treating path-only as `*` under ADR-0010 makes the library gate always pass →
silent under-release. Cascade for that arm is decided in ADR-0026: always
cascade (bounds-free `PathLinked`); do not refuse to plan.

Cargo `{ workspace = true }` inheritance is erased before metadata (same research
note) and must not be conflated with pnpm's `workspace:` protocol.

### What other release tools do

Cascade here means “bump the dependent package,” not only rewrite a dep string.

| Tool | Range check | Lesson |
| --- | --- | --- |
| **changesets** | node `semver.satisfies`; strip `workspace:`; `*` → exact old version; `^`/`~` compose with old | ADR-0010 reference — resolve protocols, then satisfies |
| **bumpy** 1.18.1 | same library, but `workspace:*` and `catalog:` short-circuit to **satisfied** | Do not copy; unread protocols must error or resolve |
| **knope** | Rewrites dep strings; no range-gated bump of the dependent | Rewrite ≠ cascade |
| **release-plz** | Cascades when Cargo `upgrade_requirement` would change the req **text** | Satisfied carets still cascade — not oakum’s gate |
| **release-please** (node workspace) | Force-bumps graph; no `satisfies` | Delivery-style, not range-derived |

Steal the **algorithm** from changesets (protocol peel → expand → satisfies). Do not
import their packages into `plan` (Node / not `no_std`).

### Approaches under ADR-0024

| Approach | Covers npm grammar / bare | `no_std` + `alloc` fit |
| --- | --- | --- |
| Hand-roll ecosystem-aware `Bounds` (constructors `from_cargo_text` / `from_npm_text`; unions as OR-of-AND clauses; match via workspace `semver`) | Yes, if storage is not a single `VersionReq` | Known fit — channel ADR-0024 already accepts |
| `Bounds(VersionReq)` only | Bare/partial via constructors; **unions no** | Fine, insufficient for peers |
| Rust `js-semver` 0.3.0 (npm grammar, `Range::satisfies`) | Yes (claims node-semver semantics) | Verified `no_std`+`alloc` in sources above; later confirmed by `plan-no-std` before ADR-0026 |
| Rust `node-semver` / `semver_rs` | Aimed at npm | Survey: std-heavy — skip for `plan` |

A single `VersionReq` cannot hold `^18 || ^19`. Constructor discipline alone
fixes the bare/partial pin holes; union storage needs a separate shape (e.g. list
of clauses) **unless** matching is delegated to `js-semver::Range`.

## Conclusions

Cargo's `VersionReq` is the wrong *storage* type for npm bounds, even though it
remains a good *matching* engine for a single Cargo clause. oakum needs
(1) ecosystem constructors so bare and partial npm never become carets,
(2) a representation for disjunction (or an npm-faithful matcher),
(3) a bounds-free edge for path-only / no declared range, and
(4) discovery that refuses to drop unparsable peer ranges, and that resolves
`workspace:` / `catalog:` instead of waiving them (changesets yes, bumpy no).

## Implications / actions

- ADR-0026 (accepted 2026-08-20): depend on `js-semver` 0.3 with
  `default-features = false`; path-linked edges always cascade via
  `DeclaredRange::PathLinked`.
- Implementation on branch `feat/declare-range-bounds` (`okm-6re`): `Bounds`,
  ecosystem constructors, protocol arms carry `Bounds`, then `okm-tnp` gates on
  `admits`.
- `catalog:default` vs `catalog:` normalization remains unconfirmed against a
  primary source (see `okm-6re` notes); not blocking the grammar decision.

## Open questions

- Whether `catalog:default` and bare `catalog:` are the same catalog (pnpm /
  yarn / bun primary sources)
- Relative `workspace:../foo` — explicit arm vs discovery refusal (ADR-0026)

## Raw data

```text
# semver 1.0.28 VersionReq::parse (2026-08-20)
ERR	^18 || ^19	expected comma after major version number, found '|'
ERR	>=1.2.3 <2.0.0	expected comma after patch version number, found '<'
ERR	1.2.3 - 2.3.4	expected comma after patch version number, found '-'
OK	>=1.2.3, <2.0.0	>=1.2.3, <2.0.0
OK	1.5.0	^1.5.0
OK	=1.5.0	=1.5.0
ERR	workspace:../foo	unexpected character 'w' while parsing major version number
bare_matches_1.6.0	true
exact_matches_1.6.0	false

# partial / x-range displays (same probe)
OK	1.2	^1.2	1.2.0=true	1.3.0=true	2.0.0=false
OK	1	^1	1.2.0=true	1.3.0=true	2.0.0=false
OK	1.2.x	1.2.*	1.2.0=true	1.3.0=false	2.0.0=false
```
