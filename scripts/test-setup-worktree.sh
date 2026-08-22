#!/usr/bin/env bash
# Smoke tests for scripts/setup-worktree.sh and claude-ensure-worktree-setup.sh.
# Uses a disposable git worktree and a stubbed `mise` so CI stays fast and
# offline-friendly (no toolchain install / beads clone).
set -euo pipefail

SOURCE_ROOT=$(cd "$(dirname "$0")/.." && pwd -P)

main_worktree() {
  local source=$1
  local list_file
  local record
  list_file=$(mktemp) || return 1
  if ! git -C "$source" worktree list --porcelain -z >"$list_file"; then
    rm -f "$list_file"
    return 1
  fi
  if ! IFS= read -r -d '' record <"$list_file"; then
    rm -f "$list_file"
    echo "cannot derive the main worktree from git worktree list" >&2
    return 1
  fi
  rm -f "$list_file"
  if [[ "$record" != worktree\ * ]]; then
    echo "cannot derive the main worktree from git worktree list" >&2
    return 1
  fi
  (cd "${record#worktree }" && pwd -P)
}

worktree_registration() {
  local path=$1
  local list_file
  local record
  local result=absent
  list_file=$(mktemp "$STUB_BIN/worktree-list.XXXXXX") || {
    printf '%s\n' unverified
    return
  }
  if ! git -C "$SOURCE_ROOT" worktree list --porcelain -z >"$list_file" 2>/dev/null; then
    rm -f "$list_file"
    printf '%s\n' unverified
    return
  fi
  while IFS= read -r -d '' record; do
    if [[ "$record" == "worktree $path" ]]; then
      result=registered
      break
    fi
  done <"$list_file"
  rm -f "$list_file"
  printf '%s\n' "$result"
}

remove_owned_path() {
  local label=$1
  local path=$2
  if [[ -e "$path" || -L "$path" ]]; then
    rm -rf -- "$path"
  fi
  if [[ -e "$path" || -L "$path" ]]; then
    echo "FAIL: could not clean $label $path" >&2
    return 1
  fi
}

MAIN_ROOT=$(main_worktree "$SOURCE_ROOT")
SETUP="$SOURCE_ROOT/scripts/setup-worktree.sh"
ENSURE="$SOURCE_ROOT/scripts/claude-ensure-worktree-setup.sh"
MARKER=".cargo/oakum-worktree-bootstrapped"
STUB_BIN=
OWNED_WORKTREE_DIR=
OWNED_BRANCH=
OWNED_TARGET_DIR=

cleanup() {
  local status=$?
  local cleanup_failed=0
  local registration
  local verify_status
  trap - EXIT
  set +e

  if [[ -n "$OWNED_WORKTREE_DIR" ]]; then
    registration=$(worktree_registration "$OWNED_WORKTREE_DIR")
    case "$registration" in
      registered)
        git -C "$SOURCE_ROOT" worktree remove --force "$OWNED_WORKTREE_DIR" >/dev/null 2>&1
        ;;
      absent) ;;
      *)
        echo "FAIL: could not verify worktree registration for $OWNED_WORKTREE_DIR" >&2
        cleanup_failed=1
        ;;
    esac
    if ! remove_owned_path "worktree directory" "$OWNED_WORKTREE_DIR"; then
      cleanup_failed=1
    fi
    registration=$(worktree_registration "$OWNED_WORKTREE_DIR")
    case "$registration" in
      registered)
        echo "FAIL: could not clean worktree $OWNED_WORKTREE_DIR" >&2
        cleanup_failed=1
        ;;
      absent) ;;
      *)
        echo "FAIL: could not verify worktree cleanup for $OWNED_WORKTREE_DIR" >&2
        cleanup_failed=1
        ;;
    esac
  fi
  if [[ -n "$OWNED_BRANCH" ]]; then
    git -C "$SOURCE_ROOT" branch -D -- "$OWNED_BRANCH" >/dev/null 2>&1
    git -C "$SOURCE_ROOT" show-ref --verify --quiet "refs/heads/$OWNED_BRANCH"
    verify_status=$?
    case "$verify_status" in
      0)
        echo "FAIL: could not clean branch $OWNED_BRANCH" >&2
        cleanup_failed=1
        ;;
      1) ;;
      *)
        echo "FAIL: could not verify branch cleanup for $OWNED_BRANCH" >&2
        cleanup_failed=1
        ;;
    esac
  fi
  if [[ -n "$STUB_BIN" ]] && ! remove_owned_path "stub directory" "$STUB_BIN"; then
    cleanup_failed=1
  fi
  if [[ -n "$OWNED_TARGET_DIR" ]] && ! remove_owned_path "target directory" "$OWNED_TARGET_DIR"; then
    cleanup_failed=1
  fi
  if [[ "$status" -eq 0 && "$cleanup_failed" -ne 0 ]]; then
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

