---
oakum: major
---

### Changed

`check` no longer fetches into a partial clone. Its git reads run with lazy fetching off, so a read that needs an object the clone left out fails, and that look is `unverified` (exit 2), quoting git and naming the partial clone, where it used to fetch the object into `.git` and answer. In a clone filtered past file contents, such as `--filter=tree:0`, `check` refuses before reading anything, naming the remote, its filter, and the two commands that bring the trees in and keep them: `git fetch --refetch --filter=blob:none <remote>`, then `git config remote.<remote>.partialclonefilter blob:none`. A `blob:none`, `blob:limit` or `sparse:oid` clone still lists a branch's changes without fetching, but a branch that edits a `Cargo.toml` or `Cargo.lock` usually needs a blob it lacks, so run `check` in CI from a clone with no filter. In a partial clone, a root file whose blob, tree or commit the clone lacks is `unverified`, where it was read as absent.

Coverage counts a file moved from one package to another against both packages; it counted only the one the file moved into.

`check --remote` in a partial clone reads the remote's URL past the `[<filter>]` git 2.56 prints after it, where it said it could not establish the remote's transport.
