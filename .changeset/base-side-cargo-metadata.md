---
oakum: major
---

### Changed

On the earlier side of a `Cargo.lock` change, coverage now asks `cargo metadata --no-deps` which rows were whose, run read-only on that side's tree in a temporary directory, instead of reconstructing cargo's answer from the manifests. A member outside the workspace root, and a registry named in a member directory's `.cargo/config.toml` when oakum runs from that directory, no longer make a dev-only lock change ask a runtime package for a bump file. Where cargo cannot read the earlier side, as with inheritance its root cannot answer or a registry no config names, coverage is `unverified` (exit 2), quoting cargo, where it used to answer. In a blob-filtered partial clone lacking one of that side's manifests, coverage is `unverified` too, naming a clone without `--filter`; the tree is read without fetching. So is a submodule or a link out of the tree on that side where a member of today's may sit. cargo runs there as the toolchain your checkout resolves to, so a toolchain the earlier side pins is not installed.
