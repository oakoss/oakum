---
oakum: patch
---

Update saphyr-parser, the YAML event parser behind in-place catalog and lockfile rewrites, from 0.0.12 to 0.2. Rewrites are unchanged. A `pnpm-lock.yaml` the parser rejects, such as one holding a NUL byte, is now named as unparsable rather than as using YAML anchors.
