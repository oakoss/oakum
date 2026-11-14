---
oakum: major
---

### Changed

Reading a changelog, oakum now decides which lines a version heading cannot be on the way a renderer does: CommonMark (pulldown-cmark) finds the raw HTML, code blocks and code spans, and an HTML tokenizer reads the raw HTML for comments, tags, quoted attributes and `script`, `style` and `textarea` text. A `</details>` inside an inline comment (`x <!-- </details> -->`), or inside a comment opened earlier in raw HTML, no longer closes a `<details>` block, and one in a code span or a quoted attribute stays text, so a heading the rendered page keeps collapsed no longer vouches for a version. A `<details>` in paragraph text now opens a block as it does on the page, and one left open in a list item, block quote or heading closes with it. A `script`, `style` or `textarea` left open swallows what follows, as it does on the page, and is named as left open. A heading directly under an HTML block with no blank line between is part of that block, as CommonMark renders it, and no longer counts as a heading either.
