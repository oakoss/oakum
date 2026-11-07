---
oakum: major
---

### Changed

`migrate`'s look for gates on the old bump-file directory now reads the hooks git runs as well as git's index. A hook naming the old directory (`.bumpy`, matched as the index search matches it) in the hooks directory — `.git/hooks/`, the one `core.hooksPath` names, or the common one from a linked worktree — is named for the reader to repoint, and a hook git would run that oakum cannot read exits 2 (`unverified`) naming it. Only a file named after a git hook event counts, and on Unix only one with an execute bit, as git runs no other. A migration run mid-conflict also exits 2 naming each conflicted file, which `git grep --cached` skips without saying so: such a run used to print "no file in the index … names the old bump-file directory" and exit 0 over a gate in a conflicted file. Hooks defined in git config (`hook.<name>.command`) are still not searched, and the report says so.
