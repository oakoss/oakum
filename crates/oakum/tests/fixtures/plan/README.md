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
| private → public | deferred — needs privacy on the package model (`okm-8nu.2`) |

Bump-level math, aggregation, and cascade eligibility stay in unit tests under
`plan/bump.rs`, `plan/aggregate.rs`, and `plan/cascade.rs`.
