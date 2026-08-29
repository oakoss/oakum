#!/usr/bin/env bash
# Reports fixture directories a test run left behind.
#
# Three signals, because they mean different things.
#
# A *marked* container needs no live process to survive: `Drop` restores the
# marker when a reclaim fails, and it is also what remains after a `SIGKILL` or
# an `abort`, where `Drop` never ran at all.
#
# The *ledger* carries the reason. `Drop` appends to it when a reclaim fails,
# and it survives libtest's capture of a passing test's stderr, which otherwise
# swallows the guard's own message. An entry whose container is gone is pruned
# rather than reported: something reclaimed it after the fact.
#
# An *unconverted* `oakum-*` directory belongs to a per-file helper this
# migration has not reached yet (okm-uaa). Expected to exist and to fall to
# zero, so it is reported as a count rather than failing the run — without it
# the marker check reads "clean" while a run leaks four figures of directories.
#
# A root this run could not read fails it: "we did not look" is not "it is
# fine". A single subdirectory that cannot be descended is only a floor on the
# counts, and is reported as one.
#
# `OAKUM_TEST_RETAIN` keeps marked containers deliberately, so only the hard
# checks are skipped when it is set.
set -euo pipefail

# Cargo's own answer, which honours `CARGO_TARGET_DIR`, `--target-dir`, and the
# `[build] target-dir` that `scripts/setup-worktree.sh` writes into a linked
# worktree's `.cargo/config.toml` without exporting anything. Guessing `target/`
# searches the wrong tree in a worktree and reports a clean run over real leaks.
# Guessing is not an option: falling back to `target/` searches the wrong tree
# in a linked worktree and would report a clean run over real leaks, which is
# the collapse AGENTS.md forbids.
metadata=$(cargo metadata --no-deps --format-version 1) || {
  echo "fixture-leak-check: cargo metadata failed, so the target directory is unknown" >&2
  exit 1
}
target_dir=$(printf '%s' "$metadata" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
if [[ -z "$target_dir" ]]; then
  echo "fixture-leak-check: cargo metadata carried no target_directory" >&2
  exit 1
fi

roots=()
for root in "$target_dir/tmp" "${TMPDIR:-/tmp}"; do
  [[ -d "$root" ]] && roots+=("$root")
done

if ((${#roots[@]} == 0)); then
  echo "fixture-leak-check: no fixture roots exist yet; nothing to check"
  exit 0
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

status=0
# A run that could not look is reported as a failure, never as a clean tree.
# Kept apart from a leak so the message can say which happened.
unverified=0

# An unreadable root is the case worth failing on: a subdirectory this user
# cannot descend is a floor on the counts (macOS keeps `TemporaryItems`
# protected, and `find` exits nonzero for it on every run), but a root that
# cannot be read means the scan saw nothing at all and would otherwise print
# zeroes.
for root in "${roots[@]}"; do
  if [[ ! -r "$root" || ! -x "$root" ]]; then
    echo "$root: unreadable, so nothing in it was scanned" >>"$scratch/scan.err"
    unverified=1
  fi
done

# `find` exiting nonzero is normal — one undescendable subdirectory does it.
# Death by signal is not, and it writes nothing to stderr, so the status is the
# only evidence that the scan stopped early.
scan() {
  local out=$1
  shift
  find "$@" -print >"$out" 2>>"$scratch/scan.err" && return 0
  local code=$?
  if ((code > 128)); then
    echo "find was killed (status $code), so the scan stopped early" >>"$scratch/scan.err"
    unverified=1
  fi
}

scan "$scratch/marked" "${roots[@]}" -maxdepth 2 \
  \( -name .oakum-fixture -o -name .oakum-unit-fixture \)
marked=$(wc -l <"$scratch/marked" | tr -d ' ')

# Excludes marked containers so the migration metric can reach zero,
# and the cached `@changesets/parse` install, which is deliberately permanent.
scan "$scratch/named" "${roots[@]}" -maxdepth 1 -type d -name 'oakum-*' \
  ! -name 'oakum-changeset-foreign'
unconverted=0
while IFS= read -r dir; do
  [[ -e "$dir/.oakum-fixture" || -e "$dir/.oakum-unit-fixture" ]] && continue
  unconverted=$((unconverted + 1))
done <"$scratch/named"

# Only entries whose container still exists are real: the rest name paths
# something already reclaimed. Each reason stays readable while its container
# lives, so a rerun reports the same diagnosis.
: >"$scratch/live"
for root in "${roots[@]}"; do
  ledger="$root/fixture-leaks.log"
  [[ -r "$root" && -x "$root" && -s "$ledger" ]] || continue
  if [[ ! -r "$ledger" ]]; then
    echo "$ledger: unreadable, so its entries were not checked" >>"$scratch/scan.err"
    unverified=1
    continue
  fi
  : >"$scratch/kept"
  while IFS=$'\t' read -r container reason; do
    [[ -n "$container" && -e "$container" ]] || continue
    printf '%s\t%s\n' "$container" "$reason" >>"$scratch/kept"
  done <"$ledger"
  cat "$scratch/kept" >>"$scratch/live"
  if [[ -s "$scratch/kept" ]]; then
    cp "$scratch/kept" "$ledger" || {
      echo "$ledger: could not be pruned" >>"$scratch/scan.err"
      unverified=1
    }
  elif ! rm -f "$ledger"; then
    echo "$ledger: could not be cleared" >>"$scratch/scan.err"
    unverified=1
  fi
done

echo "fixture-leak-check: ${marked} marked, ${unconverted} unconverted (okm-uaa)"

if [[ -s "$scratch/scan.err" ]]; then
  echo "  these paths went unread, so the counts above are a floor:" >&2
  sed 's|^|    |' "$scratch/scan.err" >&2
fi

if ((unverified)); then
  echo "error: this run could not look everywhere, so it reports nothing about" >&2
  echo "what it could not read. Fix the paths above and run it again." >&2
  status=1
fi

if [[ -s "$scratch/live" ]]; then
  echo "error: the fixture guard could not remove these:" >&2
  cat "$scratch/live" >&2
  status=1
fi

if [[ -n "${OAKUM_TEST_RETAIN:-}" && "${OAKUM_TEST_RETAIN}" != "0" ]]; then
  echo "  OAKUM_TEST_RETAIN is set, so marked containers were kept on purpose."
  exit "$status"
fi

if ((marked > 0)); then
  echo "error: these fixture containers outlived their test:" >&2
  sed 's|/\.oakum-[a-z-]*fixture$||' "$scratch/marked" | sed 's|^|  |' >&2
  if [[ -s "$scratch/live" ]]; then
    echo "Drop could not remove these and put the marker back; the ledger above" >&2
    echo "carries the reason for each." >&2
  else
    echo "No ledger accompanies these, so either Drop never ran — a kill or an" >&2
    echo "abort — or its ledger write failed." >&2
  fi
  status=1
fi

exit "$status"
