---
oakum: patch
---

Update rustls to 0.23.46 for RUSTSEC-2026-0346, -0347, -0348, and -0349, along with other semver-compatible dependency updates. serde-saphyr is capped below 1.3, so `cargo install oakum` without `--locked` keeps reading an empty block scalar (`core: |`) in a pnpm catalog as unresolved.
