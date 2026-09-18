---
oakum: minor
---

### Added

`oakum check --json` prints a versioned `CheckReport` instead of the prose report: every look the run could perform, what each established, and which refusal decided. A caller that must see both outcomes of a mixed run now has somewhere to read the second — ADR-0035 ranked a finding above a look that did not happen and named the cost: the unverified outcome survives only in the `also` prose. A look nobody asked for reports `not-asked`, and one that said something without refusing reports `reported`, so neither can read as a look that happened and found nothing. A look that raised several refusals carries every one of them with its own evidence, rather than the first alone. The document also carries what the run covered — the packages selected, the base it diffed from or why that could not be named, and whether coverage gated — which the prose report says on stdout and `--json` replaces. Refusals still reach stderr and the exit code is unchanged, so one run yields a document and a human-readable failure.
