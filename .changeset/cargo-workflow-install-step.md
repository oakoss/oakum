---
oakum: patch
---

The workflow `oakum init` and `oakum migrate` print for a Cargo repository now installs oakum with `taiki-e/install-action` (`tool: oakum@<tool-version>`), pinned to a release oakum ships with. The step it replaces, `run: cargo binstall --no-confirm oakum@<tool-version>`, cannot run on `ubuntu-latest`, which does not ship `cargo-binstall`. For the same reason `migrate` no longer tells a Cargo repository without a pin to add `cargo binstall` to it; the printed workflow is the pin it offers. `check`'s refusal for a repository with no pin names the install-action step as its example instead of `cargo binstall`. oakum is not in install-action's own tool list, so the action installs `cargo-binstall` and uses it; `cargo-binstall` 1.24.0 resolves `oakum@0.4.0` for `x86_64-unknown-linux-gnu` from the GitHub release (dry run). `oakum check` reads the new step as the install pin, as it read the old one. npm workspaces are unchanged.
