# Registry publish semantics and partial-failure handling

- Date: 2026-08-18, revised 2026-09-02
- Author: Jace Babin
- Scope: What npm and crates.io report when a version already exists, how stale registry reads affect re-runs, and how seven existing tools handle a monorepo publish that fails halfway.

## Question

When a publish run covers five packages and the third fails, what state should the repository end up in, and what signals can a tool actually rely on to make a re-run safe?

## Sources

npm 11.17.0, cargo 1.97.1, Verdaccio 6.10.0, live reads against registry.npmjs.org and index.crates.io. Source read from npm/cli, rust-lang/cargo, rust-lang/crates.io, and the seven tools compared below.

**Re-verified 2026-08-19** against cargo 1.97.1, npm 11.17.0, and live registry reads. **Partial-failure table re-derived 2026-09-02 from published source** (`lerna@10.0.0`, `nx@23.1.1` / `@nx/js@23.1.1`, `cargo-workspaces@0.4.2`, `cargo-smart-release@0.21.13`, `@changesets/cli@3.0.0` / `2.31.1`, `release_plz_core@0.37.0`) — not from live partial-failure publishes.

## Findings

### "Already published" is not machine-distinguishable on npm

npm's error `code` is `` `E${res.status}` `` (`npm-registry-fetch` 19.1.1, bundled in npm 11.17.0, `lib/errors.js:29`) — the HTTP status, carrying no semantics. registry.npmjs.org returns **403 for both** "you already published this version" and "you don't own this package", so both are `E403`.

Since **npm 11.2.0** the common case never reaches the registry: `publish.js` pre-checks the packument client-side and throws a plain `Error` with **no `code` property**:

```json
{ "error": { "summary": "You cannot publish over the previously published versions: 1.0.0.", "detail": "" } }
```

That check landed in 11.1.0, was reverted through 11.1.4, and returned in 11.2.0. It also excludes prereleases and deprecated versions from its comparison and swallows fetch errors into an empty list, so a duplicate *prerelease* still reaches the registry and produces `E409` against Verdaccio.

`EPUBLISHCONFLICT` is vestigial — one occurrence in npm 11.17.0, a `case` arm in `lib/utils/error-message.js:199` formatting a code nothing throws.

Third-party registries differ again: Verdaccio returns **409** `this package is already present`.

### crates.io is distinguishable, but cargo discards it

| Condition | crates.io HTTP | Body |
|---|---|---|
| duplicate version | **400** | ``crate version `1.0.0` is already uploaded`` |
| not an owner | **403** | "this crate exists but you don't seem to be an owner" |

Cargo collapses both into `the remote server responded with an error (status ...)` and exits 101 for every user error. Reaching the crates.io API directly preserves the distinction; going through the CLI does not.

Cargo also fails *before* uploading, through an index pre-check (`verify_unpublished`, run for all selected packages up front). A **yanked** version still counts as existing.

### Registry reads are stale by design

`registry.npmjs.org` returns `cache-control: public, max-age=300` with `cf-cache-status: HIT` — up to five minutes of stale packument reads. `index.crates.io` returns `public,max-age=600`. Both re-read live 2026-08-19.

Cargo solved this in-band: **since 1.66, `cargo publish` blocks until the package appears in the index**, polling at 1-second intervals with a **hardcoded 60-second timeout**. `publish.timeout` remains nightly-gated; on cargo 1.97.1 stable, `-Zpublish-timeout` is rejected with *"the `-Z` flag is only accepted on the nightly channel of Cargo"*.

No JavaScript tool surveyed waits for packument propagation between dependency levels.

### How seven tools behave on partial failure

Re-derived 2026-09-02 from the published artifacts named in each row (npm pack / crates.io crate tarballs). Cells describe what those sources implement; this pass did not re-run live monorepo partial-failure publishes. Every cell below is cited or corrected from that source pass.

| Tool | On failure | Topological | Tag vs publish | Re-run safe | Detection | Preflight |
|---|---|---|---|---|---|---|
| changesets 3.0.0 | stop at chunk | yes | after | yes | string only | `npm info` |
| changesets 2.31.1 | stop | yes | after | **broken vs npm ≥ 11.2** | `E403` **and** string | `npm info` |
| lerna 10.0.0 | stop | yes (default) | **before, pushed** | yes | code or string | npmjs access only |
| nx 23.1.1 | **continue** | yes (groups) | **before** (push default iff `createRelease`) | yes | code or string, handles Verdaccio | `npm view` |
| cargo-workspaces 0.4.2 | stop | yes | **before, pushed** | yes | index preflight | index |
| release-plz | stop | yes | after | yes (three layers) | both cargo and crates.io strings | tag + index |
| cargo-smart-release 0.21.13 | stop | yes | after | **no** | none — blind 3× retry | none |

`knope` does not publish to any registry. Its `Release` step creates git tags and forge releases only; publishing is delegated to CI. It is not a data point here.

**changesets 2.31.1 has a live defect** against npm ≥ 11.2.0: it guards on `json.error.code === "E403"`, and the modern client-side error has no code, so an already-published package is reported as *failed* (`@changesets/cli@2.31.1` `dist/changesets-cli.esm.js:927`). Fixed in 3.0.0 by matching the `"cannot publish over the previously published"` string alone (`@changesets/cli@3.0.0` `dist/getPublishPlan.mjs:65-66,198-201`). 3.0.0 also stops at the failing dependency chunk (`dist/publish.mjs` `break publishChunks`) and creates git tags after the publish loop.

