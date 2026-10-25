---
oakum: major
---

A clone whose remote fetches only some branches (`git clone --single-branch`) is unverified (exit 2) where oakum reads every tag (`release`'s check of local tags and `migrate`'s tag-format derivation), instead of read as a complete tag set. git follows only the tags on the branches it fetches, so a release tagged on another branch was missing and the short set read as the whole. Readers of the tags on HEAD's history (`check`, `version`, `reachable-tags`) are unaffected: the fetched branch brought every tag on it.

oakum judges the remote it reads tags from (`origin`, else the first by name). It reports when no fetch refspec stores every branch, a negative refspec takes some branches back, or the remote has no refspec at all, unless `tagOpt` is `--tags` or a refspec stores every tag under `refs/tags/`. The message names the refspec and the fix: `git config --replace-all remote.<name>.tagOpt --tags`, then `git fetch --tags -- <name>`. `actions/checkout` at `fetch-depth: 0` keeps the refspec for every branch and is unaffected.

The `--no-tags` check now follows what git fetches, too: a `tagOpt` value git does not recognise no longer hides an earlier `--no-tags`, a `git clone --mirror` (whose refspec stores every tag) is no longer reported as tag-suppressed, and a negative refspec that takes tags back counts against it. Remote settings are read with `git config -z`, so a remote whose name holds a space no longer breaks the read.

Known limits. Both checks read what the next fetch would bring, not what the last one did, so a remote widened (or set to `tagOpt --tags`) but not yet fetched reads as whole. In a single-branch clone, checking out a branch fetched by name into `FETCH_HEAD` alone leaves the tags on it unfetched, and the HEAD readers do not notice.
