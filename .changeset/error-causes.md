---
oakum: patch
---

Two kinds of error message now say why. A GitHub request that fails names its cause, such as `operation timed out` or a refused connection, where it stopped at "error sending request for url". A file oakum reads or creates through a symlink it cannot use is named as one: a dangling link (as `version` meets on a dangling `CHANGELOG.md`) says the symlink's target does not exist instead of "File exists", and where cap-std reported "a path led outside of the filesystem", the message now says the file is reached through a path that leaves the repository, or through a symlink with an absolute target, neither of which oakum follows, or, for a link that is itself absolute, says just that. Exit codes are unchanged.
