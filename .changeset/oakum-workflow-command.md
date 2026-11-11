---
oakum: minor
---

### Added

A new `oakum workflow` command prints the workflow `init` prints for this repository, alone on stdout, and writes nothing, in a repository oakum already manages too: `mkdir -p .github/workflows && oakum workflow > .github/workflows/oakum.yml` saves it. It takes `--token` and `--app`, and names on stderr what the workflow still needs. A pin naming another version refuses with exit 1, pins that disagree or cannot be read print the workflow and exit 2, and a failed action lookup exits 2 with nothing on stdout. An already-initialized `init` or `migrate` that refuses `--token` or `--app` now names `oakum workflow` with the same flag and secret.
