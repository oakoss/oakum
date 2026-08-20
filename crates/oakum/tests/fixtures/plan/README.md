# Plan algorithm fixtures

Snapshot fixtures for the pure planner (`okm-8n5`), in the knope `in/` + `out/` shape.

Nothing under `**/in` or `**/out` may be formatted — `.rumdl.toml` excludes both. Captured graphs and expected plans are intentional input, not prose to tidy.

## Layout (as cases land)

```text
plan/
  <case-name>/
    in/     # workspace + intent the planner receives
    out/    # expected plan (or error)
```

Required cases from ADR-0012 (not all present yet): diamond dependencies, two consumers of one package, a transitive chain, cycles that must error, private → public, `version.workspace = true`, and `workspace:*` / `catalog:`.

Bump-level math (`okm-qne`) is covered by unit tests in `plan/bump.rs` rather than snapshots; cascade cases go here once the planner emits a plan.
