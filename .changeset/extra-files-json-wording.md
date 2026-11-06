---
oakum: patch
---

### Changed

An `extra-files` write that meets a duplicated key or a path that is not an object now words the error as every other JSON edit does: "has duplicate keys" where it said "is duplicated", and "is not an object" where it said "is not an object or array".
