---
oakum: major
---

### Changed

`check`'s staging look now sweeps the directory of every file `version` writes, including a nested workspace's `Cargo.toml`, `Cargo.lock`, `pnpm-lock.yaml` and catalog when they are regular files. A stray `oakum` staging file beside one of them (`rust/.Cargo.lock.oakum-write.…`) is reported as unverified (exit 2) instead of passing.
