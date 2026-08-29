#!/usr/bin/env bash
# Remove stale fixture scratch under Cargo's target tmp.
#
# Keeps `oakum-changeset-foreign`, the deliberate `@changesets/parse` install
# cache. Everything else named `oakum-*` there is either harness litter a prior
# run left behind (including rust-cache restores) or a container this run should
# not still be holding.
#
# Resolves the target directory the same way `fixture-leak-check.sh` does —
# guessing `target/` would clean the wrong tree in a linked worktree.
set -euo pipefail

metadata=$(cargo metadata --no-deps --format-version 1) || {
  echo "fixture-scratch-clean: cargo metadata failed, so the target directory is unknown" >&2
  exit 1
}
target_dir=$(printf '%s' "$metadata" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
if [[ -z "$target_dir" ]]; then
  echo "fixture-scratch-clean: cargo metadata carried no target_directory" >&2
  exit 1
fi

tmp="$target_dir/tmp"
if [[ ! -d "$tmp" ]]; then
  echo "fixture-scratch-clean: $tmp does not exist yet; nothing to clean"
  exit 0
fi

removed=0
shopt -s nullglob
for dir in "$tmp"/oakum-*; do
  [[ -d "$dir" ]] || continue
  base=$(basename "$dir")
  if [[ "$base" == "oakum-changeset-foreign" ]]; then
    continue
  fi
  rm -rf "$dir"
  removed=$((removed + 1))
done
shopt -u nullglob

# Ledger rows name containers this pass deletes; leaving them confuses the next scan.
rm -f "$tmp/fixture-leaks.log"

echo "fixture-scratch-clean: removed ${removed} oakum-* dir(s) under $tmp"
