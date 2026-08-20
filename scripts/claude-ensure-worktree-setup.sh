#!/usr/bin/env bash
# Claude adapter: if this session is in a linked worktree and oakum bootstrap
# has not finished, run scripts/setup-worktree.sh.
#
# Wired from SessionStart / CwdChanged in .claude/settings.json.
# Does not replace Claude's git worktree create (see Claude worktrees docs:
# WorktreeCreate is for non-git VCS or fully custom create).
set -euo pipefail

MARKER=".cargo/oakum-worktree-bootstrapped"
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
SETUP="$REPO_ROOT/scripts/setup-worktree.sh"

phys() {
  (cd "$1" && pwd -P)
}

if ! git rev-parse --git-common-dir >/dev/null 2>&1; then
  exit 0
fi

GIT_COMMON=$(phys "$(git rev-parse --git-common-dir)")
MAIN=$(phys "$(dirname "$GIT_COMMON")")
TOPLEVEL=$(phys "$(git rev-parse --show-toplevel)")

# Main checkout (or any subdirectory of it) — nothing to do.
if [[ "$TOPLEVEL" == "$MAIN" ]]; then
  exit 0
fi

cd "$TOPLEVEL"

# Already bootstrapped (marker written only after mise install succeeds).
if [[ -f "$MARKER" ]]; then
  exit 0
fi

# Soft-fail: SessionStart/CwdChanged must not abort the session.
"$SETUP" --root "$MAIN" || true
