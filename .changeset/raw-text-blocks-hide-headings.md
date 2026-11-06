---
oakum: major
---

### Changed

A changelog heading inside a `<pre>`, `<script>`, `<style>` or `<textarea>` block no longer counts as a version section, matching how CommonMark renders it: `check` no longer reads such a heading as vouching for the version, and `check`, `version` and `release` refuse a changelog or bump-file note that leaves one of these blocks open, naming the block (and, in a changelog, its line), as they already do for an open HTML comment or `<details>` block. A `</details>` after a comment's `-->` on the same line now closes its block, where it was refused.
