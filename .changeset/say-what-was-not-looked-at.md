---
oakum: patch
---

### Fixed

`check` names a package that changed outside `HEAD` — staged, edited or untracked — instead of passing over it. The coverage look reads commits while intent is read from the working tree, so a change that is not yet a commit was in neither half, and `--strict` exited 0 on a tree it refuses one commit later. The scope report now names both ends of the diff (`<base>...HEAD`) rather than reading as a comparison against the tree in front of you.

What that look could not read is said too: a working tree `git status` cannot walk, or can walk only in part, is reported rather than passed over in silence. Both are advisory, like the rest of this half — they name what went unread without changing any exit outcome. `release` reports them on the same terms.

`ci pr-status` says why it posted nothing on each of its three non-posting paths: an empty plan, the version pull request, and `pr-status = "none"`. Only the fork and token failures announced themselves before, so an empty plan and a swallowed failure looked alike from the workflow's side. A leftover plan comment this run's token could not remove is named as well, rather than leaving a line about what was not written to imply nothing was left behind.

Every git child now runs under `LC_ALL=C`, so the diagnostics oakum quotes back to you are in one language whatever the surrounding locale.
