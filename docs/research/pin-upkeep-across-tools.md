# Pin upkeep across tools: pnpm release-age exclusions and Renovate grouping

- Date: 2026-11-09
- Author: Jace Babin
- Scope: Whether a repository can keep its pnpm release-age exemption for oakum, and its second oakum pin, from needing an edit on every oakum bump.

## Question

[claude-plugins-cutover-2026-09-10.md](claude-plugins-cutover-2026-09-10.md) (F19) found a repository with pnpm's release-age gate keeping a per-version `@oakoss/oakum` exemption that had to be edited on each bump. [ADR-0038](../decisions/0038-the-install-pin-is-the-version.md) left two questions open: whether `minimumReleaseAgeExclude` accepts a bare package name, and whether a repository pinning oakum twice gets one Renovate pull request per pin, each `unverified` alone.

## Sources

- pnpm 10.34.5, 11.25.0, and 12.16.1 (mise installs), run 2026-11-09 against the npm registry, `@oakoss/oakum` 0.9.2 and 0.9.3
- Renovate 44.181.7 from npm (published 2026-10-25), run 2026-11-09 under Node 24.22.2 with `--platform=local --dry-run=lookup`, `LOG_LEVEL=debug`, `LOG_FORMAT=json`; Renovate 44 refuses Node 26 ("Unsupported node environment detected")

## Findings

### `minimumReleaseAgeExclude` accepts a bare name

Each case is a fresh directory holding a `package.json` with one devDependency on `@oakoss/oakum` and a `pnpm-workspace.yaml` with `minimumReleaseAge: 525600` (a year in minutes, so every oakum version is too new), installed with `pnpm install --ignore-scripts`. The frozen cases copy the lockfile the bare case wrote and run `pnpm install --frozen-lockfile --ignore-scripts`.

| Exclude entry | Spec | 10.34.5 | 11.25.0 | 12.16.1 |
| --- | --- | --- | --- | --- |
| none | `0.9.3` | exit 1 `ERR_PNPM_NO_MATURE_MATCHING_VERSION` | exit 1, same | exit 1, same |
| `'@oakoss/oakum'` | `0.9.3` | exit 0 | exit 0 | exit 0 |
| `'@oakoss/oakum'` | `0.9.2` | exit 0 | exit 0 | exit 0 |
| `'@oakoss/oakum@0.9.2'` | `0.9.3` | exit 1 `ERR_PNPM_NO_MATURE_MATCHING_VERSION` | exit 1, same | exit 1, same |
| `'@oakoss/*'` | `0.9.3` | exit 0 | exit 0 | exit 0 |
| none, frozen | lockfile at `0.9.3` | exit 0 | exit 1 `ERR_PNPM_MINIMUM_RELEASE_AGE_VIOLATION` | exit 1, same |
| `'@oakoss/oakum'`, frozen | lockfile at `0.9.3` | exit 0 | exit 0 | exit 0 |

pnpm 10.34.5 named the version and its age: `Version 0.9.3 (released 2 days ago) of @oakoss/oakum does not meet the minimumReleaseAge constraint`. On 10.34.5 and 12.16.1 no run changed `pnpm-workspace.yaml`; the 11.25.0 files were not read back.

### Renovate opens one branch per manager, and `groupName` joins them

Each case is a fresh git repository with `"@oakoss/oakum": "0.9.2"` under `devDependencies` and an oakum pin at `0.9.2` under `[tools]` in `.mise.toml`; `renovate.json` extends `config:recommended`. Branches are read from the `packageFiles with updates` debug record.

| Case | Extracted deps (manager, depName, packageName, datasource) | Planned branches |
| --- | --- | --- |
| no rule, `"cargo:oakum"` | mise `cargo:oakum` `oakum` crate; npm `@oakoss/oakum` `@oakoss/oakum` npm | `renovate/cargo-oakum-0.x`, `renovate/oakoss-oakum-0.x` |
| `groupName: "oakum"` rule, `"cargo:oakum"` | same | both in `renovate/oakum` |
| `groupName` rule, `"github:oakoss/oakum"` | mise `github:oakoss/oakum` `oakoss/oakum` github-releases; npm as above | npm in `renovate/oakum`; the github-releases lookup did not run (`GitHub token is required for some dependencies`, no token given) |

The rule:

```json
{
  "matchPackageNames": ["@oakoss/oakum", "oakum", "oakoss/oakum"],
  "groupName": "oakum"
}
```

`matchPackageNames` compares `packageName` ([renovate-rule-matching.md](renovate-rule-matching.md)), which differs by mise backend, so the rule names all three.

### Renovate reads no workflow install line

With a workflow holding `taiki-e/install-action` with `tool: oakum@0.9.2` and a `run: npm i -g @oakoss/oakum@0.9.2` step, Renovate extracted `taiki-e/install-action` and the runner image and neither oakum line. Two regex custom managers made both lines dependencies:

```json
[
  {
    "customType": "regex",
    "managerFilePatterns": ["/(^|/)\\.github/workflows/[^/]+\\.ya?ml$/"],
    "matchStrings": ["npm i(?:nstall)? -g @oakoss/oakum@(?<currentValue>[^\\s]+)"],
    "depNameTemplate": "@oakoss/oakum",
    "datasourceTemplate": "npm"
  },
  {
    "customType": "regex",
    "managerFilePatterns": ["/(^|/)\\.github/workflows/[^/]+\\.ya?ml$/"],
    "matchStrings": ["tool: oakum@(?<currentValue>[^\\s]+)"],
    "depNameTemplate": "oakum",
    "datasourceTemplate": "crate",
    "versioningTemplate": "semver"
  }
]
```

With the `groupName` rule, the npm and mise pins and both workflow lines planned `0.9.3` in `renovate/oakum`. Without `versioningTemplate: "semver"` the crate entry planned no update. Inferred, not read from Renovate's source: the crate datasource's default Cargo versioning reads `0.9.2` as a range that `0.9.3` satisfies.

## Conclusions

A bare `'@oakoss/oakum'` in `minimumReleaseAgeExclude` exempts every version, so the exemption needs no edit when the pin moves. A versioned entry exempts only that version. pnpm 10 enforces the age only when it resolves; 11 and 12 also enforce it on a frozen install.

A repository with two oakum pins gets one Renovate pull request per manager, each raising one pin, so each fails `check` alone. A `groupName` rule naming all three package names puts them in one branch. A workflow install line is not updated by Renovate at all without a custom manager.

## Implications / actions

- The GitHub Actions guide shows the bare exclusion and the `groupName` rule, and says a workflow install line needs a custom manager.
- ADR-0038's inferred per-manager split is now measured, and its open release-age question is answered.

## Open questions

- Whether Renovate writes the grouped update to every file correctly: these were `--dry-run=lookup` runs, which plan branches and write nothing.
- The `github:` backend's lookup, which needs a token.
