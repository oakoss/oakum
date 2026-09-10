---
oakum: patch
---

`check` finds the tool-version pin in local composite actions, matrix cells, `with.*` inputs other than `tool`, `with.tool` arrays, and `workflow_call` input defaults. It previously read only workflow `run`, `tool`, and `with.tool` strings, so a pin drifting at any of those sites still verified `ok`. Invalid composite YAML and a non-object `with` fail closed; a step whose command comes only from `${{ matrix.* }}` is documented as unsupported.
