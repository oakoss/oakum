---
oakum: patch
---

A file `version` rewrites through a symlink stays a symlink. `version` read a symlinked manifest through the link but replaced the link with a regular file, so the shared file it named kept the old version. Writes now follow the link: the file it names gets the new text, staged beside that file, and the link stays, and `check` looks for staging files beside a link's file too. A link oakum does not follow safely (one into a `.git`, one it cannot resolve, one whose file refuses the new text, or one whose file another write of the same run also names) is replaced by a regular file as before, and stderr names the file that did not get the new text.
