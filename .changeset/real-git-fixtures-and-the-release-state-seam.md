---
oakum: none
---

Test fixtures, one internal seam, and corrections to behaviour that has not shipped yet.

`migrate`'s and `status`'s test fixtures are real git repositories with a base commit. The `.git` in them was an empty directory, so every git read failed and every assertion passed on the failure path — measured, eleven of `status`'s own tests were exercising the coverage look's failure while reading as though they covered its success. A repository alone was not enough: `git init` leaves no ref for a diff to resolve against, so the base commit is what actually moves those tests onto the success path, and the two that assert the outcome now name it.

`status` and `ci pr-status` now build release state through one path, and a caller names what it does when the coverage look fails rather than implying it by writing `match` or `?`. The version-PR body is not on that path: it renders a plan already in hand, with no repository to look at. Building that state twice is how two renderers of it came to disagree, which posted a pull-request comment that was an invisible marker and nothing else.

`release` is now tested against the shape this work came from: an all-private monorepo tagged `name@version`, cutting several tags in one run on the `private-packages.tag` opt-in. Each ingredient had a test; the combination did not, and it is the config `migrate` derives for exactly such a repository.

Three corrections to 0.3.0 before it ships. The coverage field distinguishes a look that was never asked for from one that failed, so the Version Packages body no longer blames git for a question it never put. A multi-line git diagnostic is reported by its verdict — the last `fatal:` — rather than by its first line, which names a symptom: against a corrupt loose object git opens with `error: inflate: data stream error` and concludes `fatal: Not a valid commit name main`. And `ci pr-status` refusing a look it could not make is now held by a test: flipping that disposition previously left the whole suite green.
