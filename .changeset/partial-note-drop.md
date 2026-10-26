---
oakum: major
---

A bump-file note that keeps one section and loses another is now caught, as one that loses them all already was. A heading with nothing under it reaches no changelog, and when another heading in the same note rendered, the release looked complete while the empty section disappeared without a word. `add` refuses such a note before writing, and `check` reports it (refusing under `--strict`), each naming the empty heading: "has a heading with nothing under it (`### Fixed`)". A note that opens with a heading, an empty note, and a coverage-only note are unaffected.
