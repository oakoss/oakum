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

# Failed conversion is unverified, not a mismatch.
worktree_paths_compare() {
  local a=$1
  local b=$2
  local wa wb
  [[ "$a" == "$b" ]] && { printf '%s\n' equal; return; }
  if ! command -v cygpath >/dev/null 2>&1; then
    case "${OSTYPE:-}" in
      msys* | cygwin* | mingw*) printf '%s\n' unverified ;;
      *) printf '%s\n' unequal ;;
    esac
    return
  fi
  wa=$(cygpath -w "$a") || { printf '%s\n' unverified; return; }
  wb=$(cygpath -w "$b") || { printf '%s\n' unverified; return; }
  wa=$(printf '%s' "$wa" | tr '[:upper:]' '[:lower:]')
  wb=$(printf '%s' "$wb" | tr '[:upper:]' '[:lower:]')
  if [[ "$wa" == "$wb" ]]; then
    printf '%s\n' equal
  else
    printf '%s\n' unequal
  fi
}

worktree_paths_equal() {
  [[ "$(worktree_paths_compare "$1" "$2")" == equal ]]
}

# Listed spelling, unverified, or empty.
match_worktree_list() {
  local path=$1
  local list_file=$2
  local record
  local listed
  local compare
  local found=
  local saw_unverified=
  while IFS= read -r -d '' record; do
    if [[ "$record" == worktree\ * ]]; then
      listed=${record#worktree }
      compare=$(worktree_paths_compare "$listed" "$path")
      case "$compare" in
        equal)
          found=$listed
          break
          ;;
        unverified) saw_unverified=1 ;;
      esac
    fi
  done <"$list_file"
  if [[ -n "$found" ]]; then
    printf '%s\n' "$found"
  elif [[ -n "$saw_unverified" ]]; then
    printf '%s\n' unverified
  fi
}

registered_worktree_path() {
  local path=$1
  local list_file
  list_file=$(mktemp "$STUB_BIN/worktree-list.XXXXXX") || {
    printf '%s\n' unverified
    return
  }
  if ! git -C "$SOURCE_ROOT" worktree list --porcelain -z >"$list_file" 2>/dev/null; then
    rm -f "$list_file"
    printf '%s\n' unverified
    return
  fi
  match_worktree_list "$path" "$list_file"
  rm -f "$list_file"
}

worktree_registration() {
  local path=$1
  local listed
  listed=$(registered_worktree_path "$path")
  case "$listed" in
    unverified) printf '%s\n' unverified ;;
    "") printf '%s\n' absent ;;
    *) printf '%s\n' registered ;;
  esac
}

unregister_owned_worktree() {
  local listed
  listed=$(registered_worktree_path "$OWNED_WORKTREE_DIR")
  case "$listed" in
    unverified)
      echo "FAIL: could not verify worktree registration for $OWNED_WORKTREE_DIR" >&2
      return 1
      ;;
    "") ;;
    *)
      git -C "$SOURCE_ROOT" worktree remove --force "$listed" >/dev/null 2>&1
      ;;
  esac
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
    if ! unregister_owned_worktree; then
      cleanup_failed=1
    fi
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

create_owned_worktree() {
  local path=$1
  local branch=$2
  mkdir -p "$MAIN_ROOT/.claude/worktrees"
  [[ ! -e "$path" && ! -L "$path" ]] || fail "refusing to reuse worktree path $path"
  OWNED_WORKTREE_DIR=$path
  git -C "$SOURCE_ROOT" branch "$branch" HEAD
  OWNED_BRANCH=$branch
  git -C "$SOURCE_ROOT" worktree add "$path" "$branch" >/dev/null
}

