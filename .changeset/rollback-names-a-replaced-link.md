---
oakum: patch
---

When `oakum version` replaced a symlinked file with a regular file (a link into `.git`, one it could not follow, or one another write of the run shares) and the run then rolled back, the original text came back as a regular file and nothing said the link was gone. The rollback report now names that file and the link it was, so it can be recreated.