**lerna 10.0.0** defaults to topological publish (`sort !== false`), soft-succeeds on `E409` / `EPUBLISHCONFLICT` / `E403` with the npmjs body phrase, and rethrows other publish errors (`dist/chunk-EB6RZPL6.js:226,848-863`). The default bump path runs the version command first (`chunk-EB6RZPL6.js:357`), which commits and tags (`chunk-TN6YF3FA.js:742` `commitAndTagUpdates`) and pushes with `--follow-tags` (`chunk-7CE6W7VR.js:9`) before packing. `prepareRegistryActions` skips username/access checks for any registry other than `https://registry.npmjs.org/` (`chunk-EB6RZPL6.js:631-634`) — that is the “npmjs only” preflight; packument unpublished detection for `from-package` still runs elsewhere.

**nx 23.1.1** continues across release groups, merging per-project results and exiting non-zero only after the full pass (`nx` `dist/src/command-line/release/publish.js:24-27,118-133`). Combined `nx release` tags before `releasePublish` (`release.js:172-185,266-270`). Git push defaults to false unless changelog `createRelease` is enabled (`config/config.js:154-159`); explicit `git.push` / `changelog.git.push: true` can still enable push without it. The earlier “before, pushed” cell overstated the default. `@nx/js` `release-publish.impl.js:25-40` accepts `EPUBLISHCONFLICT`, the npmjs string, and Verdaccio's `E409` + `this package is already present`, scanning summary/detail/message/body/stderr/stdout; preflight uses `npm view` (or `bun info`).

**cargo-workspaces 0.4.2** versions with commit + tag + `git push --follow-tags` before the publish loop (`src/utils/git.rs:197-273`), walks a `dag` order, stops on hard publish failure (`src/publish.rs:180-184`), and skips versions already present in the index (`is_published`, `:121-123`).

**cargo-smart-release 0.21.13** publishes then tags each success, pushes tags after the loop (`src/command/release/mod.rs:453-471`), and stops on the first publish error. Each `cargo publish` is retried blindly up to three times with no already-published classifier (`src/command/release/cargo.rs:28-68`) — a re-run after a partial success is not safe.

**nx has the most portable detector**, searching `summary`, `detail`, `message`, `body.error`, raw stderr, and raw stdout, and accepting `EPUBLISHCONFLICT`, the npmjs phrasing, and Verdaccio's `E409`.

**release-plz has the best idempotency model** — three independent layers:

1. `if repo.tag_exists(&git_tag)? { … return Ok(None); }` — the git tag is the resume marker, and the skip is logged as `Already published - Tag {} already exists` (`release_plz_core` 0.37.0 `src/command/release.rs:629`)
2. an index query before shelling out
3. post-hoc string matching on both the crates.io and cargo phrasings

### `cargo publish --workspace` is not resumable

Stabilized in **cargo 1.90** (2025-09-18, [#15636](https://github.com/rust-lang/cargo/pull/15636) and [#15711](https://github.com/rust-lang/cargo/pull/15711)). The changelog says plainly: *"Note that `cargo publish` is still non-atomic at this time. If there is a server side error during the publish, the workspace will be left in a partially published state."* (Cargo Book changelog, §Cargo 1.90, read 2026-08-19.)

Because `verify_unpublished` runs for all selected packages before anything uploads, re-running after a partial success aborts on the crates that already landed. Cargo's own test asserts this — `tests/testsuite/publish.rs::workspace_missing_dependency` expects exit 101 and `[ERROR] crate a@0.0.1 already exists on crates.io index`. Its `--keep-going` affects only the build phase, not the publish loop.

`npm publish --workspaces` is worse as a primitive: a sequential loop in glob order with **no dependency ordering**, throwing on first failure.

## Conclusions

Ordering is table stakes; every serious tool does it. The differentiators are propagation waits between levels — which no JavaScript tool implements despite a measured 300-second cache window — and a resume marker that does not depend on registry state.

## Implications / actions

The publish path, when it lands:

- **Preflight the whole set** before publishing anything: credentials, every target version absent, every manifest valid. Most partial-failure scenarios become a clean abort with nothing to resume.
- **Publish in topological order**, waiting for each level to become visible before starting the next — by polling until the version appears, not by sleeping out the cache TTL.
- **Publish, then tag.** Tag-first leaves a permanent hole in the version history when the publish fails; publish-first leaves a published version with no tag, which a re-run repairs.
- **Use tag existence as the per-package skip gate** — local, deterministic, and immune to every registry inconsistency above.
- **Classify errors defensively** as the race-window fallback: accept `E403`, `E409`, `EPUBLISHCONFLICT`, and *no code*, matching at least `cannot publish over the previously published`, `this package is already present`, `is already uploaded`, and `already exists on`.
- **Stop at the first hard failure** and report the remaining set, the way cargo does with `note: the following crates have not been published yet:`.
- **Drive per-package publishes**, not `cargo publish --workspace` or `npm publish --workspaces`.

## Open questions

- Whether to reach the crates.io API directly for the clean 400-vs-403 signal. Under OIDC the npm side is settled — the npm CLI performs the token exchange itself, so a native HTTP client would have to reimplement it — but crates.io's exchange happens in a separate workflow step, leaving a native client viable there.
