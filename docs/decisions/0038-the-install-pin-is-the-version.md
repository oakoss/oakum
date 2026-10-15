# Read oakum's version from the install pin; drop `tool-version`

- Status: accepted
- Date: 2026-10-14
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0007](0007-pin-the-tool-version-in-config.md) records oakum's version twice: `tool-version` in `.changeset/_config.toml`, and the install site (`package.json`, a workflow install line, `.mise.toml`). `check` makes the two agree. Renovate edits only the install site, so every oakum release fails CI in every consuming repository until someone runs `oakum upgrade` on the Renovate branch. ADR-0007 named this cost and accepted it. The first real occurrence, in claude-plugins on the 0.5.0 bump:

```text
unverified: install pin is 0.5.0 (package.json) but `tool-version` is `0.4.0`
```

Of the eight tools in [tool-version-pinning.md](../research/tool-version-pinning.md), only cargo-dist keeps a version in config, and it can do so because it generates the CI that installs it: one edit updates both. changesets, nx, and bumpy pin through the repository's own dependency; semantic-release pins in the workflow; release-plz and release-please pin through the action, and knope through the action's `version:` input. Each has one place to edit. Oakum copied cargo-dist's config pin without its CI generation, which [ADR-0003](0003-write-only-what-a-command-owns.md) rules out, and the second copy became the user's job.

How should oakum keep its behavior from changing without a reviewed commit, with one place to edit?

## Decision Drivers

- Behavior must not change without a commit someone reviewed (ADR-0007, unchanged).
- A dependency update is one edit in one pull request, as it is for every peer except cargo-dist.
- [ADR-0004](0004-derive-facts-configure-preference.md): config holds preference, facts are derived. Where an install pin exists, `tool-version` restates it.
- A look that did not happen is `unverified`, never fine.

## Considered Options

- **Keep ADR-0007**, and document a Renovate rule that bumps `tool-version` alongside the install pin
- **Install pin first, `tool-version` as a fallback** when no pin is readable
- **The install pin is the only source**; `tool-version` is removed

## Decision Outcome

Chosen option: **the install pin is the only source**, because it is the one option with one place to edit. The Renovate rule leaves two copies and makes every consuming repository maintain a regex that can match nothing without erroring ([renovate-rule-matching.md](../research/renovate-rule-matching.md)). A fallback is two sources with a precedence rule: a repository holding both drifts, and the precedence rule hides it.

The Renovate pull request is the reviewed commit ADR-0007 asks for. A second edit in the same pull request adds no review that the first did not get.

### The version

The install sites ADR-0007 lists are unchanged: versioned install lines in `.github/workflows` and `.github/actions`, an exact `oakum` or `@oakoss/oakum` in the root `package.json`, an exact pin in `.mise.toml` / `mise.toml`, and a Cargo workspace member named `oakum`. `run: oakum check` and `pnpm exec oakum` remain invocations, not pins.

