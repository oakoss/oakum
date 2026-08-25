# Customizing release text: prior art and the command-execution question

- Date: 2026-08-18, revised 2026-08-19, engine table re-derived 2026-08-25 (`okm-s8e`)
- Author: Jace Babin
- Scope: How other tools let users control release text, and whether a config template should be able to run a shell command.

## Question

Everything oakum produces as text — release title and body, PR title and body, commit messages, changelog entries, tag format — is a user-owned template. What shape should that take, and should a template value be allowed to shell out?

## Sources

Engine table and the file-based-template claim, fetched 2026-08-25:

- git-cliff **v2.13.1** (crates.io `max_stable_version`, `updated_at` 2026-04-26):
  - [`git-cliff-core/src/config.rs`](https://github.com/orhun/git-cliff/blob/v2.13.1/git-cliff-core/src/config.rs) — `ChangelogConfig.body: String`; `header`/`footer` are `Option<String>`
  - [`git-cliff-core/Cargo.toml`](https://github.com/orhun/git-cliff/blob/v2.13.1/git-cliff-core/Cargo.toml) — `tera = "1.20.1"`
  - [changelog config](https://git-cliff.org/docs/configuration/changelog/) — `header` / `body` / `footer` are TOML strings
  - [Tera 1.20.1 docs](https://github.com/Keats/tera/blob/v1.20.1/docs/content/docs/_index.md) — missing `{{ }}` errors; undefined is falsy in `{% if %}`
  - [PR #1574](https://github.com/orhun/git-cliff/pull/1574) (merged 2026-07-11) — unreleased `main` adds CLI `--body-file`; `ChangelogConfig.body` stays `String`
- release-plz (workspace `main`, fetched 2026-08-25):
  - [config](https://release-plz.dev/docs/config) — `git_release_name` / `git_release_body` / `git_tag_name` / `pr_name` / `pr_body` “use Tera 2”; changelog templates “rendered by git-cliff, which still uses Tera 1”
  - workspace [`Cargo.toml`](https://github.com/release-plz/release-plz/blob/main/Cargo.toml) — `tera = "2.0.0"`, `git-cliff-core = { version = "2.10.0", ... }`
  - git-cliff-core **2.10.0** [`Cargo.toml`](https://github.com/orhun/git-cliff/blob/v2.10.0/git-cliff-core/Cargo.toml) — `tera = "1.20.0"`
- GoReleaser **v2.17.1** [`internal/tmpl/tmpl.go`](https://github.com/goreleaser/goreleaser/blob/v2.17.1/internal/tmpl/tmpl.go) — `missingkey=error` at `Apply` and `ApplySingleEnvOnly`; 42 FuncMap keys including `readFile` / `mustReadFile`; no `exec`. **v2.18.0** (2026-08-24) adds `join` (43 keys).
- GoReleaser docs (live 2026-08-25; previous Pro URL 404s and is not cited):
  - [Releases](https://goreleaser.com/customization/publish/scm/) (updated 2026-08-23) — inline `header:` / `footer:` strings; `{ from_file.path }` / `{ from_url.url }` marked **GoReleaser Pro**
  - [Template Files](https://goreleaser.com/customization/general/templatefiles/) (updated 2026-08-11) — `template_files.src` “exclusively available with GoReleaser Pro”
  - [Templates](https://goreleaser.com/customization/templates/) — lists `readFile` / `mustReadFile`; no exec. `custom.goreleaser.com` does not resolve (DNS NXDOMAIN) — **unverified**
- knope [Customizing release notes](https://knope.tech/recipes/customizing-release-notes/) — `change_templates` are `$token` strings; first applicable template wins; missing variables skip that template
- semantic-release **v25.0.9** [`package.json`](https://github.com/semantic-release/semantic-release/blob/v25.0.9/package.json) — `@semantic-release/release-notes-generator` `^14.1.0`
- `@semantic-release/release-notes-generator` **14.1.1** [README](https://github.com/semantic-release/release-notes-generator/blob/v14.1.1/README.md) — `writerOpts` merge onto conventional-changelog-writer options
- `conventional-changelog-writer` **8.4.0** [`src/template.ts`](https://github.com/conventional-changelog/conventional-changelog/blob/conventional-changelog-writer-v8.4.0/packages/conventional-changelog-writer/src/template.ts) — `Handlebars.compile(mainTemplate, { noEscape: true })`; templates are strings, not paths. [README](https://github.com/conventional-changelog/conventional-changelog/blob/conventional-changelog-writer-v8.4.0/packages/conventional-changelog-writer/README.md): “If you are using handlebars template files, read files by yourself.”
- [Handlebars compile options](https://handlebarsjs.com/api-reference/compilation.html) — `strict` “throw rather than silently ignore missing fields”; writer does not set `strict`
- release-please **17.11.2** [`schemas/config.json`](https://github.com/googleapis/release-please/blob/v17.11.2/schemas/config.json) — `changelog-type` enum `default` | `github`; [`package.json`](https://github.com/googleapis/release-please/blob/v17.11.2/package.json) — `conventional-changelog-writer` `^6.0.0`
- changesets [Customize Changelog Format](https://changesets.dev/guide/customize-changelog-format) — `changelog` is a module path, package name, or `[module, options]`
- nx [Configure Changelog Format](https://nx.dev/docs/guides/nx-release/configure-changelog-format) — `renderer` is a JS/TS class extending `DefaultChangelogRenderer` (Nx 22+); not a changesets wrap

Earlier sources (mise, direnv, pnpm, GitHub Actions, minijinja) are unchanged from the 2026-08-18/19 pass and are not re-derived here.

## Findings

### Two traditions, split by host language

Rust and Go tools give users **template strings**; JavaScript tools give users a **code module** and skip templating entirely.

| Tool | Mechanism | Engine | Undefined variable |
|---|---|---|---|
| git-cliff 2.13.1 | inline strings in `cliff.toml` (`header` / `body` / `footer`) | Tera 1.20.1 | **inferred** from Tera 1.20.1 docs: errors on `{{ }}`; falsy in `{% if %}` |
| release-plz | inline strings | Tera 2 for its own fields; **Tera 1.20.0** through git-cliff-core 2.10.0 | **inferred** from those engines; not rendered through release-plz |
| GoReleaser OSS v2.17.1 | inline strings; FuncMap `readFile` / `mustReadFile` | Go `text/template` | **hard error** (`missingkey=error`) |
| GoReleaser Pro | `from_file` / `from_url` for header/footer; `template_files` | **inferred** same as OSS (docs only; Pro source is closed) | **inferred** same as OSS (docs only) |
| knope | `$summary` etc. via substring replace | none | skip that template, try the next |
| semantic-release (rng 14.1.1) | Handlebars strings through `writerOpts` | Handlebars (`noEscape: true`, not `strict`) | **inferred** from non-`strict`: missing fields silently ignored |
| release-please 17.11.2 | `changelog-type`: `default` \| `github` | **inferred** Handlebars (writer `^6` pin; 6.0.x lists `handlebars`; not in config) | n/a — users do not supply a template |
| changesets | JS or TS module path | none | n/a |
| nx | JS or TS `renderer` class (own, not changesets) | none | n/a |

**File-based** here means a first-class config value whose contents are a template *body* loaded from a path — oakum's `{ file = "notes.md" }`. It is not a config file that happens to contain an inline string, a JS module, a FuncMap helper that reads a file into an inline template, or a CLI flag that replaces `--body "$(cat file)"`.

Under that definition, **no surveyed free product ships file-based templates.** git-cliff 2.13.1 has no template-body path field — `header` / `body` / `footer` stay strings; `output` is the written changelog path. Unreleased `main` adds `--body-file` (CLI only). semantic-release's `writerOpts.mainTemplate` / `*Partial` are strings; the writer README (v8.4.0) says if you have Handlebars files, “read files by yourself.” GoReleaser OSS keeps header/footer as inline strings and documents `from_file` / `from_url` and `template_files` as Pro. `readFile` inside an inline string is a different feature. changesets and nx take code modules. knope and release-plz take inline strings. release-please does not expose a template.

That is why `{ file = "path" }` is differentiated rather than a copy. The Pro file/URL form is **documentation-only** — `github.com/goreleaser/goreleaser-pro` is public but is not the product source.

### Two design lessons worth taking

**Context is per-surface, not global.** GoReleaser's git fields are unavailable in the `env` section; artifact fields exist only in per-artifact scopes; `.Checksums` only in the release body. Strict undefined-checking is only tractable when the context is scoped — otherwise failures become a game of "which section am I in".

**Templates render; hooks execute; they are separate surfaces.** GoReleaser has 42 template functions at v2.17.1 (`internal/tmpl/tmpl.go`; 43 at v2.18.0 with `join`), including `readFile` and `mustReadFile`, and **no exec function**. Command execution lives in `before.hooks` / `after.hooks`. It is the most template-heavy tool in the survey and it draws that line hard.

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

The one CI-compatible pattern is default-deny plus allowlist — pnpm 10's `onlyBuiltDependencies`. **CVE-2025-69264**, *"pnpm v10+ Bypass 'Dependency lifecycle scripts execution disabled by default'"* (`CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:H/I:H/A:H` — 8.8 High, OSV, re-read 2026-08-19), bypassed it entirely for a year, because git-hosted dependencies never consulted the allowlist. An allowlist checked at each call site rather than at one chokepoint is a bug waiting to happen.

### minijinja specifics

Stable **2.24.0** and `3.0.0-alpha.0` were both published 2026-08-12, and 2.24.0 is still the stable line (crates.io, 2026-08-19). `debug` is in the `default` feature set alongside `builtins`, `deserialization`, `macros`, `multi_template`, `adjacent_loop_items`, `std_collections`, and `serde`; `loader` and `fuel` are not.

`UndefinedBehavior` has four variants, and **`Strict` also errors on `{% if undefined %}`**. Release text branches on legitimately absent fields constantly — no previous version on a first release, no breaking changes on a patch — so `Strict` forces `is defined` guards throughout every user template. The four variants are `Lenient`, `Chainable`, `SemiStrict`, and `Strict`. **`SemiStrict` fails on printing, iteration, attribute access, and string coercion in filters and functions, while treating undefined as falsy in `{% if %}`** — its own doc comment reads *"Like strict, but does not error when the undefined is checked for truthyness"* (`minijinja` 2.24.0 `src/utils.rs:209`). That is both the property wanted and exactly Tera's behavior, so intuitions carry over from git-cliff and release-plz.

Three traps:

- **Auto-escape is chosen by file extension, behind a non-default feature.** With the `json` feature enabled — not in `default`, per the feature list above — `.json`, `.json5`, `.js`, `.yaml`, and `.yml` map to JSON escaping, and `.j2`, `.jinja`, and `.jinja2` are stripped before the match, so `pr-body.yml.j2` lands there too (`src/defaults.rs:18,39-44`). On a default build `pr-body.yml` is `AutoEscape::None` already. Set the callback to `None` unconditionally so the build's feature set stops mattering.
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

## Unverified (2026-08-25)

Do not read these as checked.

- Empirical `{{ missing }}` / `{% if missing %}` against a git-cliff or Handlebars binary. Table cells are labeled inferred from Tera 1.20.1 docs and Handlebars non-`strict`.
- Whether git-cliff's `{% include "file.tera" %}` can reach the disk. Source shows `Tera::default()` plus `add_raw_template` and no directory loader; that combination was not executed.
- GoReleaser Pro implementation of `from_file` / `from_url`. Live docs mark them Pro; the product source is closed. Engine and undefined cells are labeled inferred.
- writer **6.x** compile options and render path. `conventional-changelog-writer` **6.0.1** `package.json` lists `handlebars` `^4.7.7`; the compile site was not read. Latest writer **9.x** dropped Handlebars for JS render functions and is **not** this pin.
- release-plz's own undefined-variable behavior. It inherits Tera 2 (own fields) and Tera 1 (git-cliff); those engines were not re-rendered through release-plz.
- Writer 9.2.1 / `@semantic-release/release-notes-generator` 15.0.0-beta.2 (2026-08-24) as a stable semantic-release path. They exist; they are not what semantic-release 25's `^14.1.0` rng line ships.
