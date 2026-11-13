---
oakum: major
---

### Changed

A Cargo workspace's root `Cargo.toml` or `Cargo.lock` that is a symlink is now compared where it leads, as cargo reads and writes it. A lock update through `Cargo.lock -> locks/Cargo.lock` changed only the target and passed `check --strict` with no version-managed package changed; it now reaches the packages that ship it, named by the target's path (`locks/Cargo.lock: rusqlite`). Pointing the link at another lock compares the two locks it led to, and each commit is read through its own link. A link leading out of the repository is read in place.
