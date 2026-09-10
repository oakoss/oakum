---
oakum: major
---

### Changed

`check` refuses when no selected package can produce work on either the version or the tag axis, where it exited 0 in silence. That state plans nothing and releases nothing while every gate reports green, and it is what a migration produces when the private-packages opt-in is lost. The refusal names both keys. A selection left empty by `include`/`exclude` stays silent, because that is a decision someone wrote down, while an absent `private-packages` key is the absence of one. `release` does not ask, so a release with nothing to do still says so in its own words.

`init` says the same thing where it writes such a config, rather than leaving it to the next command: an all-private workspace still gets its files and a zero exit, with the guidance on stderr.
