---
oakum: patch
---

A YAML catalog pin that shares an anchor with another entry moves alone. `version` rewrote the anchored value, so a pin written as `*caret` also moved every other entry using `&caret`, including another package's, and the next `pnpm install` failed. Now an alias pin is written out as a quoted literal range and the anchor keeps its value; a pin that carries an anchor is rewritten and its aliases are written out, quoted, with the old value. An entry that reaches the pin through an alias of a whole table still moves with it. A line on stderr names each case once the files are written.
