---
oakum: patch
---

### Fixed

The workflow `init`, `migrate`, and `oakum workflow` print gives the release job `actions: read` beside `contents: write`, and, under `--app`, mints its token with `permission-actions: read`. `release` reads the commit's workflow runs to confirm a tag-listening workflow ran, before the first tag as well as after each, and a private repository refuses that read to a token without Actions read: with a tag-listening workflow there, the printed release job failed with exit 1 before writing any tag. A refused read now exits 2 (unverified) and names the permission, and the `--token` next step says the secret needs Actions read on a private repository.