copy_smoke_inputs() {
  local destination=$1
  cp -p "$SOURCE_ROOT/scripts/test-setup-worktree.sh" "$destination/scripts/"
  cp -p "$SOURCE_ROOT/scripts/setup-worktree.sh" "$destination/scripts/"
  cp -p "$SOURCE_ROOT/scripts/claude-ensure-worktree-setup.sh" "$destination/scripts/"
  cp -p "$SOURCE_ROOT/.cursor/worktrees.json" "$destination/.cursor/"
}

STUB_BIN=$(mktemp -d)
WT_NAME="oakum-setup-smoke-$(basename "$STUB_BIN")"
BRANCH="worktree-${WT_NAME}"

if [[ "$SOURCE_ROOT" == "$MAIN_ROOT" || "${OAKUM_SMOKE_FORCE_LINKED:-0}" == 1 ]]; then
  WT_DIR="$MAIN_ROOT/.claude/worktrees/${WT_NAME}-outer-é"
  create_owned_worktree "$WT_DIR" "$BRANCH"
  copy_smoke_inputs "$WT_DIR"
  OAKUM_SMOKE_FORCE_LINKED=0 "$WT_DIR/scripts/test-setup-worktree.sh"
  exit
fi

WT_DIR="$MAIN_ROOT/.claude/worktrees/${WT_NAME}-é"
TARGET_DIR="$MAIN_ROOT/target/wt-$(basename "$WT_DIR")"

# Stub mise: install/setup succeed without network or beads.
cat >"$STUB_BIN/mise" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$STUB_BIN/mise"
export PATH="$STUB_BIN:$PATH"

# Darwin has no cygpath. Remove the stub after these asserts so cleanup uses a real one.
cat >"$STUB_BIN/cygpath" <<'EOF'
#!/usr/bin/env python3
import sys

if len(sys.argv) < 3 or sys.argv[1] != "-w":
    sys.exit(1)
p = sys.argv[2]
if len(p) >= 3 and p[0] == "/" and p[1].isalpha() and p[2] == "/":
    p = p[1].upper() + ":\\" + p[3:].replace("/", "\\")
else:
    p = p.replace("/", "\\")
print(p)
EOF
chmod +x "$STUB_BIN/cygpath"
PATH="$STUB_BIN:$PATH" worktree_paths_equal /d/proj/wt /d/proj/wt \
  || fail "exact MSYS paths should match"
PATH="$STUB_BIN:$PATH" worktree_paths_equal /d/proj/wt 'D:\proj\wt' \
  || fail "MSYS vs Win32 should match via cygpath"
PATH="$STUB_BIN:$PATH" worktree_paths_equal /d/proj/wt 'D:/proj/wt' \
  || fail "MSYS vs D:/ should match via cygpath"
PATH="$STUB_BIN:$PATH" worktree_paths_equal /d/proj/wt 'd:\proj\wt' \
  || fail "mixed-case Win32 should match"
PATH="$STUB_BIN:$PATH" worktree_paths_equal /d/proj/wt-é 'D:\proj\wt-é' \
  || fail "MSYS vs Win32 should preserve é"
if PATH="$STUB_BIN:$PATH" worktree_paths_equal /d/proj/wt /d/other/wt; then
  fail "different MSYS paths should not match"
fi
list_file=$(mktemp "$STUB_BIN/porcelain.XXXXXX") || fail "mktemp porcelain fixture"
printf 'worktree %s\0' 'D:\proj\wt' >"$list_file"
got=$(PATH="$STUB_BIN:$PATH" match_worktree_list /d/proj/wt "$list_file")
[[ "$got" == 'D:\proj\wt' ]] || fail "porcelain Win32 listing should return listed spelling (got $got)"
rm -f "$list_file"
cat >"$STUB_BIN/cygpath" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
chmod +x "$STUB_BIN/cygpath"
[[ "$(PATH="$STUB_BIN:$PATH" worktree_paths_compare /d/proj/wt 'D:\proj\wt')" == unverified ]] \
  || fail "cygpath failure is unverified, not a mismatch"
