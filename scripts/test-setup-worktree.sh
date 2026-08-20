#!/usr/bin/env bash
# Smoke tests for scripts/setup-worktree.sh and claude-ensure-worktree-setup.sh.
# Uses a disposable git worktree and a stubbed `mise` so CI stays fast and
# offline-friendly (no toolchain install / beads clone).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
SETUP="$ROOT/scripts/setup-worktree.sh"
ENSURE="$ROOT/scripts/claude-ensure-worktree-setup.sh"
STAMP=$$
WT_NAME="oakum-setup-smoke-${STAMP}"
WT_DIR="$ROOT/.claude/worktrees/${WT_NAME}"
BRANCH="worktree-${WT_NAME}"
STUB_BIN=$(mktemp -d)
TARGET_DIR=""
MARKER=".cargo/oakum-worktree-bootstrapped"

cleanup() {
  if git -C "$ROOT" worktree list --porcelain 2>/dev/null | grep -q "worktree $WT_DIR"; then
    git -C "$ROOT" worktree remove --force "$WT_DIR" >/dev/null 2>&1 || rm -rf "$WT_DIR"
  else
    rm -rf "$WT_DIR"
  fi
  git -C "$ROOT" branch -D "$BRANCH" >/dev/null 2>&1 || true
  rm -rf "$STUB_BIN"
  if [[ -n "$TARGET_DIR" && -d "$TARGET_DIR" ]]; then
    rm -rf "$TARGET_DIR"
  fi
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Stub mise: install/setup succeed without network or beads.
cat >"$STUB_BIN/mise" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$STUB_BIN/mise"
export PATH="$STUB_BIN:$PATH"

mkdir -p "$ROOT/.claude/worktrees"
git -C "$ROOT" worktree add -b "$BRANCH" "$WT_DIR" HEAD >/dev/null

# --- refuse main checkout ---
if (cd "$ROOT" && "$SETUP" --root "$ROOT") >/dev/null 2>&1; then
  fail "setup-worktree.sh should refuse the main checkout"
fi
if [[ -f "$ROOT/.cargo/config.toml" ]]; then
  fail "setup must not write .cargo/config.toml in the main checkout"
fi

# --- refuse main subdirectory (CwdChanged trap) ---
if (cd "$ROOT/crates" && "$SETUP" --root "$ROOT") >/dev/null 2>&1; then
  fail "setup-worktree.sh should refuse a subdirectory of the main checkout"
fi
if [[ -f "$ROOT/crates/.cargo/config.toml" ]]; then
  fail "setup must not write crates/.cargo/config.toml"
fi

# --- wrong --root ---
if (cd "$WT_DIR" && "$SETUP" --root /tmp) >/dev/null 2>&1; then
  fail "setup-worktree.sh should refuse a wrong --root"
fi

# --- happy path (from a subdirectory of the worktree) ---
out=$(cd "$WT_DIR/crates" && "$SETUP" --root "$ROOT")
TARGET_DIR="$ROOT/target/wt-${WT_NAME}"
[[ -d "$TARGET_DIR" ]] || fail "expected target dir $TARGET_DIR"
[[ -f "$WT_DIR/.cargo/config.toml" ]] || fail "expected $WT_DIR/.cargo/config.toml"
[[ -f "$WT_DIR/$MARKER" ]] || fail "expected success marker $WT_DIR/$MARKER"
[[ ! -f "$WT_DIR/crates/.cargo/config.toml" ]] || fail "must not write config under crates/"
grep -F "target-dir = \"$TARGET_DIR\"" "$WT_DIR/.cargo/config.toml" >/dev/null \
  || fail "config.toml missing correct target-dir"
printf '%s\n' "$out" | grep -F "$TARGET_DIR" >/dev/null \
  || fail "setup stdout should mention target-dir"

# --- ensure: no-op on main and main subdirectory ---
(cd "$ROOT" && "$ENSURE") || fail "ensure should exit 0 on main"
(cd "$ROOT/crates" && "$ENSURE") || fail "ensure should exit 0 under main/crates"
[[ ! -f "$ROOT/crates/.cargo/config.toml" ]] || fail "ensure must not write under main/crates"

# --- ensure: no-op when marker exists ---
(cd "$WT_DIR/crates" && "$ENSURE") || fail "ensure should exit 0 when marker exists"

# --- ensure: retries when config exists but marker is missing ---
rm -f "$WT_DIR/$MARKER"
(cd "$WT_DIR" && "$ENSURE") || fail "ensure should exit 0 when retrying without marker"
[[ -f "$WT_DIR/$MARKER" ]] || fail "ensure retry should recreate the success marker"

# --- ensure: bootstraps a bare worktree ---
rm -f "$WT_DIR/.cargo/config.toml" "$WT_DIR/$MARKER"
(cd "$WT_DIR" && "$ENSURE") || fail "ensure should exit 0 on bare worktree"
[[ -f "$WT_DIR/.cargo/config.toml" && -f "$WT_DIR/$MARKER" ]] \
  || fail "ensure should create config.toml and success marker"

# --- Cursor adapter path resolves ---
rel=$(python3 -c "import json; print(json.load(open('$ROOT/.cursor/worktrees.json'))['setup-worktree-unix'])")
resolved=$(cd "$ROOT/.cursor" && python3 -c "import os,sys; print(os.path.realpath(sys.argv[1]))" "$rel")
expected=$(cd "$ROOT/scripts" && pwd -P)/setup-worktree.sh
[[ "$resolved" == "$expected" ]] || fail "Cursor adapter should resolve to $expected (got $resolved)"
[[ -x "$resolved" ]] || fail "resolved Cursor setup script is not executable: $resolved"

# --- scripts stay executable ---
[[ -x "$SETUP" && -x "$ENSURE" ]] || fail "setup scripts must be executable"

echo "ok: setup-worktree smoke ($WT_NAME)"
