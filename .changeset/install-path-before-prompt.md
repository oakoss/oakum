---
oakum: patch
---

`oakum init --interactive` and `oakum migrate` decide the printed workflow's install steps from what they read before the prompt. A `package.json` replaced (by a directory, a FIFO, or a symlink loop) while the prompt waited no longer turns an npm workspace's workflow into the Cargo one. A pnpm version that cannot be read for the workflow is now reported before the prompt and before any GitHub lookup.
