---
oakum: patch
---

### Fixed

A workflow that installs oakum with an inexact version flag, such as `cargo install oakum --version=latest` or `cargo binstall oakum --version latest`, was reported as installing oakum without a version. It now names the spec, `latest`, as not an exact version. The outcome was already unverified; only the message changes.