list_file=$(mktemp "$STUB_BIN/porcelain.XXXXXX") || fail "mktemp porcelain fixture"
printf 'worktree %s\0' 'D:\proj\wt' >"$list_file"
got=$(PATH="$STUB_BIN:$PATH" match_worktree_list /d/proj/wt "$list_file")
[[ "$got" == unverified ]] || fail "cygpath failure on porcelain is unverified (got ${got:-empty})"
rm -f "$list_file"
rm -f "$STUB_BIN/cygpath"
# Hide a real Git Bash cygpath; deleting the stub does not remove /usr/bin/cygpath.
# Force a non-Windows OSTYPE: Git Bash sets msys, which is unverified without cygpath.
[[ "$(OSTYPE=darwin PATH="$STUB_BIN" worktree_paths_compare /d/proj/wt 'D:\proj\wt')" == unequal ]] \
  || fail "without cygpath, spelling mismatch is unequal"
[[ "$(OSTYPE=msys PATH="$STUB_BIN" worktree_paths_compare /d/proj/wt 'D:\proj\wt')" == unverified ]] \
  || fail "Windows without cygpath is unverified"
got=$(
  registered_worktree_path() { printf '%s\n' unverified; }
  worktree_registration /d/proj/wt
)
[[ "$got" == unverified ]] || fail "worktree_registration must not map unverified to absent"
(
  calls=$STUB_BIN/reg-calls
  remove_arg=$STUB_BIN/remove-arg
  : >"$calls"
  rm -f "$remove_arg"
  registered_worktree_path() {
    printf x >>"$calls"
    printf '%s\n' 'D:\proj\wt'
  }
  git() {
    if [[ "${1:-}" == -C && "${3:-}" == worktree && "${4:-}" == remove ]]; then
      printf '%s\n' "${6:-}" >"$remove_arg"
      return 0
    fi
    command git "$@"
  }
  OWNED_WORKTREE_DIR=/d/proj/wt
  unregister_owned_worktree
  [[ "$(wc -c <"$calls" | tr -d '[:space:]')" == 1 ]] \
    || fail "cleanup should look up registration once"
  [[ "$(cat "$remove_arg")" == 'D:\proj\wt' ]] \
    || fail "remove operand should be listed spelling, not unverified"
)
(
  remove_arg=$STUB_BIN/remove-arg
  rm -f "$remove_arg"
  registered_worktree_path() { printf '%s\n' unverified; }
  git() {
    if [[ "${1:-}" == -C && "${3:-}" == worktree && "${4:-}" == remove ]]; then
      printf '%s\n' "${6:-}" >"$remove_arg"
      return 0
    fi
    command git "$@"
  }
  OWNED_WORKTREE_DIR=/d/proj/wt
  if unregister_owned_worktree 2>/dev/null; then
    fail "unverified registration should fail closed"
  fi
  [[ ! -e "$remove_arg" ]] || fail "unverified registration must not call git worktree remove"
)

create_owned_worktree "$WT_DIR" "$BRANCH"
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
# Python's realpath prints a Win32 path; bash `pwd -P` prints `/d/...`.
# Compare both via Python after cd so the argument is relative, not MSYS-absolute.
resolved=$(cd "$SOURCE_ROOT/.cursor" && python3 -c "import os,sys; print(os.path.realpath(sys.argv[1]))" "$rel")
expected=$(cd "$SOURCE_ROOT/scripts" && python3 -c "import os,sys; print(os.path.realpath(sys.argv[1]))" "setup-worktree.sh")
[[ "$resolved" == "$expected" ]] || fail "Cursor adapter should resolve to $expected (got $resolved)"
[[ -x "$resolved" ]] || fail "resolved Cursor setup script is not executable: $resolved"

# --- scripts stay executable ---
[[ -x "$SETUP" && -x "$ENSURE" ]] || fail "setup scripts must be executable"

echo "ok: setup-worktree smoke ($WT_NAME)"
