---
oakum: patch
---

### Fixed

`check` names a package whose coverage a pull request will not receive. Changed packages are read from commits while intent is read from the working tree, so a bump file that is not committed — or one that is committed and has since been edited to name another package — covered a change that was committed, and `--strict` exited 0 while the ref you were about to push exited 1, with nothing said. The mirror of the change named outside `HEAD`, which this release also carries: there the change is invisible, here the coverage is.

The question is asked of `HEAD` itself, reading the intent its tree carries rather than guessing from which files are staged. That is what catches the ordinary flow: commit a bump file, touch a second package later, add a line to the file you already have. It costs nothing where there is no intent on disk to rest on, and nothing at all under `conventional-commits`, where both halves already read commits.

Advisory, like the rest of that half: it names what a pull request will not get without changing any exit outcome. An uncommitted bump file is the normal state seconds after `oakum add`, so refusing would fire on nearly every local loop.
