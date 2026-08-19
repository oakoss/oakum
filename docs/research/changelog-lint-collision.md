# When a generated changelog fails the repository's own linter

- Date: 2026-08-19
- Author: Jace Babin
- Scope: a live collision between a release tool's generated markdown and the linter of the repository it writes into, captured before the evidence expires

## Question

[okm-v2y] asks how oakum's generated markdown should relate to a project's formatter and linter. A real instance exists rather than a hypothetical: what exactly collides, and what does the collision imply for a tool that writes changelogs into repositories it does not control?

## Sources

- `oakoss/claude-plugins` pull request [#26](https://github.com/oakoss/claude-plugins/pull/26), branch `bumpy/version-packages`, CI run `32224322374`, read 2026-08-19
- `@varlock/bumpy` 1.18.1, which generated the changelog. claude-plugins' install carries `patches/@varlock__bumpy@1.18.1.patch`; it touches only `findUnpublishedPackages` on the publish path, so changelog generation is stock
- `markdownlint-cli2` 0.23.2 with `markdownlint` 0.41.1, pinned in claude-plugins' `pnpm-lock.yaml`
- `rumdl` 0.2.57, this repository's own markdown formatter
- Artifacts kept verbatim in [`tests/fixtures/changelog-lint/`](../../tests/fixtures/changelog-lint/)

## Findings

### The failure

```text
plugins/review-cycle/CHANGELOG.md:10 error MD022/blanks-around-headings
Headings should be surrounded by blank lines [Expected: 1; Actual: 0; Below]
[Context: "## 0.15.0"]
```

bumpy emits the version heading with its date immediately beneath:

```markdown
## 0.15.0
<sub>2026-08-19</sub>
```

`markdownlint-cli2 --fix` resolves it by inserting one blank line. The whole diff between the captured input and output is `10a11`.

**That the fixer inserts a line rather than stripping the `<sub>` is mechanics, not judgment.** MD022's only defined fix is blank-line insertion, and MD033 — inline HTML — has no fix at all, so removing the tag was never available under any configuration. The argument that this is a defect rather than a style preference has to rest elsewhere, and it does: MD022 is enabled by `default: true`, and claude-plugins disabled twelve rules without disabling it. The generator collided with a rule the repository actively kept.

### A second defect the configuration hides

Lines 6 through 9 of the generated file are four consecutive blank lines. claude-plugins disables `MD012`, so nothing reports them and `--fix` leaves them alone. With `MD012` enabled, three further errors fire at lines 7, 8, and 9. A repository running default rules fails on both defects; this one fails on the half it happened not to disable.

### The formatters disagree with each other

Capturing the artifacts into this repository destroyed them, twice, before the mechanism was understood. `mise run fmt` runs `rumdl fmt`, which swept the new directory and repaired the input — inserting the MD022 blank line *and* collapsing two double spaces at lines 13 and 33 that `markdownlint --fix` had deliberately left. Input and output became byte-identical and the capture demonstrated nothing. Every project check passed while it was broken.

The second time, it reached the *output* file as well, which is another tool's exact result. `.rumdl.toml` now excludes `tests/fixtures/**/in` and `tests/fixtures/**/out`, scoped to the snapshots so surrounding prose stays linted.

**This is the same collision one level up, and it is the more general finding.** Two markdown formatters, given one input, produce two different correct-by-their-own-rules outputs. "Conform to the project's formatter" is therefore underspecified wherever a project runs more than one — and this project runs two.

## Conclusions

A release tool writing markdown into a repository it does not control faces a linter it cannot see, configured by rules it does not know, and possibly more than one formatter that disagree with each other. The claude-plugins case is not exotic: a default-enabled rule, a generator that never considered it, and a release blocked on a single blank line.

Excluding generated files from linting resolves it for the repository and dissolves the question for every tool, oakum included. That is the option to reject deliberately rather than by omission, and it is why [okm-jh7] exists as a decision separate from this research.

## Implications / actions

- Whatever oakum emits has to survive a linter it did not configure. The cheapest form of that is emitting markdown that passes the default rule set, since a repository's config is subtractive from defaults far more often than additive.
- A post-generation pass that runs the repository's own fixer is viable — proved here at a one-line diff — but inside oakum it means executing a user-named binary, which [ADR-0006](../decisions/0006-no-command-execution-in-templates.md) rejects in the templating path. In the user's own workflow it carries no such constraint. That asymmetry is the substance of [okm-jh7].
- Nothing under `tests/fixtures/**/in` or `**/out` may be formatted. Captured artifacts are malformed or foreign on purpose.

## Open questions

- Whether emitting default-rule-clean markdown is sufficient in practice, or whether real repositories add rules often enough that conforming to defaults still fails.
- Whether the `<sub>` date beneath the heading is deliberate styling in bumpy or incidental. It decides whether an upstream fix is a one-line change or a rendering argument.
