---
oakum: major
---

### Changed

A changelog heading inside a `<pre>`, `<script>`, `<style>` or `<textarea>` block no longer counts as a version section, matching how CommonMark renders it: `check` no longer reads such a heading as vouching for the version, and `check`, `version` and `release` refuse a changelog or bump-file note that leaves one of these blocks open, naming the block (and, in a changelog, its line), as they already do for an open HTML comment or `<details>` block. A `</details>` after a comment's `-->` on the same line now closes its block, where it was refused. Inside one of these blocks a `#` line is content too: it no longer ends the version's section or the release body read from it, and in a bump-file note it no longer splits the note into Keep a Changelog sections. The same holds for a Keep a Changelog heading inside an HTML comment or `<details>` block in a note: the note is written whole, where `version` refused it as leaving the block open.
