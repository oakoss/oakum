# Registry publish semantics and partial-failure handling

- Date: 2026-08-18
- Author: Claude Code research agent
- Scope: What npm and crates.io report when a version already exists, how stale registry reads affect re-runs, and how seven existing tools handle a monorepo publish that fails halfway.

## Question

When a publish run covers five packages and the third fails, what state should the repository end up in, and what signals can a tool actually rely on to make a re-run safe?

## Sources

npm 11.17.0, cargo 1.94.1, Verdaccio 6.10.0, live reads against registry.npmjs.org and index.crates.io. Source read from npm/cli, rust-lang/cargo, rust-lang/crates.io, and the seven tools compared below.

## Findings

### "Already published" is not machine-distinguishable on npm

npm's error `code` is `` `E${res.status}` `` — the HTTP status, carrying no semantics. registry.npmjs.org returns **403 for both** "you already published this version" and "you don't own this package", so both are `E403`.

Since **npm 11.2.0** the common case never reaches the registry: `publish.js` pre-checks the packument client-side and throws a plain `Error` with **no `code` property**:

```json
{ "error": { "summary": "You cannot publish over the previously published versions: 1.0.0.", "detail": "" } }
```

That check landed in 11.1.0, was reverted through 11.1.4, and returned in 11.2.0. It also excludes prereleases and deprecated versions from its comparison and swallows fetch errors into an empty list, so a duplicate *prerelease* still reaches the registry and produces `E409` against Verdaccio.

`EPUBLISHCONFLICT` is vestigial — one occurrence in npm 11.17.0, in a formatter for a code nothing throws.

Third-party registries differ again: Verdaccio returns **409** `this package is already present`.

### crates.io is distinguishable, but cargo discards it

| Condition | crates.io HTTP | Body |
|---|---|---|
| duplicate version | **400** | ``crate version `1.0.0` is already uploaded`` |
| not an owner | **403** | "this crate exists but you don't seem to be an owner" |

Cargo collapses both into `the remote server responded with an error (status ...)` and exits 101 for every user error. Reaching the crates.io API directly preserves the distinction; going through the CLI does not.

Cargo also fails *before* uploading, through an index pre-check (`verify_unpublished`, run for all selected packages up front). A **yanked** version still counts as existing.

### Registry reads are stale by design

`registry.npmjs.org` returns `cache-control: public, max-age=300` with `cf-cache-status: HIT` — up to five minutes of stale packument reads. `index.crates.io` returns `max-age=600`.

Cargo solved this in-band: **since 1.66, `cargo publish` blocks until the package appears in the index**, polling at 1-second intervals with a **hardcoded 60-second timeout**. `publish.timeout` remains nightly-gated; `-Zpublish-timeout` is rejected on stable.

No JavaScript tool surveyed waits for packument propagation between dependency levels.

### How seven tools behave on partial failure

| Tool | On failure | Topological | Tag vs publish | Re-run safe | Detection | Preflight |
|---|---|---|---|---|---|---|
| changesets 3.0.0 | stop at chunk | yes | after | yes | string only | `npm info` |
| changesets 2.31.1 | stop | yes | after | **broken vs npm ≥ 11.2** | `E403` **and** string | `npm info` |
| lerna 10.0.0 | stop | yes (default) | **before, pushed** | yes | code or string | npmjs only |
| nx 23.1.1 | **continue** | yes (groups) | **before, pushed** | yes | code or string, handles Verdaccio | `npm view` |
| cargo-workspaces | stop | yes | **before, pushed** | yes | index preflight | index |
| release-plz | stop | yes | after | yes (three layers) | both cargo and crates.io strings | tag + index |
| cargo-smart-release | stop | yes | after | **no** | none — blind 3× retry | none |

`knope` does not publish to any registry. Its `Release` step creates git tags and forge releases only; publishing is delegated to CI. It is not a data point here.

**changesets 2.31.1 has a live defect** against npm ≥ 11.2.0: it guards on `json.error.code === "E403"`, and the modern client-side error has no code, so an already-published package is reported as *failed*. Fixed in 3.0.0 by dropping the code requirement.

**nx has the most portable detector**, searching `summary`, `detail`, `message`, `body.error`, raw stderr, and raw stdout, and accepting `EPUBLISHCONFLICT`, the npmjs phrasing, and Verdaccio's `E409`.

**release-plz has the best idempotency model** — three independent layers:

1. `if repo.tag_exists(&git_tag) { return Ok(None) }` — the git tag is the resume marker
2. an index query before shelling out
3. post-hoc string matching on both the crates.io and cargo phrasings

### `cargo publish --workspace` is not resumable

Stabilized in **cargo 1.90**. The changelog says plainly: *"`cargo publish` is still non-atomic at this time. If there is a server side error during the publish, the workspace will be left in a partially published state."*

Because `verify_unpublished` runs for all selected packages before anything uploads, re-running after a partial success aborts on the crates that already landed. Cargo's own test asserts this. Its `--keep-going` affects only the build phase, not the publish loop.

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