STUB_BIN=$(mktemp -d)
WT_NAME="oakum-setup-smoke-$(basename "$STUB_BIN")"
WT_DIR="$MAIN_ROOT/.claude/worktrees/${WT_NAME}-é"
BRANCH="worktree-${WT_NAME}"
TARGET_DIR="$MAIN_ROOT/target/wt-$(basename "$WT_DIR")"

# Stub mise: install/setup succeed without network or beads.
cat >"$STUB_BIN/mise" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$STUB_BIN/mise"
export PATH="$STUB_BIN:$PATH"

mkdir -p "$MAIN_ROOT/.claude/worktrees"
[[ ! -e "$WT_DIR" && ! -L "$WT_DIR" ]] || fail "refusing to reuse worktree path $WT_DIR"
OWNED_WORKTREE_DIR=$WT_DIR
git -C "$SOURCE_ROOT" branch "$BRANCH" HEAD
OWNED_BRANCH=$BRANCH
git -C "$SOURCE_ROOT" worktree add "$WT_DIR" "$BRANCH" >/dev/null
[[ "$(main_worktree "$WT_DIR")" == "$MAIN_ROOT" ]] \
  || fail "linked worktree did not resolve main root $MAIN_ROOT"

# --- refuse main checkout ---
if (cd "$MAIN_ROOT" && "$SETUP" --root "$MAIN_ROOT") >/dev/null 2>&1; then
  fail "setup-worktree.sh should refuse the main checkout"
fi
if [[ -f "$MAIN_ROOT/.cargo/config.toml" ]]; then
  fail "setup must not write .cargo/config.toml in the main checkout"
fi

# --- refuse main subdirectory (CwdChanged trap) ---
if (cd "$MAIN_ROOT/crates" && "$SETUP" --root "$MAIN_ROOT") >/dev/null 2>&1; then
  fail "setup-worktree.sh should refuse a subdirectory of the main checkout"
fi
if [[ -f "$MAIN_ROOT/crates/.cargo/config.toml" ]]; then
  fail "setup must not write crates/.cargo/config.toml"
fi

# --- wrong --root ---
if (cd "$WT_DIR" && "$SETUP" --root /tmp) >/dev/null 2>&1; then
  fail "setup-worktree.sh should refuse a wrong --root"
fi

# --- happy path (from a subdirectory of the worktree) ---
[[ ! -e "$TARGET_DIR" && ! -L "$TARGET_DIR" ]] || fail "refusing to reuse target dir $TARGET_DIR"
OWNED_TARGET_DIR=$TARGET_DIR
out=$(cd "$WT_DIR/crates" && "$SETUP" --root "$MAIN_ROOT")
[[ -d "$TARGET_DIR" ]] || fail "expected target dir $TARGET_DIR"
[[ -f "$WT_DIR/.cargo/config.toml" ]] || fail "expected $WT_DIR/.cargo/config.toml"
[[ -f "$WT_DIR/$MARKER" ]] || fail "expected success marker $WT_DIR/$MARKER"
[[ ! -f "$WT_DIR/crates/.cargo/config.toml" ]] || fail "must not write config under crates/"
grep -F "target-dir = \"$TARGET_DIR\"" "$WT_DIR/.cargo/config.toml" >/dev/null \
  || fail "config.toml missing correct target-dir"
printf '%s\n' "$out" | grep -F "$TARGET_DIR" >/dev/null \
  || fail "setup stdout should mention target-dir"

# --- ensure: no-op on main and main subdirectory ---
(cd "$MAIN_ROOT" && "$ENSURE") || fail "ensure should exit 0 on main"
(cd "$MAIN_ROOT/crates" && "$ENSURE") || fail "ensure should exit 0 under main/crates"
[[ ! -f "$MAIN_ROOT/crates/.cargo/config.toml" ]] || fail "ensure must not write under main/crates"

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
rel=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["setup-worktree-unix"])' \
  "$SOURCE_ROOT/.cursor/worktrees.json")
resolved=$(cd "$SOURCE_ROOT/.cursor" && python3 -c "import os,sys; print(os.path.realpath(sys.argv[1]))" "$rel")
expected="$(cd "$SOURCE_ROOT/scripts" && pwd -P)/setup-worktree.sh"
[[ "$resolved" == "$expected" ]] || fail "Cursor adapter should resolve to $expected (got $resolved)"
[[ -x "$resolved" ]] || fail "resolved Cursor setup script is not executable: $resolved"

# --- scripts stay executable ---
[[ -x "$SETUP" && -x "$ENSURE" ]] || fail "setup scripts must be executable"

echo "ok: setup-worktree smoke ($WT_NAME)"
