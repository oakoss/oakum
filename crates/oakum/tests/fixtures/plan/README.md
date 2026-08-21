# Plan algorithm fixtures

Snapshot fixtures for the pure planner (`okm-8n5`), in the knope `in/` + `out/` shape.

`compose` (`src/plan/compose.rs`) builds a `Plan` from a workspace, aggregated
intent, and tagged published ranges.

Nothing under `**/in` or `**/out` may be formatted — `.rumdl.toml` excludes both. Captured graphs and expected plans are intentional input, not prose to tidy.

## Layout (as cases land)

```text
plan/
  <case-name>/
    in/     # workspace + intent the planner receives
    out/    # expected plan (or error)
```

Required cases from ADR-0012 (not all present yet): diamond dependencies, two consumers of one package, a transitive chain, cycles that must error, private → public, `version.workspace = true`, and `workspace:*` / `catalog:`.

Bump-level math (`okm-qne`), per-package bump-file aggregation (`okm-4eg`), cascade
eligibility (`okm-3yb` / `okm-tnp`), and the compose walk (`okm-8nu.3`) are covered by
unit tests in `plan/bump.rs`, `plan/aggregate.rs`, `plan/cascade.rs`, and
`plan/compose.rs`. Snapshot fixtures belong here once the harness serializes
workspace and intent into `in/` / `out/`.

