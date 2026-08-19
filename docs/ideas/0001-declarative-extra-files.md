# What if version writes outside a manifest were declared, not scripted?

- Status: draft
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

Some version strings live outside any package manifest. claude-plugins keeps one in each plugin's `plugin.json` and another in a shared `marketplace.json`, and today a Node script (`scripts/sync-plugin-versions.mjs`) copies them into place after the bump. release-please solves the same problem declaratively, with an `extra-files` entry naming a file, a format, and a path within it:

```json
{"type": "json", "path": "plugin.json", "jsonpath": "$.version"}
```

A leading `/` escapes to the repository root, so each package can write its own entry into a file the whole workspace shares — which is exactly the `marketplace.json` shape.

## Why it might matter

- It deletes `scripts/sync-plugin-versions.mjs` entirely, along with the CI drift check that exists to catch it going stale
- It is a purer expression of [ADR-0004](../decisions/0004-derive-facts-configure-preference.md) than the command hook it would replace: the config declares *where a version lives*, rather than scripting *how to put it there*
- A declaration can be validated before anything is written; a script can only be run

## Sketch

Keep the command hook as the escape hatch for genuinely irregular cases, but make the declarative form the documented default. The hook is [ADR-0013](../decisions/0013-no-plugin-runtime.md)'s process boundary — oakum runs a program the user names, hands it JSON on stdin, and reads JSON back — so this idea proposes a declarative alternative to an existing surface rather than a new one. The formats worth supporting first are JSON with jsonpath and TOML with a dotted key, since those cover every case in the repositories surveyed.

Writing has to preserve formatting exactly — see [implementation stack](../research/implementation-stack.md) for why `jsonc-parser` with the `cst` feature and `toml_edit` with decor restoration are the crates that do that.

## Open questions

- Whether `jsonc-parser`'s CST API supports arbitrary jsonpath edits or only simple key replacement. If only the latter, the feature shrinks to "a dotted key path", which may still be enough.
- What happens when two packages declare a write to the same shared file in one release. Ordering must be deterministic, and the result must not depend on which package was planned first.
- Whether a declared target that does not exist is an error or a skip. Erroring is consistent with the rest of the design, but it makes adding a package to a marketplace a two-step change.

## Related work

- [ADR-0013](../decisions/0013-no-plugin-runtime.md) — the process-boundary hook this would sit in front of
- release-please's `extra-files`, which is the prior art being borrowed
- `scripts/sync-plugin-versions.mjs` in claude-plugins, the thing this replaces
