---
oakum: patch
---

`init` and `migrate` say `unchanged` for a `_schema.json` that already holds the bundled schema, where they said `replaced`, and `migrate`'s pending line says it will leave such a file alone. After the prompt `migrate` looks at the owned files again and reports under `changed while waiting:` when a README appeared or stopped being oakum's in the meantime.
