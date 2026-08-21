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
      intent.json      # aggregated bump levels
      options.json     # optional: cascade-as, versioning
    out/
      plan.json        # expected Plan (stable PackageId order)
```

Omit `range` on a Cargo edge for path-linked (always cascade). Defaults:
`cascade-as: patch`, `versioning: zero-major`. Published ranges for the gate are
the declared ranges in `workspace.json`; `version_at_tag` is each package's
`version` field.

## Cases

| Case | Status |
| --- | --- |
| `diamond` | present |
| `two-consumers` | present |
| transitive chain | TODO |
| cycles that must error | TODO (`Workspace::new`, not compose) |
| private → public | TODO (needs privacy on the model) |
| `version.workspace = true` | TODO |
| `workspace:*` / `catalog:` | TODO |

Bump-level math, aggregation, and cascade eligibility stay in unit tests under
`plan/bump.rs`, `plan/aggregate.rs`, and `plan/cascade.rs`.
