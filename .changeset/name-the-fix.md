---
oakum: patch
---

Four messages now say what to do:

- A version no tag records (`never released, but the manifest is 1.0.0`) names the tag to cut, rendered from `tag-format` or the default: `` `git tag @demo/a/v1.0.0 <commit>` ``. A `tag-format` whose tag oakum could not read back as that version names none.
- In `oakum check`, a change with no covering intent names `oakum add --packages <package>:<level>`, and, when conventional commits are on, `oakum generate --from <base>` over the range `check` read.
- `oakum add` without `--message` says the bump file's changelog entry will be empty.
- `oakum init` and `oakum migrate` say why they reach GitHub when the lookup for the printed workflow's action pins fails, and that nothing was written.
