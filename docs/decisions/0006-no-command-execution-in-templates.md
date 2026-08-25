# Templates render; they do not execute

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Any value that produces text is a user-owned template. Should a template value be allowed to take the form `{ command = "..." }`, running a shell command and using its output?

## Decision Drivers

- `check` must be pure ([ADR-0003](0003-write-only-what-a-command-owns.md))
- A release must not fail because of something absent from the runner
- Reviewers scrutinize workflow files; they do not scrutinize a changelog template

## Considered Options

- Inline strings, `{ file = "path" }`, and `{ command = "..." }` as three tagged forms
- Inline strings and `{ file = "path" }` only

## Decision Outcome

Chosen option: **inline and `{ file = ... }` only**.

The disqualifying argument is not security. **An executable template makes `check` impure by construction** — rendering for a dry run, a preview, or an error message would execute the command. mise shipped exactly this and documents it as a warning rather than a fix: *"Dry-run mode suppresses the planned mise operation; it does not sandbox or suppress commands executed by template functions."*

The second argument is hermeticity. A template string and a template file produce the same output on any machine. A command depends on a binary being present, on its version, its exit code, its output encoding. That failure does not degrade the changelog; it aborts the release. git-cliff's own flagship example shells out to `pandoc`, which is not installed on a stock GitHub runner.

GoReleaser is the existence proof that the capability is unnecessary: 42 template functions at v2.17.1 including `readFile` / `mustReadFile`, `missingkey=error`, and no exec function anywhere — with command execution living on a separate `hooks` surface. *(The 2026-08-18 draft said 36; the count is 42 at v2.17.1 and 43 at v2.18.0 with `join`. Sources in [templating-prior-art.md](../research/templating-prior-art.md).)*

The escape hatch costs nothing: accept release notes on stdin or with `--notes-file`. Whatever needs generating runs in the user's workflow, which branch protection and code review already cover, and arrives as text.

### Consequences

- Good, because `check`, `plan`, and `--dry-run` stay genuinely side-effect free
- Good, because file-based templates are differentiated on their own — no surveyed free product ships a config value that loads a template body from a path, and GoReleaser documents `from_file` / `from_url` as Pro (see the 2026-08-25 amendment)
- Bad, because a user with an unusual formatting need must add a workflow step instead of a config line
- Neutral, because the tagged-form design keeps `{ command = ... }` addable later; this defers rather than forecloses

### Confirmation

If it is ever added, it goes on a separate named surface, defaults to `shell = false`, is enabled only from outside the config file, and never runs during `check`. The gate must live outside the file being gated — every interactive trust mechanism (direnv, mise, VS Code) degrades to exactly that in CI.

## Amendment (2026-08-25)

The 2026-08-18 consequence that file-based templates are differentiated was unverified. [templating-prior-art.md](../research/templating-prior-art.md) re-derived the engine table (`okm-s8e`). The likely counterexamples are not: git-cliff 2.13.1 keeps `body` as a `String`; unreleased `--body-file` is CLI-only. semantic-release's `writerOpts` are Handlebars strings. GoReleaser OSS keeps header/footer as inline strings; live docs (not the 404'd Pro page) mark `from_file` / `from_url` and `template_files` as Pro. `readFile` in an inline template is not `{ file = ... }`.

The decision does not change. The disqualifying argument was never this row.

## More Information

- [templating-prior-art.md](../research/templating-prior-art.md)
- `{ file = ... }` needs containment: a template body is often published, so a path escaping the repository root would splice credentials into a public release. Canonicalize, then verify containment.
- pnpm 10's `onlyBuiltDependencies` allowlist was bypassed for a year (CVE-2025-69264, CVSS 8.8) because it was consulted at some call sites and not others. Any gate needs one chokepoint.
