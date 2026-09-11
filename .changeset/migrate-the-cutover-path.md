---
oakum: minor
---

### Fixed

`migrate` looks for the source tool in `node_modules/.bin` before giving up on it. `npm i -g` and Homebrew put `oakum` on `PATH` and a devDependency's binaries nowhere near it, so the install the docs recommend left the parity check unrunnable — on the one command whose whole job is comparing against that tool. The comparison then ran oakum against oakum, which cannot detect a mapping error, and said `match` about it. A source tool that really is absent now says where oakum looked, because installing globally and installing locally fix different halves of that. A source tool that ran and failed is no longer mistaken for one that answered: a crash whose output happens to parse is not an empty plan, a child the kernel killed is reported as such rather than as `exited -1`, and a difference measured against a plan read under a tool's exit-1 convention is unverified rather than proof the transform changed anything.

A tag that states no version — `v1`, `latest`, `nightly`, a date stamp — no longer cancels the whole tag-format derivation. `release` already classifies such a tag as someone else's and skips it, so one `v1` (the GitHub Actions convention) turned the derivation off on exactly the histories it was written for, and the two commands disagreed about the same tag. A history of nothing but moving tags is still reported, naming them: it has tags, and calling that "no tags" would say never released about a repository nobody managed to read.

`migrate` says which `versioning` mode it wrote and what settled it. `semver` is the non-default, and for a repository whose packages are all below 1.0.0 it is the difference between the next release being `0.18.0` and `1.0.0` — written silently until now. knope and release-plz imply `zero-major` instead; release-plz's own configuration documentation states the rule, and it had been grouped with the tools that take `0.1.3` to `1.0.0`.

A bump file copied out of the old tool's directory is described as a copy rather than a rewrite, and every original left behind is named among the remaining steps. oakum does not own that directory and will not delete from it, but the old tool goes on counting those files: measured in a clone, after `migrate` and `version` released a package 0.17.0 to 0.18.0, the old tool still showed three pending and would have released it again. A repository whose workflow still runs the old tool on every push to `main` gets that second release on the merge commit.

`migrate` names what in a repository refers to the old tool's bump-file directory, so a commit hook or CI step pointed at it can be repointed before it rejects the bump files oakum writes — which is what happened in the first repository to adopt oakum. oakum cannot tell a gate from a mention in prose, so the step asks the reader to look rather than asserting each match is a gate, and it says what it did not search: the look reads git's index outside `.changeset/`, so an untracked file, an unstaged edit, a submodule and `.git/hooks/` are all uncovered. A look that fails says so and exits unverified rather than passing for no gate. Only a bumpy cutover has an old directory to repoint anything at; changesets and knope already keep bump files in `.changeset/`, which oakum adopts in place.

The plan `migrate` prints is sorted. It came out in filesystem order, so the same repository could show a reader a differently ordered plan on a different machine.

A difference the parity check found outranks a look that could not happen. The first is a finding and the second is missing evidence, so a migration that proved the release plan changed reports that, at exit 1, even when the gate look also failed.
