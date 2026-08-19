# GitHub's release path: tagging, triggering, and verification

- Date: 2026-08-18, revised 2026-08-19
- Author: Jace Babin
- Scope: what GitHub actually does when a release tool pushes tags and creates releases, and which behaviors would silently break a multi-package release

## Question

Oakum stops at the tag and expects a downstream workflow to react ([ADR-0011](../decisions/0011-stop-at-the-tag.md)). Which GitHub behaviors sit between "we pushed a tag" and "the workflow ran", and which of them fail quietly?

## Sources

- GitHub Actions and REST/GraphQL API documentation, read 2026-08-18; the three-tag limit re-read 2026-08-19 at *Events that trigger workflows*
- release-plz source, `release-plz/release-plz`: `crates/release_plz_core/src/git/github_graphql.rs` (`release_plz_core` 0.37.0)
- Cargo book, [specifying dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html), and [pnpm workspaces](https://pnpm.io/workspaces), both re-read 2026-08-18

## Findings

### Pushing more than three tags at once suppresses the event

GitHub documents it plainly, on the `push` event: *"Events will not be created for tags when more than three tags are pushed at once."* The limit is not unique to `push` — `create` and `delete` carry their own variants on the same page, *"An event will not be created when you create more than three tags at once"* and the matching sentence for deletion. Those three are the direct tag events, so switching a workflow from `push` to `create` does not escape the cap. The `release` event carries no such note, and neither does any indirect route such as `workflow_run`; the docs do not support assuming either way.

This is not an edge case here. claude-plugins has three plugins today; a fourth makes the first four-package release push four tags, and the tag events are suppressed — on the safe reading, **for the whole push**, so no workflow runs at all. Nothing errors. The release looks complete and no artifacts are built.

**Push tags one at a time, always.** Not "when there are more than three" — a conditional that only activates on large releases is a conditional that is never tested.

### An API-created release may not emit a `push` event

A `release` event fires when a release is created through the API. The documentation is **silent** on whether the tag that release creates also emits a `push` event — and cargo-dist listens on `push` with a `tags` filter.

Silence is not permission. The safe ordering, all documented individually:

1. Create the version commit via the GraphQL `createCommitOnBranch` mutation as the App, which yields a signed commit
2. Push the tag with git, using the App token — documented as the way automation triggers workflows
3. Create the release against the tag that now exists

### The `GITHUB_TOKEN` suppression rule is narrower than it reads

`GITHUB_TOKEN` is itself an App installation token, and the rule that a token's own pushes do not trigger workflows is scoped to **the repository's** token specifically. A separate App's installation token does not fall under it, which is what makes step 2 above work.

### There is no ref or tag filter on the workflow-runs API

To find the run a tag triggered, resolve the tag to a SHA and filter on `head_sha`. The one endpoint that documents accepting `tags/NAME` directly is `GET /repos/{owner}/{repo}/commits/{ref}/check-runs`.

### No latency SLA exists for run creation

A run can appear seconds or minutes after the push. Poll with bounded exponential backoff, using the ETag and `If-None-Match` rather than `x-poll-interval` — that header belongs to the Events API and is not returned by `/actions/runs` ([downstream-handoff.md](downstream-handoff.md)) — and report **three** states — triggered, not triggered, and `unverified` — never collapsing "we did not look long enough" into "it is fine".

### Skip annotations suppress workflows for the commit that carries them

`[skip ci]` and its variants in a commit message stop workflows for that commit. A version commit carrying one would tag successfully and trigger nothing. Refuse to tag a commit whose message contains a skip annotation.

## Conclusions

Four of the six findings above are silent failures: the release completes, the output looks correct, and nothing downstream runs. That shape — success reported, work undelivered — is the same failure that motivated the delivery-artifact rule in [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md). The verification step is not optional polish; it is what distinguishes a release from a hope.

## Implications / actions

- Tag pushes are serialized, unconditionally.
- Refusing to tag a commit with a skip annotation is a precondition, checked before anything is written.
- The `unverified` state must exist in the output types from the start. Retrofitting a third state into code that assumed two is where it gets collapsed back into "ok".
- release-plz already implements the signed-commit trick in Rust, readable today in `release-plz/release-plz` at `crates/release_plz_core/src/git/github_graphql.rs`. The doc comment above `commit_changes` (`release_plz_core` 0.37.0, `src/git/github_graphql.rs:13`, re-read 2026-08-19): *"We use this API, because it gives the \"Verified\" status to the commit without a GPG key."*

## Open questions

- Whether an API-created release emits `push` in practice. Testable on a throwaway repository, and worth testing rather than designing around indefinitely.
- Whether the three-tag limit counts tags per push invocation or per ref update batch.
- Whether exceeding it drops only tags four and up or suppresses the event for the whole push. GitHub's sentence does not say; the whole-push reading is the safe one and is what the guidance above assumes.
