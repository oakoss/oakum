---
oakum: patch
---

Removing a package no longer leaves `check` exiting 2 on its old tags. Delete the package and the lines that name it; its tags stay as history, and its published releases with them. A package renamed in place whose new name has no tags of its own still exits 2, now naming the tag to add (`git tag <new>@<version> <commit>`). An `include`/`exclude` entry or a bump file naming a removed package says to delete that line.
