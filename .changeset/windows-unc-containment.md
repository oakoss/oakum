---
oakum: patch
---

Windows containment treats a loopback admin UNC path as the same volume as the drive letter, so a localhost C$ prefix cannot smuggle a path past the repository check.
