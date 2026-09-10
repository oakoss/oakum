---
oakum: patch
---

### Fixed

`migrate` carries a source tool's `privatePackages` into oakum's `private-packages`, where it dropped it. A repository whose packages are all private migrated to a config that managed nothing, so `check` exited 0, `status` planned nothing, and a release would have done nothing while reporting success. Changesets' bare-boolean form is read the way changesets reads it, as both axes. Both source files are read when present and their axes are unioned, so a repository migrating from one tool while a readable config from the other is still on disk carries both; the report names what each file contributed and the one line the write produces. A bumpy source also gets the "not carried over" report the changesets path already had. A source config oakum cannot open, read, or parse, or one whose axis is not a boolean, is reported and skipped rather than stopping the migration: both tools accept JSON their own loaders tolerate and `serde_json` does not, so a leftover config from an abandoned experiment no longer aborts an unrelated migration. A dangling symlink at either path is named as one rather than read as the file being absent.
