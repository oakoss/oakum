# Customizing release text: prior art and the command-execution question

- Date: 2026-08-18
- Author: Jace Babin
- Scope: How other tools let users control release text, and whether a config template should be able to run a shell command.

## Question

Everything oakum produces as text — release title and body, PR title and body, commit messages, changelog entries, tag format — is a user-owned template. What shape should that take, and should a template value be allowed to shell out?

## Sources

Source and documentation for git-cliff, GoReleaser, release-plz, knope, changesets, release-please, semantic-release, nx, cargo-dist, mise, direnv, pnpm; GitHub Actions security documentation; minijinja docs.rs and crates.io.

## Findings

### Two traditions, split by host language

Rust and Go tools give users **template strings**; JavaScript tools give users a **code module** and skip templating entirely.

| Tool | Mechanism | Engine | Undefined variable |
|---|---|---|---|
| git-cliff | inline strings in `cliff.toml` | Tera | errors on render; **falsy in `{% if %}`** |
| release-plz | inline strings | Tera 2 for its own fields, **Tera 1** through git-cliff for changelogs | same |
| GoReleaser | inline strings; file and URL are **Pro-only** | Go `text/template` | **hard error** (`missingkey=error`) |
| knope | naive substring replacement | none | n/a |
| semantic-release | Handlebars through `writerOpts` | Handlebars | empty string |
| release-please | Handlebars internally, **not exposed in config** | Handlebars | n/a |
| changesets / nx | JS or TS module | none | n/a |

**No surveyed tool ships file-based templates for free.** That makes `{ file = "path" }` genuinely differentiated rather than a copy.

### Two design lessons worth taking

**Context is per-surface, not global.** GoReleaser's git fields are unavailable in the `env` section; artifact fields exist only in per-artifact scopes; `.Checksums` only in the release body. Strict undefined-checking is only tractable when the context is scoped — otherwise failures become a game of "which section am I in".

**Templates render; hooks execute; they are separate surfaces.** GoReleaser has 36 template functions including `readFile` and `mustReadFile`, and **no exec function**. Command execution lives in `before.hooks` / `after.hooks`. It is the most template-heavy tool in the survey and it draws that line hard.

Also: do not repeat release-plz's two-engine split. Users there write Tera 1 and Tera 2 dialects in one config file.

### Config-triggered execution is common and completely ungated

git-cliff's `commit_preprocessors` and `postprocessors` accept `replace_command`, piping matched text through a shell with `$COMMIT_SHA` in the environment. release-plz inherits it. `@semantic-release/exec` generates shell commands through a Lodash template, which is **arbitrary JavaScript evaluation**, and its stdout becomes the release notes.

**None of them carries a security note.** There is also no CVE for "release tool executed a config-declared command and leaked secrets" — a null result best read as an unexamined practice rather than a safe one.

### The GitHub Actions rules, which resolve the threat model

| Trigger | Secrets | `GITHUB_TOKEN` |
|---|---|---|
| `pull_request` from a **fork** | none | read-only |
| `pull_request` from a **same-repo branch** | **yes** | writable |
| `pull_request_target` | yes | writable |
| `push` to main | yes | writable |

A fork PR adding a malicious command reaches nothing, and public repositories require approval for first-time contributors' runs by default. `actions/checkout` v7 (2026-06-18) now refuses fork PR code under `pull_request_target` unless explicitly allowed.

So the exposure begins at **merge** — the same trust boundary that already protects `build.rs`, `postinstall`, and workflow files. Two residual risks are genuinely different: a same-repo branch PR *does* carry secrets, so any job rendering templates on PR events executes with secrets pre-merge; and a `command =` inside a TOML value reads as configuration in a diff, while a workflow step reads as code.

### Trust mechanisms do not survive CI

direnv hashes `.envrc` content; mise has `mise trust`; VS Code has Workspace Trust. All three are interactive and local. In CI they degrade to an environment variable — which is the right shape, because **the authorization then lives outside the file being authorized**.

mise is the near-exact precedent: its Tera templates have `exec()` and `read_file()`, and it had to ship `MISE_PARANOID` to take them back out, for the documented case of bots processing pull requests. Its docs also name the sharp edge: *"exec() runs whenever its template is rendered, including during --dry-run operations... Dry-run mode suppresses the planned mise operation; it does not sandbox or suppress commands executed by template functions."*

The one CI-compatible pattern is default-deny plus allowlist — pnpm 10's `onlyBuiltDependencies`. **CVE-2025-69264** (High, CVSS 8.8) bypassed it entirely for a year, because git-hosted dependencies never consulted the allowlist. An allowlist checked at each call site rather than at one chokepoint is a bug waiting to happen.

### minijinja specifics

Stable **2.24.0** (2026-08-12); `3.0.0-alpha.0` published the same day. `debug` is a default feature; `loader` and `fuel` are not.

`UndefinedBehavior` has four variants, and **`Strict` also errors on `{% if undefined %}`**. Release text branches on legitimately absent fields constantly — no previous version on a first release, no breaking changes on a patch — so `Strict` forces `is defined` guards throughout every user template. **`SemiStrict` fails on printing, iteration, and attribute access while treating undefined as falsy in `{% if %}`**, which is both the property wanted and exactly Tera's behavior, so intuitions carry over from git-cliff and release-plz.

Three traps:

- **Auto-escape is chosen by file extension.** `.json`, `.yaml`, and `.yml` map to JSON escaping, so a template named `pr-body.yml` is silently escaped. Set the callback to `None` unconditionally.
- `keep_trailing_newline`, `trim_blocks`, and `lstrip_blocks` all default to false. Turn the latter two on, or `{% for %}` blocks leave ragged blank lines in commit and tag messages.
- `Debug` **hides** line numbers and source context unless formatted with `{:#}` — the inverse of the usual convention.

The default builtin set touches no filesystem, environment, or process. Templates are inert unless something is added.

## Conclusions

Ship inline strings and `{ file = "path" }`. Do not ship `{ command = "..." }`.

The security argument is real but secondary. The disqualifying arguments are that a command is the most environment-dependent value a config can hold — a missing binary does not degrade the changelog, it aborts the release — and that an executable template collides directly with the rule that `check` is pure. mise shipped that collision and documented it as a warning rather than fixing it.

## Implications / actions

- `SemiStrict`, not `Strict`. Auto-escape forced to `None`. `trim_blocks` and `lstrip_blocks` on.
- One engine everywhere.
- Per-surface template contexts, not one global object.
- **Resolve `{ file = ... }` paths against the repository root and reject anything escaping it.** A template body is often published, so `{ file = "../../.npmrc" }` would splice credentials into a public release. Canonicalize, then verify containment.
- The escape hatch is stdin or `--notes-file`: whatever needs generating runs in the user's workflow, which reviewers already scrutinize, and arrives as text.
- If command execution ever earns its place, it goes on a separate named surface, defaults to `shell = false` like knope, is enabled only from outside the config file, and never runs during `check`, `plan`, or `--dry-run`.
- Document GitHub Environments with required reviewers for publish tokens. It is a stronger control than anything implementable in-process.

## Open questions

- Whether `{% include %}` hard-errors or silently no-ops without the `loader` feature. Only matters if template composition is ever supported.
