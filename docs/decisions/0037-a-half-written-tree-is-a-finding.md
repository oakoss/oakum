# Exit 1 when a write set leaves the tree changed

- Status: accepted
- Date: 2026-09-20
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0034](0034-exit-two-for-unverified.md) gives `Unverified` exit `2` and everything else exit `1`. It does not say which side a *partial write* falls on. `commit_write_set` rolls back when a write or delete fails, and rollback can itself fail: a file it could not restore, a file it deleted and could not put back, a staging file it could not remove, a directory it could not read. Today every one of those exits `1`, identically to a rollback that put the tree back exactly as it was — only the prose differs.

`okm-404.60` proposed giving the half-written case exit `2`, on the ground that "the work landed partially and the tree is not what the caller asked for". Is a tree a failed run left changed a finding, or a look that did not happen?

## Decision Drivers

- ADR-0034's split is the project's own: *"The split that carries meaning is between a finding and a look that did not happen."*
- The same ADR already rules out the proposed ground: *"The code classifies the **outcome**, not whether anything was written. `migrate` can exit `2` having applied every file and failed only to verify it, and can exit `2` having aborted before touching the tree."* Partial writing is orthogonal to the code.
- [ADR-0035](0035-a-finding-outranks-an-unverified-look.md) documents what callers do with each number, and `2` is the retry branch.
- `version` has no `--json`. Measured: `oakum version --help` mentions `--json` zero times, so the exit code is its only machine-readable channel and a wrong number has nowhere to be corrected.

## Considered Options

- **Exit 1 — a tree left changed is a finding**
- **Exit 2 — a tree left changed is unverified** (what `okm-404.60` asked for)
- **A new `Outcome` variant** for partial writes

## Decision Outcome

Chosen option: **exit 1 — a tree left changed is a finding**, because oakum *measured* it. A restore that returned `Err` is not a look oakum failed to take; it is a look that came back with an answer, and the answer was "this did not work". Three of the four things that leave a tree changed — a file whose restore failed, a create that could not be removed, a staging file that could not be swept — are facts established on disk.

**Exit 2 would make a measured harm worse.** ADR-0035's table gives the two plausible scripts, and the first is `retry on 2, fail on 1`. Measured 2026-09-20, with the real binary: a `version` run whose rollback could not restore a consumed bump file left that file deleted; re-running then consumed what remained and printed `demo (cargo) 0.1.0 -> 0.1.1` at exit `0`, where `0.2.0` was intended. Under exit `1` that script stops. ADR-0035 already names the principle — *"Retrying cannot help a failure oakum measured"* — and this is an instance of it.

A new variant was rejected for the reason ADR-0034 rejected per-variant codes: it invents vocabulary a caller must learn, to describe something the message already says precisely.

**The remaining "we didn't look" is `unswept`** — a directory rollback could not read, which supports no claim about the tree either way. It keeps exit `1` with the rest, and this is the weakest part of the decision.

The argument is that a directory a write set sweeps is one it wrote to or deleted from, so the plan phase already opened a file there and a permission that blocks the sweep blocks planning first. Measured on macOS, for one fixture: a two-member Cargo workspace whose `.changeset` note bumps both members, with `chmod 0333` on `crates/beta`, refuses with `error: failed to open crates/beta/Cargo.toml: Permission denied (os error 13)` at exit `1`, both manifests still at `0.1.0` and both bump files intact. Change that fixture so the run does not bump `beta` and it exits `0` — the directory is never written to, so it is never swept either.

That is one route, not a proof. It is **inferred**, not established, that no route reaches the arm: `an_unreadable_directory_is_named_not_counted_clean` drives it at the `commit_write_set` seam, a non-root Linux run may order the permission checks differently, and a concurrent `rmdir` or an I/O error mid-run was never exercised. So `unswept` rides a number that does not describe it, on an argument that could be wrong. **A route that reaches it is the trigger to revisit**, because at that point the tree really would be unvouched and AGENTS.md's rule would apply.

### Consequences

- Good, because a script that stops on `1` stops on the case where stopping matters most: the bump files this run consumed may be gone, and the next run versions without them.
- Good, because it agrees with `Outcome`'s `Ord` (`Error` before `Unverified`) rather than contradicting it, so the ranking rule and the classification rule say the same thing.
- Bad, because a caller reading only the exit code cannot distinguish a clean rollback from a half-written tree. That distinction is in the message, which is where ADR-0034 puts what was written: *"What was written is in the message, where it can be said precisely; no exit code can carry it."* The message now names every file the run deleted and could not restore, and prints its opening lines.
- Bad, because `unswept` rides a number that does not describe it, accepted on an inferred unreachability rather than a measured one. The moment a route is found, this is wrong and gets revisited.
- Neutral, because nothing changes for existing callers: every one of these cases exited `1` before this decision and exits `1` after.

### Confirmation

`a_write_set_that_rolled_back_is_a_finding_not_an_unverified_look` in `crates/oakum/tests/version_cli.rs` asserts `Some(1)` and the `error:` token at the process boundary for a rollback driven by a read-only `.changeset`. It is `#[cfg(unix)]` and needs a non-root process, which is what makes it the pin that matters: the two `chflags`-driven tests beside it — `a_bump_file_this_run_destroyed_is_named_with_where_it_comes_back_from` and `an_uncommitted_bump_file_this_run_destroyed_is_quoted_in_full` — are `#[cfg(target_os = "macos")]`, and no workflow uses a macOS runner, so they pin this decision on a developer's machine and nowhere else. Measured: applying `okm-404.60`'s own proposal (`version.rs` mapping every write-set failure to `CliError::unverified`) turns all three red locally, and the `#[cfg(unix)]` one red on its own where CI runs.

`commit_write_set` returns `Result<(), WriteSetFailure>` rather than a boxed error, which makes the signature honest and is the prerequisite for a caller reading the account. It is not yet that: the fields are private with no accessors, and `version::run` still returns `Box<dyn std::error::Error>`, so its `?` reboxes the value and `CliError::from_boxed` flattens it to prose. Measured: stderr is byte-identical before and after the signature change on every failure shape. So `okm-8q3`'s alternative — a caller that can ask what was left changed without parsing prose — is **not** satisfied, and this record is what answers it; `okm-2ppr.13` is where the accessors go, after the refused-versus-rolled-back conflation below them is made structural.

Revisit if `version` grows a machine-readable channel, or if a route to `unswept` is found.

## More Information

- [ADR-0034](0034-exit-two-for-unverified.md) — which number each variant answers, which this applies to a case it left open
- [ADR-0035](0035-a-finding-outranks-an-unverified-look.md) — a finding outranks a look that did not happen, whose retry analysis decides this
- `okm-404.60`, which asked for exit 2, and `okm-8q3`, whose acceptance criteria made this record the condition of answering otherwise
