#!/usr/bin/env bash
# Reports fixture directories a test run left behind.
#
# Three signals, because they mean different things.
#
# The *ledger* is authoritative. `Drop` appends to it when a reclaim fails, and
# it survives what the on-disk evidence does not: libtest captures per-test
# stderr and prints it only for failures, so the guard's own message reaches
# nobody on a green run, and `remove_dir_all` deletes container-level files
# before it fails, so the marker itself may already be gone.
#
# A *marked* container catches the case the ledger cannot: a `SIGKILL` or an
# `abort`, where `Drop` never ran at all.
#
# An *unconverted* `oakum-*` directory belongs to a per-file helper this
# migration has not reached yet (okm-uaa). Expected to exist and to fall to
# zero, so it is reported as a count rather than failing the run — without it
# the marker check reads "clean" while a run leaks four figures of directories.
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

# Each `find` runs into a file so its exit status is checked rather than lost in
# a pipeline or a process substitution, where a failure would read as "found
# nothing" and pass.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# `find`'s status is tolerated but its stderr is not discarded: a shared
# `$TMPDIR` holds directories this user cannot descend (macOS keeps
# `TemporaryItems` protected), which is a partial scan rather than a failure.
# Reporting what could not be read keeps "we did not look" out of "it is fine".
find "${roots[@]}" -maxdepth 2 \
  \( -name .oakum-fixture -o -name .oakum-unit-fixture \) \
  -print >"$scratch/marked" 2>"$scratch/marked.err" || true
marked=$(wc -l <"$scratch/marked" | tr -d ' ')

# Excludes marked containers so the migration metric can reach zero,
# and the cached `@changesets/parse` install, which is deliberately permanent.
find "${roots[@]}" -maxdepth 1 -type d -name 'oakum-*' \
  ! -name 'oakum-changeset-foreign' -print >"$scratch/named" 2>>"$scratch/marked.err" || true
unconverted=0
while IFS= read -r dir; do
  [[ -e "$dir/.oakum-fixture" || -e "$dir/.oakum-unit-fixture" ]] && continue
  unconverted=$((unconverted + 1))
done <"$scratch/named"

ledgers=()
for root in "${roots[@]}"; do
  [[ -s "$root/fixture-leaks.log" ]] && ledgers+=("$root/fixture-leaks.log")
done

echo "fixture-leak-check: ${marked} marked, ${unconverted} unconverted (okm-uaa)"

if [[ -s "$scratch/marked.err" ]]; then
  echo "  note: some paths could not be scanned, so this count is a floor:"
  sed 's|^|    |' "$scratch/marked.err"
fi

status=0

if ((${#ledgers[@]} > 0)); then
  echo "error: the fixture guard could not remove these:" >&2
  cat "${ledgers[@]}" >&2
  rm -f "${ledgers[@]}"
  status=1
fi

if [[ -n "${OAKUM_TEST_RETAIN:-}" && "${OAKUM_TEST_RETAIN}" != "0" ]]; then
  echo "  OAKUM_TEST_RETAIN is set, so marked containers were kept on purpose."
  exit "$status"
fi

if ((marked > 0)); then
  echo "error: these fixture containers outlived their test:" >&2
  sed 's|/\.oakum-[a-z-]*fixture$||' "$scratch/marked" | sed 's|^|  |' >&2
  echo "A marked container survives only when Drop never ran — a kill or an" >&2
  echo "abort. A reclaim that failed is reported through the ledger above." >&2
  status=1
fi

exit "$status"
