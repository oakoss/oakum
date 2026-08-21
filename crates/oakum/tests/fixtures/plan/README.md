# Plan algorithm fixtures

Snapshot fixtures for the pure planner (`okm-8n5`), in the knope `in/` + `out/` shape.

`compose` (`src/plan/compose.rs`) builds a `Plan` from a workspace, aggregated
intent, and tagged published ranges. The harness is `tests/plan_fixtures.rs`:
every `plan/<case>/{in,out}/` pair is loaded and compared.

Nothing under `**/in` or `**/out` may be formatted — `.rumdl.toml` excludes both.
Captured graphs and expected plans are intentional input, not prose to tidy.

## Layout

```text
plan/
  <case-name>/
    in/
      workspace.json   # packages + edges
      intent.json      # required for plan cases
      options.json     # optional: cascade-as, versioning
    out/
      plan.json        # expected Plan (stable PackageId order)
      # — or —
      error.json       # expected Workspace::new refusal (cycles)
```

Omit `range` on a Cargo edge for path-linked (always cascade). Protocol shapes:

```json
"range": "^0.1.3"
"range": { "workspace-tracking": "exact" }
"range": { "workspace": "^1.5.0" }
"range": { "catalog": { "bounds": "^1.5.0" } }
```

Defaults: `cascade-as: patch`, `versioning: zero-major`. Published ranges for the
gate are the declared ranges in `workspace.json`; `version_at_tag` is each
package's `version` field.

## Cases (ADR-0012)

| Case | Status |
| --- | --- |
| `diamond` | present |
| `two-consumers` | present |
| `transitive-chain` | present |
| `cycle` | present (`out/error.json`) |
| `version-workspace` | present (Cargo: discovery resolves `version.workspace = true` to a plain range; patch still admits → library omitted) |
| `workspace-star-catalog` | present (`workspace:*` and catalog exact pin both cascade on patch) |
| `linesmith-silent-miss` | present (`okm-vio`: core `0.4.0`→`0.4.1` inside `^0.4.0`; binary Always — ADR-0009) |
| `linesmith-feat-miss` | present (`okm-vio`: core minor `0.2.0`→`0.3.0` excludes `^0.2.0`; cascade via ADR-0010 + Always path) |
| `linesmith-plugin-patch` | present (`okm-vio`: plugin patch bumps binary Always; core library stays quiet) |
| `linesmith-dev-edge` | present (`okm-vio`: development edge must not cascade to the binary) |
| private → public | deferred: needs privacy on the package model (`okm-8nu.2`) |

### linesmith silent misses (`okm-vio`)

The abort bar (ADR-0012 / ADR-0009): predict every undelivered binary release,
with no false positive on a library and no cascade on a development edge.

Two planner shapes cover the eight historical events (oakoss/linesmith):

| # | Event | Shape | Fixture |
| --- | --- | --- | --- |
| 1–7 | `feat(core)` into core 0.3.0 while binary stayed 0.2.1 | minor / range **excludes** (`^0.2.0` ↛ `0.3.0`) → cascade (ADR-0010; binary also takes the Always path) | `linesmith-feat-miss` |
| 8 | 403 classification fix in core 0.4.1 | patch / range **admits** (`^0.4.0` still covers `0.4.1`); **Always is decisive** (ADR-0009) | `linesmith-silent-miss` |

Evidence: `lsm-w5xm` PRs #21, #23, #24, #25, #27, #29, #30; ADR-0032 / PR #48.

`linesmith-plugin-patch` is the false-positive check: core is an install-time
library whose caret still admits, so only the binary cascades.
`linesmith-dev-edge` is the ADR-0008 refusal.

Bump-level math, aggregation, and cascade eligibility stay in unit tests under
`plan/bump.rs`, `plan/aggregate.rs`, and `plan/cascade.rs`. Decision traces for
`--explain` live in unit tests under `plan/explain.rs`; the CLI flag ships with
`check`.
