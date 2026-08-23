# Git TRACE2 versus a PATH shim for counting reachable-tags subprocesses

- Date: 2026-08-22
- Author: research session (oakum)
- Scope: whether `GIT_TRACE2_EVENT` is a better harness than a PATH `git` shim for `discovery_subprocess_count_is_constant_in_the_tag_count` (`okm-410`). After these measurements the count test was switched to TRACE2. Not an ADR.

## Question

`oakum reachable-tags` is specified to spawn a **fixed** number of Git child processes, independent of tag count. HEAD's integration test wrote a `#!/bin/sh` PATH shim, made it executable, and ran oakum through it. Linux CI printed `error: unverified: failed to run git rev-parse: Text file busy (os error 26)` in `discovery_subprocess_count_is_constant_in_the_tag_count` on the Tests job's Test step after [#69](https://github.com/oakoss/oakum/actions/runs/32611853614/job/97126072946) and earlier after [#65](https://github.com/oakoss/oakum/actions/runs/32607226150/job/97114046805).

Is `GIT_TRACE2_EVENT` a better way to count those children? This note looks for reasons TRACE2 is **worse or equal**, not only reasons it is better.

## Sources

Accessed **2026-08-22** (America/Chicago; commands ran after 21:30, which is 2026-08-23 UTC) unless noted.

### Official Git

- [Trace2 API](https://git-scm.com/docs/api-trace2) (`api-trace2`, last updated in 2.54.0; page history starts at **2.22.0**, 2019-06-07)
- [git(1) environment](https://git-scm.com/docs/git): `GIT_TRACE`, `GIT_TRACE2`, `GIT_TRACE2_EVENT`, `GIT_TRACE2_PERF`
- git.git `Documentation/RelNotes/2.22.0.txt` (tag `v2.22.0`): "A more structured way to obtain execution trace has been added." git.git `Documentation/RelNotes/2.21.0.txt` has **no** `trace2` mention
- git.git `trace2/tr2_dst.c` at `v2.55.0` (`tr2_dst_get_trace_fd`, `tr2_dst_try_path`, `tr2_dst_write_line`)
- git.git `trace2/tr2_sysenv.c` at `v2.55.0` (`GIT_TRACE2_EVENT` → `trace2.eventTarget`; `GIT_TRACE2_DST_DEBUG` → `trace2.destinationDebug`)
- Local `man git` on Homebrew Git 2.55.0 (same `GIT_TRACE` / `GIT_TRACE2*` text as git-scm.com)

### Runner images (first-party)

- [actions/runner-images README](https://github.com/actions/runner-images): `ubuntu-latest` → Ubuntu 24.04; `macos-latest` → macOS 26 arm64; `windows-latest` → Windows 2025
- [Ubuntu 24.04 software list](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md), image `20260816.277.1`: **Git 2.55.0**
- [macOS 26 arm64 software list](https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md), image `20260728.0273.1`: **Git 2.55.0**
- [Windows 2025 software list](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md): **Git 2.55.0.windows.4**

### Kernel / POSIX / Rust

- Linux `execve(2)` (man7.org): `ETXTBSY` — "The specified executable was open for writing by one or more processes"
- POSIX.1-2017 `execve`: `[ETXTBSY]` is documented; the Rationale says POSIX **neither requires nor prohibits** System V's "open for writing" behavior
- rustc 1.97.1 std: `std::fs::write` is `File::create(path)?.write_all(contents)` (`library/std/src/fs.rs`); `Command` children inherit the parent environment unless `env_clear` is called (`library/std/src/process.rs`)

### This repository (on disk 2026-08-22)

- `crates/oakum/src/cli/tags.rs`: three `Command::new("git")` calls (`rev-parse --is-shallow-repository`, `config --get-regexp ^remote\..*\.tagopt$`, `for-each-ref --merged=HEAD ... refs/tags`); comment that Git I/O stays in the binary (ADR-0002)
- `crates/oakum/tests/reachable_tags.rs` at **HEAD** (the PATH-shim test that CI flaked): `#[cfg(unix)]` `count_git_calls` — in-place `fs::write` + `chmod`, no `install_git_shim`. After okm-410: `git_processes` sets an absolute `GIT_TRACE2_EVENT` and asserts Trace2 `start` argv commands `rev-parse`, `config`, `for-each-ref`.
- `.github/workflows/ci.yml`: `tests` job `runs-on: ubuntu-latest` only
- [ADR-0002](../decisions/0002-single-crate-until-io.md): binary and `tests/` may perform I/O; the binary is where CLI-level I/O belongs

### This repository (CI)

- [Tests after #69](https://github.com/oakoss/oakum/actions/runs/32611853614/job/97126072946), Test step: `error: unverified: failed to run git rev-parse: Text file busy (os error 26)`
- [Tests after #65](https://github.com/oakoss/oakum/actions/runs/32607226150/job/97114046805), Test step: same panic text

### Commands run for this note

Quoted under Findings / Raw data. Throwaway fixtures lived under `/tmp/oakum-trace2-*`. No `git commit` in the oakum worktree.

## Findings

### What TRACE2 is, and what a `start` event means

Trace2 is inactive unless a target is enabled. The event target is JSONL, one object per line, enabled by `GIT_TRACE2_EVENT` or `trace2.eventTarget` ([api-trace2](https://git-scm.com/docs/api-trace2); git(1) `GIT_TRACE2_EVENT` defers to `GIT_TRACE2` for path rules).

Each Git process that enables the event target emits, among others:

- `"event":"version"` — first in a session; `exe` is the Git version, `evt` is the event-format version (`"4"` in 2.55 / Apple 2.50.1)
- `"event":"start"` — `argv` as received by `main()`
- `"event":"cmd_name"` — resolved command name and hierarchy

A Git process that **spawns** another process also emits `"event":"child_start"`. A child that is itself Git inherits Trace2 **context** (SID prefix, hierarchy such as `fetch/gc`). That inheritance is Git-to-Git. oakum is not Git; its children each get a fresh SID. They still see `GIT_TRACE2_EVENT` because Rust `Command` inherits the environment (`process.rs`), and `tags.rs` does not call `env_clear`.

Trace2 shipped in **2.22.0** (RelNotes: "A more structured way to obtain execution trace has been added"; api-trace2 page history starts there). 2.21.0 RelNotes do not mention it. Every host below is years newer.

`git help trace2` and `git help api-trace2` on this machine print `No manual entry for gittrace2` / `gitapi-trace2` (Homebrew does not install that page). `man git` does document `GIT_TRACE2_EVENT`.

### Host Git versions versus TRACE2 availability

| Host | Git | TRACE2? |
|---|---|---|
| This machine, `PATH` / Homebrew `/opt/homebrew/bin/git` | `git version 2.55.0` (observed) | yes (measured) |
| This machine, `/usr/bin/git` | `git version 2.50.1 (Apple Git-155)` (observed) | yes (wrote `/tmp/oakum-apple-trace2.event` with `event:start`) |
| GitHub `ubuntu-latest` (CI for this repo) | 2.55.0 (Ubuntu 24.04 image `20260816.277.1` software list; not `git --version` on a runner) | expected: 2.55 ≥ 2.22 |
| GitHub `macos-latest` (not in oakum CI) | 2.55.0 (macOS 26 arm64 image software list; not measured on a runner) | expected |
| GitHub `windows-latest` (not in oakum CI) | 2.55.0.windows.4 (Windows 2025 image software list; not measured on a runner) | expected |

Missing TRACE2 would look like "zero `start` events," not a compile error. That is a TRACE2-specific failure mode the shim does not have.

### The count: TRACE2 `start` versus PATH-shim exec, same binary

Product success path in `tags.rs` is three `Command::new("git")` calls. HEAD's shim test asserted **3** log lines and that one-tag and many-tag repos match. After okm-410 the test asserts three Trace2 `start` events and matching argv lists.

`GIT_TRACE2_EVENT=/tmp/oakum-trace2-self.event ./target/debug/oakum reachable-tags` from the oakum checkout (many tags) produced **3** `start` records and **0** `child_start` records. argv lists:

```text
["git", "rev-parse", "--is-shallow-repository"]
["git", "config", "--get-regexp", "^remote\\..*\\.tagopt$"]
["git", "for-each-ref", "--merged=HEAD", "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)", "refs/tags"]
```

Same three commands, same order as `tags.rs`.

On `/tmp` fixtures (1 lightweight tag vs 12 lightweight + 12 annotated = 24 tags):

| Fixture | oakum stdout lines | TRACE2 `start` | TRACE2 `child_start` | PATH-shim lines |
|---|---|---|---|---|
| `/tmp/oakum-trace2-one` | 1 | 3 | 0 | 3 |
| `/tmp/oakum-trace2-many` | 24 | 3 | 0 | 3 |

On **one** oakum invocation with **both** a PATH shim and `GIT_TRACE2_EVENT=/tmp/oakum-both.event`: shim lines = 3, `start` = 3, `child_start` = 0. The counts match. TRACE2 is not a more accurate counter for today's builtins; it is the **same** number.

That equality is an observation about `rev-parse`, `config`, and `for-each-ref` as **builtins that spawn no Git children**. It is not a guarantee about a future fourth `Command::new("git")` that ran a dashed external or a hook. A PATH shim counts `execve` of whatever PATH named `git`. TRACE2 `start` counts Git `main()`. If those ever diverge, the tests would disagree. They do not diverge on this path, measured.

A directory target (`GIT_TRACE2_EVENT=/tmp/oakum-trace2-dir` with that path already a directory) wrote **three** SID-named files, one `start` each: the documented "one file per process" behavior. Counting files in a directory is another way to get 3; counting `start` lines in a single file is enough if the path is a file.

### TRACE2 does not change the stdout oakum parses

`git rev-parse --is-shallow-repository` with and without `GIT_TRACE2_EVENT` both wrote `b'false\n'`. Stderr was empty in both cases (0 bytes). `diff` of the two stdout files was empty.

The same `for-each-ref` format string oakum uses, with and without the env var, produced identical stdout. `git config --get-regexp '^remote\..*\.tagopt$'` (exit 1, no matches) was likewise identical.

`GIT_TRACE=1` and `GIT_TRACE2=1` also left rev-parse stdout as `b'false\n'`. Both wrote human-readable traces to **stderr**. git(1): `GIT_TRACE` set to `1`/`true` prints to stderr. That would not break `parse_is_shallow` (it only reads stdout) **unless** someone pointed `GIT_TRACE` at a relative path and expected a file; git(1) only treats an **absolute** path as a file. `GIT_TRACE` is the wrong counter anyway: it is Trace1 printf text, not one record per process with stable `argv`.

oakum's own stdout on the one-tag fixture was identical with and without `GIT_TRACE2_EVENT`:

```text
plain b'e0bccb44e65fecbc4f89f95964187f850a04f3c8\tv0.1.0\n'
traced b'e0bccb44e65fecbc4f89f95964187f850a04f3c8\tv0.1.0\n'
```

### Event path must be absolute; relative is not resolved against git's cwd

git(1) `GIT_TRACE2`: if the value is an absolute path (starts with `/`), append to that file; if it is an existing directory, write one file per process. `tr2_dst.c` `tr2_dst_get_trace_fd` only opens a path when `is_absolute_path(tgt_value)`; anything else that is not `0`/`false`/`1`/`true`/a single digit/`af_unix:` is a **malformed** value.

Measured from `/tmp` with `GIT_TRACE2_EVENT=oakum-rel.event` and `git -C <oakum-checkout> rev-parse --is-shallow-repository`:

- no file at `/tmp/oakum-rel.event`
- no file at `<oakum-checkout>/oakum-rel.event`
- stderr: `warning: trace2: unknown value for 'GIT_TRACE2_EVENT': 'oakum-rel.event'`
- stdout still `b'false\n'`

oakum sets `Command.current_dir(repo)` on each git. That does **not** make a relative `GIT_TRACE2_EVENT` land in the fixture. The env var is rejected before cwd matters. A test that passed `trace.event` (relative) would count 0 and fail, while git and oakum still succeeded.

Pass an absolute path (for example `std::env::temp_dir()` joined to a unique name, or `CARGO_TARGET_TMPDIR`). Truncate or delete the file before the run; the target is opened `O_APPEND`.

### Concurrent append

`tr2_dst_try_path` opens `O_WRONLY | O_APPEND | O_CREAT`. `tr2_dst_write_line` comments that the kernel's `O_APPEND` seek+write is treated as atomic for a **complete** line, and that a short write would interleave with another thread or process. api-trace2 says thread events are "atomically appended to the shared target stream."

oakum runs the three git commands **sequentially** (`is_shallow`, then `tag_suppressed_remote`, then `reachable_tag_records`). Concurrent writers are not the success-path case. A leftover writer (developer `trace2.eventTarget` in **global** config) is possible: Trace2 reads system and global config, not repo-local. The environment overrides config (`tr2_sysenv_get` prefers `getenv`). A test that **sets** `GIT_TRACE2_EVENT` to its own absolute file owns that target. It does not stop a second target (`GIT_TRACE2` normal format) from also firing.

### `ETXTBSY` and the shim

Linux `execve(2)`: `ETXTBSY` if the executable is open for writing. POSIX documents the error and explicitly does **not** require it.

`std::fs::write` is create + `write_all`; the `File` is dropped when the statement ends, so the write fd is closed before a later `chmod`/`exec`. That is the "closed, then exec" sequence.

Re-run **outside** the oakum tree, `python:3.12-slim` on Docker Desktop, kernel `Linux 7.0.12-linuxkit`, 200 iterations each (`/tmp/oakum-etxtbsy.py`):

```text
closed-then-exec: ok=200 fail=0 n=200
hold-open-then-exec: ok=0 fail=200 n=200
hold-open errors: {"OSError errno=26 strerror='Text file busy'": 200}
```

That matches the review mutation as a **lead that survived this re-measure**: ETXTBSY appeared only while the write `File` was still open. It does **not** prove GitHub Actions' `ubuntu-latest` filesystem. GHA is an Azure VM (ext4 is typical; **not** measured here). Overlayfs caveats are inferred from Docker Desktop's container FS, not from first-party GHA or kernel overlay docs.

HEAD's shim is in-place `fs::write` + `chmod` (no sibling, no `rename`). A close-then-rename pattern was tried in an uncommitted pass and is not in this tree. If CI still flakes after TRACE2, the cause is unmeasured here.

TRACE2 never writes an executable, so it never hits this `execve` class.

### Failure modes TRACE2 adds

| Condition | Observed / sourced | Effect on oakum | Effect on a count test |
|---|---|---|---|
| Absolute event file, writable | 3 `start` lines | stdout unchanged | count works |
| Relative event path | warning; no file | stdout unchanged | count 0 — false fail |
| Unwritable absolute file | git exit 0, stdout `false\n`, stderr **empty**, file size 0 | success | count 0 — false fail |
| Same, plus `GIT_TRACE2_DST_DEBUG=1` | stderr: `warning: trace2: could not open '…' for 'GIT_TRACE2_EVENT' tracing: Permission denied` | still success | still count 0, but stderr explains it |
| `GIT_TRACE2_EVENT=1` / `true` | writes to git **stderr** (docs) | oakum ignores git stderr on success | events never hit a file the test reads |
| Git older than 2.22, or a build without Trace2 | env ignored (inferred from "inactive unless enabled") | product path fine | count 0 |
| `GIT_TRACE=1` | stderr pollution; stdout unchanged (measured) | product path fine | not a usable counter |

The silent unwritable case is worse than a PATH shim that fails the child: TRACE2 drops events and git still exits 0. HEAD's shim script has no `set -e`, so a failed `echo` into `calls.log` does not stop `exec`; oakum can succeed and the harness fails later while reading the log.

### Alternatives (brief)

**Compiled helper on PATH.** Cargo writes a test binary and closes it before `#[test]` runs (`CARGO_BIN_EXE_*`, Book ch. 11.3). The test would still intercept `Command::new("git")` via PATH, but it would not write-then-exec a script in the same process. No ETXTBSY class from test-time `fs::write`. Still a PATH lie; still `#[cfg(unix)]` unless the helper is a real `.exe` on Windows.

**`GitRunner` trait in product code.** Would make the three calls injectable. ADR-0002 puts Git I/O in the **binary**; `tags.rs` already says that. A trait is a product seam for a test problem. `tests/` may perform I/O without triggering the crate split, but they cannot reach a private trait unless the binary grows one. Heavier than TRACE2 or a compiled helper.

**Drop the count test; keep behavioral tests.** `nested_annotated_tag_peels_to_the_commit` and friends assert stdout. A regression that peeled each tag with an extra `git rev-parse` would still print the same `commit\tname` lines. The count test exists because those cases cannot see process count. Dropping it would not lock the contract in `reachable_tag_records`'s comment.

## Conclusions

**Tied as a counter; better as a way to stop writing an executable.**

On this machine, against this `target/debug/oakum`, TRACE2 `start` and a PATH shim both counted **3**, on one tag and on 24 tags, including a combined run. `child_start` was 0. stdout oakum parses did not change. Those disconfirming checks **survived**: TRACE2 is not a more accurate counter here.

TRACE2 **is** better at avoiding Linux `ETXTBSY`, because nothing is created mode `0755` and then exec'd. That is the only clear win over a PATH shim. The docker re-measure also survived as a **disconfirmation of "any write-then-exec is unsafe"**: closed-fd then exec was 200/200 ok; only a still-open write fd failed. HEAD's shim is that closed-fd in-place write, not a rename.

okm-410 adopted TRACE2 so the test does not maintain a `git` executable: an **absolute** `GIT_TRACE2_EVENT` on the oakum `Command`, count `"event":"start"`, assert the three argv command names, do not use `GIT_TRACE`. This note is not an ADR.

## Implications / actions

These are research implications, **not** locked decisions:

- Prefer TRACE2 over a new PATH shim when rewriting the count test. Keep the absolute-path and truncate-before-run rules. (Adopted for okm-410.)
- Do not treat a TRACE2 rewrite as a correctness upgrade of the count. Assert `start == 3` (and optionally the argv lists). Keep one-tag vs many-tag equality.
- Do not introduce `GIT_TRACE` as the counter; it is stderr Trace1 text.
- Do not add a `GitRunner` trait for this test alone; that is a product seam ADR-0002 did not ask for.
- Do not drop the count test if the "fixed number of Git children" comment in `tags.rs` is still a contract. Behavioral peel tests would not catch a per-tag `rev-parse` loop.
- If the shim is kept, keep close-then-rename (or close-then-exec). Do not exec a path whose write `File` is still in scope.
- CI stays `ubuntu-latest`. The Ubuntu 24.04 image software list has Git 2.55 (≥ 2.22); that is inventory, not a runner `git --version`. Contributor machines with Git < 2.22, or a relative event path, are the TRACE2-availability risk.

## Open questions

- Whether `ubuntu-latest` still flakes with HEAD's in-place `fs::write` + `chmod` shim (not re-run on GHA).
- Whether GHA's disk is overlayfs or something else that keeps a write fd across `close` (not found in first-party runner docs; inferred only from Docker Desktop).
- Windows: TRACE2 exists on Git for Windows 2.55. The count test is not `#[cfg(unix)]`; CI still has no Windows job.
- Whether a future fourth git invocation (hooks, aliases, a dashed external) would inflate `start` without inflating a PATH shim, or the reverse.
- Whether `trace2.maxFiles` / a directory target with a low cap could discard events (`too_many_files`). Defaults do not cap (`tr2env_max_files` starts at 0 = disabled). Not exercised.

## Raw data

### Local versions (observed)

```text
$ git --version
git version 2.55.0

$ /usr/bin/git --version
git version 2.50.1 (Apple Git-155)

$ /opt/homebrew/bin/git --version
git version 2.55.0

$ git help trace2
No manual entry for gittrace2
```

### Apple Git emits TRACE2 events (observed)

```text
$ GIT_TRACE2_EVENT=/tmp/oakum-apple-trace2.event /usr/bin/git rev-parse --is-shallow-repository
false
```

First two event lines included `"event":"version"` with `"exe":"2.50.1 (Apple Git-155)"` and `"event":"start"` with argv `rev-parse --is-shallow-repository`.

### Relative path (observed)

```text
warning: trace2: unknown value for 'GIT_TRACE2_EVENT': 'oakum-rel.event'
```

### Unwritable file without debug (observed)

git exit 0, stdout `b'false\n'`, stderr empty, event file size 0.

With `GIT_TRACE2_DST_DEBUG=1`: `warning: trace2: could not open '/tmp/oakum-nowrite3.event' for 'GIT_TRACE2_EVENT' tracing: Permission denied`.

### `tr2_dst.c` (v2.55.0) — absolute paths only

```c
if (is_absolute_path(tgt_value)) {
    if (is_directory(tgt_value))
        return tr2_dst_try_auto_path(dst, tgt_value);
    else
        return tr2_dst_try_path(dst, tgt_value);
}
/* Always warn about malformed values. */
tr2_dst_malformed_warning(dst, tgt_value);
```

### rustc 1.97.1 `fs::write`

```rust
File::create(path)?.write_all(contents)
```