- `check`'s install-pin look verifies that the pins agree with each other. Pins that disagree are `unverified`, as today.
- The write gate (`add`, `generate`, `version`, `ci version-pr`, `release`, `init`, `migrate`) compared the binary to `tool-version` and now compares it to the pins. A disagreement names the fix for its direction: a binary older than the pin names installing the pinned version (and `pnpm exec oakum` where `package.json` pins it); a binary newer than the pin names raising the pin.
- No pin found: `check` and `release` are `unverified`, as today. The rest of the write gate refuses too, naming the cheapest pin for the repository: `pnpm add -D @oakoss/oakum@<version>` in an npm workspace, otherwise an `oakum` entry under `[tools]` in `.mise.toml`. `init` and `migrate` are how a repository adopts oakum, often before any pin exists, so a missing pin does not stop them; they print the same advice as a remaining step. Neither writes a pin, which ADR-0003 forbids.
- `check`, `status`, and the hidden read commands stay off the write gate ([ADR-0007](0007-pin-the-tool-version-in-config.md), 2026-08-22 amendment).
- The version-PR body and the changelog footer stamp the binary's version instead of `tool-version`. Behind the gate it equals the pin, so the stamped value does not change.
- Self-host loses its lockstep write. The Cargo member named `oakum` is the pin and the binary built from it, so `version` no longer writes `tool-version` (ADR-0007, 2026-09-01 amendment; [ADR-0023](0023-name-every-verb-and-what-it-owns.md)).
- Never self-upgrade in CI (ADR-0007's confirmation, unchanged).

### Retiring `tool-version`

`deny_unknown_fields` would turn the removed key into a hard error, and the Renovate pull request for the release that removes it would fail the way this ADR exists to stop. `tool-version` is instead a **deprecated** key: it parses, nothing reads it, and every command that loads config warns on stderr that it is no longer read and that `oakum upgrade` removes it. The warning changes no exit code. Removing the deprecated key is the first migration in `upgrade`'s registry. Unknown keys stay a hard error.

### The schema

`_config.toml` names its schema with taplo's `#:schema` directive. The schema changes often: the committed `.changeset/_schema.json` in this repository differs across 3 of the 6 releases from 0.1.4 to 0.5.0, a patch among them, since description text is part of it. Failing `check` on a stale schema would bring back the second edit on half of all bumps.

- **npm workspaces with the devDependency pin** point the directive into `node_modules`, as bumpy and nx do, so the schema is whatever is installed and nothing is committed. This needs the published `@oakoss/oakum` package to carry the schema. Today it is cargo-dist's fetcher and does not (the 0.5.0 tarball holds `package.json`, `README.md`, `run-oakum.js`, `binary.js`, `binary-install.js`, `install.js`, `.gitignore`, `LICENSE`, `npm-shrinkwrap.json`, and `CHANGELOG.md`). Until it does, npm workspaces keep the committed file and are treated like every other install below (`okm-zd1p`).
- **Every other install** (Cargo, Homebrew, mise, a global npm install) keeps the committed `.changeset/_schema.json`. `check` reports a stale one as a note that does not change the exit code, naming `oakum upgrade`. The binary validates config on every run, so a stale schema degrades editor hints and nothing else.

`upgrade` keeps two jobs: run migrations, and regenerate the committed schema. It no longer writes a version.

### Consequences

- Good, because a Renovate or Dependabot bump of a repository's only oakum pin is one edit and goes green.
- Good, because `tool-version` was a key restating a fact ADR-0004 says to derive; removing it removes a class of drift rather than reporting it.
- Good, because the mismatch message now names the fix in both directions, which the pin-to-config message did not.
- Bad, because a Homebrew or global-npm user with no pin in the repository was allowed through `add`, `generate`, `version`, and `ci version-pr` by `tool-version` alone (`release` already refused) and is now refused until the repository carries a pin. That is a refusal on an input that worked: a breaking change under [ADR-0022](0022-zero-major-versioning.md).
- Bad, because a repository that pins oakum twice (a `package.json` devDependency and a workflow install line) can still get one Renovate pull request per pin, each `unverified` alone. That Renovate splits them by manager is inferred, not measured. For npm workspaces `init` stops printing its second pin, `npm i -g @oakoss/oakum@<version>` (`okm-3rfh`); guidance for repositories that keep both is `okm-0wia`.
- Bad, because an exact pin is still required. A range such as `^0.5.0` stays `unverified`, though the lockfile records the resolved version. Reading lockfiles is a separate decision (`okm-kepc`).
- Neutral, because the deprecation warning fires in every repository until `oakum upgrade` runs once.

## Amendment (2026-10-14)

Settled while planning `okm-muxr`, against the code at 0.5.1.

**A missing pin, per command.** `add`, `generate`, `version`, and `ci version-pr` refuse with exit 2 (`unverified`), as a missing config does. `release` passes the gate on a missing pin and lets its install-pin precondition report it, as it does today: the gate never read pins, so "`release` already refused" above is true of that precondition, not of the gate. Agreeing pins that differ from the binary stop `release` at the gate with exit 1, like the other writers. `init` and `migrate` check pins on every run, a fresh repository included, since a pin can exist before any config does. A missing pin, pins that disagree, or a pin file that cannot be read do not stop them: they write their files, `migrate` lists the pin first among its remaining steps, and `init` prints it as the next step. A missing pin then exits 0, since adopting oakum before pinning it is the normal order. Pins that disagree or cannot be read exit 2 after the writes, as `migrate` already does when it cannot run the source tool, so a caller can tell a look that failed from one that found nothing. Only agreeing pins that differ from the binary stop them before any write, with exit 1. When no pin exists and the workflow they print installs oakum at a version, that line is the pin, and the pin step says to commit it rather than add a second one.

**A pin the reader cannot settle stops `add`, `generate`, `version`, and `ci version-pr`.** A pin file that cannot be parsed, an unversioned install line, or a range such as `^0.5.0` was `unverified` only in `check` and `release`; since these commands now read the pin, each stops them with exit 2. A range let them through at 0.5.1, so this is a second refusal on an input that worked, breaking under [ADR-0022](0022-zero-major-versioning.md) like the missing-pin refusal. `okm-kepc` is what would accept a range.

**`upgrade` is gated like `add`, except that a missing pin lets it continue.** Agreeing pins that differ from the binary stop it with exit 1; pins that disagree or cannot be read stop it with exit 2. A missing pin prints the no-pin advice on stderr, without changing the exit code, so the run says it was not checked against one. Ungated, a binary newer than the pin regenerates a schema that the pinned binary then reports as stale. The upgrade path stays one step: Renovate raises the pin, and `oakum upgrade` run through that pin (`pnpm exec oakum`, or mise's shim) is the new binary.

**A dangling symlink where a pin source would be is unverified, not absent.** A `.github/workflows` or `.github/actions` entry, a mise file, or `package.json` that is a symlink to a missing target could hide a pin that disagrees with the others, so the reader reports it as a pin that cannot be read, and each command treats it as one.

**The mise pin is `github:oakoss/oakum`.** `mise registry` has no bare `oakum`, so the bare key above does not resolve. `cargo:oakum`, `npm:@oakoss/oakum`, and `github:oakoss/oakum` all resolve (`mise ls-remote`), and Renovate 44.48.3's mise manager updates all three: a local `--dry-run=lookup` run extracted each and planned an update to 0.5.1 (`github-releases`, `crate`, and `npm` datasources). `github:` installs the prebuilt release binary, where `cargo:` compiles from source and `npm:` needs Node, so the no-pin advice outside an npm workspace names `"github:oakoss/oakum" = "<version>"` under `[tools]`, and `check` reads `github:oakoss/oakum` as a pin beside the keys it already reads.

**The npm pin advice approves oakum's build script.** Without an `allowBuilds` entry for `@oakoss/oakum`, pnpm 11.25.0 and 12.11.2 exit 1 with `ERR_PNPM_IGNORED_BUILDS` on `pnpm add` and on a CI `pnpm install`; pnpm 10.34.5 only warns. The advice is `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@<version>`: on all three it saves the exact version and writes `allowBuilds`, and a CI `pnpm install` then exits 0.

**An existing `_schema.json` is compared whether or not `#:schema` names it; a missing one is reported like a stale one only when `_config.toml`'s `#:schema` line names it.** `upgrade` keeps the file either way, so a stale one exists to be found. With no such line and no file, `check` says nothing about the schema. When the install pins agree on a version other than the running binary's, `check` does not compare: it notes that the schema was not compared and names both versions, since this binary's schema is not the one the repository expects and `oakum upgrade` from it refuses.

**The npm workflow `init` prints keeps `npm i -g @oakoss/oakum@<version>` for now.** Installing through the repository's pin (`pnpm install`, then `pnpm exec oakum`) waits on `okm-6ux4`: `version` rewrites a workspace range in `package.json` without updating `pnpm-lock.yaml`, so a frozen CI install then fails (measured on pnpm 10.34.5, 11.25.0, and 12.11.2). The printed install line is a pin that agrees with the devDependency, so the gate holds; the cost is a second Renovate pull request. `okm-jg5z` decides the workflow's form, the half of `okm-3rfh` this decision names.

## More Information

- Supersedes the pin model of [ADR-0007](0007-pin-the-tool-version-in-config.md): `tool-version`, its gate, and its self-host write. ADR-0007's install sites, three-outcome verification, `deny_unknown_fields`, version stamping, and "never self-upgrade in CI" stand.
- [tool-version-pinning.md](../research/tool-version-pinning.md) — the peer survey; its "Implications" section describes the ADR-0007 model this replaces.
- [install-pin-sources.md](../research/install-pin-sources.md) — what `check` reads as a pin.
- [claude-plugins-cutover-2026-09-10.md](../research/claude-plugins-cutover-2026-09-10.md) — release-age exemptions as a further per-bump edit.
- Open: whether cargo-dist can add `schema.json` to the npm fetcher package, or a release step must, and whether taplo resolves a `#:schema` path that leaves `.changeset/` (`okm-zd1p`).
- Open: whether the pnpm release-age exclude accepts a bare package name, which would remove the per-bump exemption edit (`okm-0wia`).
- Implemented by `okm-muxr` (2026-10-15), which also updated ADR-0023's ownership table, `AGENTS.md`, `docs/contributing/invariants.md`, the guides, and the specs.
