---
oakum: patch
---

An exported `GIT_LITERAL_PATHSPECS`, `GIT_GLOB_PATHSPECS`, `GIT_NOGLOB_PATHSPECS` or `GIT_ICASE_PATHSPECS` no longer changes what oakum asks git. Oakum writes its pathspecs for git's defaults, and under `GIT_LITERAL_PATHSPECS=1` the exclusions in `oakum migrate`'s gate look became literal file names: the look searched nothing, and a repository whose hook names `.bumpy/` was told no file names the old bump-file directory. Under `GIT_ICASE_PATHSPECS=1`, `oakum release` refused before tagging, because git rejects the `:(literal)` pathspec it reads the changelog through. Oakum now clears these variables for every git child it runs, as it already clears git's trace variables.
