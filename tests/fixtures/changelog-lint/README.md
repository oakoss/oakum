# Captured artifacts: a generated changelog and the linter that rejects it

Evidence for [changelog-lint-collision.md](../../../docs/research/changelog-lint-collision.md),
which carries the findings. These are the files themselves.

| File | What it is |
|---|---|
| `in/CHANGELOG.md` | exactly what `@varlock/bumpy` 1.18.1 generated, byte for byte |
| `.markdownlint-cli2.yaml` | `oakoss/claude-plugins`' own config, copied unchanged |
| `out/CHANGELOG.md` | what `markdownlint-cli2 --fix` produces from that pair |

Captured 2026-08-19 from `oakoss/claude-plugins` pull request
[#26](https://github.com/oakoss/claude-plugins/pull/26), branch
`bumpy/version-packages`, CI run `32224322374`. Kept because the reproduction is
temporary: once that pull request merges the branch is gone and CI logs expire.

`diff in/ out/` is `10a11`, one inserted blank line.

**Do not format anything here.** `in/` is malformed on purpose and `out/` is
another tool's exact output; formatting either erases what they record.
`.rumdl.toml` excludes both directories for that reason, and the research
document explains what happened when it did not.
