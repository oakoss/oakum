# What if `check` ran as a git hook?

- Status: draft
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

`check` is pure — it reports drift and names the fix, never applies it ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)) — which makes it safe to run automatically. A hook context (`oakum check --hook`) could report only what matters at that moment, paired with an explicit `install-hook` command the user runs deliberately.

**Target pre-push, not pre-commit.** A missing bump file is not wrong at commit time; it is wrong when the branch leaves the machine. Blocking every commit for it trains people to pass `--no-verify`, which disables the checks that actually needed to run.

## Why it might matter

- The failure it catches — merging a change with no bump file — is silent and only discovered at release
- Pre-push is late enough to be accurate and early enough to be cheap to fix

## Sketch

Installation must stay opt-in and explicit. [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md) forbids auto-installing hooks outright, and that rule exists specifically because `bd init` set `core.hooksPath` and silently broke lefthook. `install-hook` is a command the user names; nothing installs on `init`.

Hook manager support, weighted by what the repositories actually use: **lefthook first-class** (16 repositories), husky by printed snippet (5), plain `.git/hooks` as the fallback. Skip Python `pre-commit` — zero repositories use it.

For lefthook the right output is probably a config block to paste rather than a file write, since lefthook owns `lefthook.yml` and oakum does not.

## Open questions

- Whether `--hook` should differ from plain `check` at all, or whether the difference is only in exit code and verbosity. **Still open**, though an adjacent question is settled: bumpy's two hook values differ from *each other* substantively, since `--hook pre-commit` counts staged *and* committed bump files while `--hook pre-push` counts committed ones only ([bump-file tool interfaces](../research/bump-file-tool-interfaces.md)). What that research does not establish is how either differs from plain `check`, which is what this question asks. `--strict` and `--no-fail` are orthogonal flags rather than hook-mode behavior.
- What it does on a branch with no bump files that is *intentionally* releaseless — docs-only branches are common, and a hook that cries wolf there gets disabled. **Narrowed**: `--no-fail` covers the advisory case; deliberately releaseless is expressible via empty / `none` bump files ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)). What remains is whether a hook treats those files as satisfying coverage the same way `check --strict` will.
- Whether pre-push can tell "pushing a feature branch" from "pushing to main", since the answer differs. `--base <branch>` is the input bumpy takes, and taking a base is not detecting a destination — but the information exists, from git rather than from any tool. `git help hooks` documents that `pre-push` receives the remote name and location as its two arguments, and lines of `<local-ref> <local-object-name> <remote-ref> <remote-object-name>` on stdin. So the destination is available to a hook and the question is what oakum should do with it, not whether it can know.

## Related work

- [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md) — why installation is never automatic
- This repository's own `lefthook.yml`, which is the first-class case working in practice
