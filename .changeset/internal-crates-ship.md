---
oakum: major
---

A change to a package oakum does not version, unmanaged or excluded, now counts as a change to every versioned package that depends on it at runtime (Cargo `dependencies` and `build-dependencies`; npm `dependencies`, `peerDependencies` and `optionalDependencies`), directly or through other unversioned packages. `check --strict` asks that package for coverage and names the path, as in `app (cargo): changed (via store) with no covering intent`, so a branch that changes only an internal crate is now refused until a bump file covers the package that ships it; `oakum add --none --packages <package>` covers it without a release. `generate` and the commits-only plan release it: a `fix` commit to the internal crate patches the package that ships it, where it used to reach no package. An excluded package nothing versioned depends on stays silent.
