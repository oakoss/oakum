//! The fixture harness's own contract.
//!
//! Everything the sweep of the other suites depends on is asserted here, so a
//! change to the guard fails in one small file rather than as a puzzle spread
//! across twenty.
#![allow(clippy::disallowed_methods)]

mod support;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cap_std::fs::Dir;

use support::fixture::{
    git, git_output, git_repo, git_stdout, oakum, plain_repo, sandbox_config, LEDGER, MARKER,
};

/// The call sites bind a fixture and then use it as a path in every shape the
/// suite contains. `Deref` alone does not satisfy an `AsRef` bound, so a type
/// carrying only `Deref` compiles here and fails at `Command::current_dir`,
/// `fs::create_dir_all`, `fs::metadata`, `Dir::open_ambient_dir` and
/// `Command::arg`. This test is what keeps all three impls.
#[test]
fn a_fixture_is_usable_everywhere_a_path_is() {
    fn takes_path(_: &Path) {}

    let root = plain_repo("fixture", "usable");

    takes_path(&root);
    let _ = root.join("a/b");
    let _ = root.parent().expect("container");
    let _ = root.file_name().expect("name");
    let _ = root.to_str().expect("utf-8");
    let _ = format!("{}", root.display());
    let _: PathBuf = root.to_path_buf();
    assert!(root.is_dir());

    fs::create_dir_all(root.join("nested")).expect("create_dir_all takes AsRef<Path>");
    let _ = fs::metadata(&root).expect("metadata takes AsRef<Path>");
    let _ = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).expect("cap-std");

    let mut command = Command::new("true");
    command.current_dir(&root).arg(&root);
    let _: &OsStr = root.as_ref();
}

/// The whole point: the tree is gone when the binding goes out of scope.
#[test]
fn a_dropped_fixture_removes_its_container() {
    let container = {
        let root = plain_repo("fixture", "dropped");
        let container = root.container().to_path_buf();
        fs::write(root.join("file.txt"), "x").expect("write");
        assert!(container.is_dir());
        container
    };
    assert!(
        !container.exists(),
        "{} survived its fixture",
        container.display()
    );
}

/// The property the old helpers never had: a fixture whose test fails is still
/// removed. `Drop` runs while the panic unwinds — asserted rather than assumed,
/// because it is the entire reason a failing suite stopped filling the disk.
#[test]
fn a_panicking_test_still_removes_its_fixture() {
    let seen = std::sync::Mutex::new(PathBuf::new());
    let caught = std::panic::catch_unwind(|| {
        let root = plain_repo("fixture", "panicking");
        *seen.lock().expect("lock") = root.container().to_path_buf();
        panic!("the fixture must outlive neither the test nor this panic");
    });

    assert!(caught.is_err(), "the probe was supposed to panic");
    let container = seen.lock().expect("lock").clone();
    assert!(
        !container.as_os_str().is_empty(),
        "the probe never built one"
    );
    assert!(
        !container.exists(),
        "{} survived a panicking test",
        container.display()
    );
}

/// `claim_label`'s job, done by construction. Two fixtures asking for the same
/// label must not share a path — the collision it guarded against deleted a
/// sibling test's repository mid-run.
#[test]
fn the_same_label_twice_gets_two_containers() {
    let one = plain_repo("fixture", "same");
    let two = plain_repo("fixture", "same");
    assert_ne!(one.container(), two.container());
    assert!(one.is_dir() && two.is_dir());
}

/// The container layout is what lets the ~95 sites that write beside their
/// root keep working unedited: a sibling lands inside the guarded tree, and a
/// bare origin cloned there resolves the same sandboxed config.
#[test]
fn a_sibling_written_beside_the_root_lives_inside_the_container() {
    let root = git_repo("fixture", "sibling");
    let container = root.container().to_path_buf();

    let bare = root.parent().expect("container").join("origin.git");
    git(&root, &["init", "--bare", bare.to_str().expect("utf-8")]);

    assert!(bare.starts_with(&container), "{}", bare.display());
    assert_eq!(sandbox_config(&bare), sandbox_config(&root));
    drop(root);
    assert!(!bare.exists(), "the sibling outlived the fixture");
}

/// Resolution walks ancestors rather than taking `parent()`, so it answers for
/// a path at any depth inside the container.
#[test]
fn the_sandbox_config_resolves_from_root_sibling_and_grandchild() {
    let root = plain_repo("fixture", "resolve");
    let expected = root.container().join("gitconfig");

    assert_eq!(sandbox_config(&root), expected);
    assert_eq!(
        sandbox_config(&root.parent().expect("c").join("shim")),
        expected
    );
    assert_eq!(sandbox_config(&root.join("a/b/c")), expected);
    assert!(root.container().join(MARKER).is_file());
}

/// A path outside any container names itself in the panic rather than sending
/// a git child at a config file that does not exist.
#[test]
#[should_panic(expected = "not inside a fixture container")]
fn a_path_outside_a_container_refuses_to_resolve() {
    let _ = sandbox_config(Path::new("/"));
}

/// The isolation has to actually reach git, not merely be set: a fixture
/// commits without the developer's signing config, and reports the identity
/// the seed pins rather than theirs.
#[test]
fn a_fixture_commit_uses_the_sandboxed_identity_and_no_signing() {
    let root = git_repo("fixture", "identity");
    fs::write(root.join("a.txt"), "x").expect("write");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "fixture"]);

    assert_eq!(
        git_stdout(&root, &["log", "-1", "--format=%an <%ae>"]),
        "oakum test <oakum@test.invalid>"
    );

    // `init.defaultBranch` from the seed, not from the developer's config.
    assert_eq!(
        git_stdout(&root, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );

    // The commit is unsigned: a leaked `commit.gpgsign=true` would have made
    // the commit above fail or hang, but assert the result too.
    assert_eq!(git_stdout(&root, &["log", "-1", "--format=%G?"]), "N");
}

/// The oakum binary reads the same sandboxed config its fixture's git children
/// do. A split between the two is the trap a process-wide seed would set.
///
/// The assertion has to turn on a value only the sandbox supplies. An earlier
/// version asserted `oakum --version` exits zero, which passes with the config
/// file deleted and with `git_env` removed from `oakum()` entirely — it read
/// no git config at all, so it measured nothing.
#[test]
fn the_binary_and_git_read_the_same_sandboxed_config() {
    let root = git_repo("fixture", "shared-config");
    git(&root, &["config", "--global", "user.name", "sentinel name"]);
    assert_eq!(
        git_stdout(&root, &["config", "--global", "user.name"]),
        "sentinel name",
        "a test's own edit to its global config must be visible to git"
    );

    // The binary has to *read* the sandboxed tier for this to prove anything,
    // so pin a key only the sandbox carries and whose effect oakum reports.
    // `remote.<name>.tagOpt = --no-tags` makes a clone fetch no tags, which
    // `tags` refuses as unverified rather than reporting an empty tag list.
    git(
        &root,
        &["remote", "add", "origin", "https://example.invalid/x.git"],
    );
    git(
        &root,
        &["config", "--global", "remote.origin.tagOpt", "--no-tags"],
    );

    let out = oakum(&root).arg("reachable-tags").output().expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && stderr.contains("tagOpt --no-tags"),
        "the binary must read the sandboxed global tier, not the developer's: {stderr}"
    );
}

/// The seed's two classes, asserted by what each is for.
///
/// An earlier version claimed to prove the seed "neutralizes ambient values"
/// and could not: `GIT_CONFIG_GLOBAL` replaces the whole global tier, so no
/// ambient config value reaches a fixture for the seed to neutralize, and
/// deleting `push.followTags` or `excludesFile` from the seed left it green.
/// What is observable is the seed's *supplied* values, and its one real
/// defense — the XDG ignore file, which git reads outside the config tier.
#[test]
fn the_seed_supplies_an_identity_and_defends_against_the_xdg_ignore_file() {
    let root = git_repo("fixture", "seed-effect");

    // Supplied: git has no default for either, and `init.defaultBranch`
    // otherwise answers `master` with a warning.
    fs::write(root.join("a.txt"), "x").expect("write");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "one"]);
    assert_eq!(
        git_stdout(&root, &["log", "-1", "--format=%an <%ae>"]),
        "oakum test <oakum@test.invalid>"
    );
    assert_eq!(
        git_stdout(&root, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );

    // Defensive: an ignore file reached through `XDG_CONFIG_HOME` is read
    // independently of the config tier, so only the empty `excludesFile` pin
    // stops it hiding a fixture's files.
    let xdg = root.parent().expect("container").join("xdg");
    fs::create_dir_all(xdg.join("git")).expect("xdg");
    fs::write(xdg.join("git/ignore"), "*.log\n").expect("ignore");
    fs::write(root.join("noisy.log"), "x").expect("write");

    let mut command = std::process::Command::new("git");
    let listed = support::fixture::git_env(&mut command, &root)
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&root)
        .output()
        .expect("git");
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("noisy.log"),
        "an XDG ignore file hid a fixture file: {}",
        String::from_utf8_lossy(&listed.stdout)
    );

    // The attributes file is the same shape: read outside the config tier, so
    // only the empty pin stops it applying to a fixture's files.
    fs::write(xdg.join("git/attributes"), "*.bin -diff\n").expect("attributes");
    fs::write(root.join("a.bin"), "x").expect("write");
    let mut command = std::process::Command::new("git");
    let attr = support::fixture::git_env(&mut command, &root)
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["check-attr", "diff", "--", "a.bin"])
        .current_dir(&root)
        .output()
        .expect("git");
    assert!(
        String::from_utf8_lossy(&attr.stdout).contains("unspecified"),
        "an XDG attributes file reached a fixture: {}",
        String::from_utf8_lossy(&attr.stdout)
    );
}

/// All three routes, because each is defeatable alone: libtest swallows a
/// passing test's stderr, `remove_dir_all` deletes the marker on its way to
/// failing, and the ledger's own write can fail.
#[cfg(unix)]
#[test]
fn a_container_that_cannot_be_removed_reaches_the_gate() {
    use std::os::unix::fs::PermissionsExt as _;

    let ledger = std::sync::Mutex::new(std::path::PathBuf::new());
    // The guard panics on a failed reclaim when the test is not already
    // failing, which is the property being exercised — so this one has to fail
    // deliberately and catch it.
    let seen = std::sync::Mutex::new(std::path::PathBuf::new());
    let blocked = std::sync::Mutex::new(std::path::PathBuf::new());
    let leaked = std::panic::catch_unwind(|| {
        let root = plain_repo("fixture", "undeletable");
        *ledger.lock().expect("lock") = root.container().parent().expect("base").join(LEDGER);
        *seen.lock().expect("lock") = root.container().to_path_buf();
        // A directory without write permission cannot have its children
        // unlinked, which is what makes the reclaim fail.
        let stuck = root.join("stuck");
        fs::create_dir_all(stuck.join("child")).expect("stuck");
        fs::set_permissions(&stuck, fs::Permissions::from_mode(0o555)).expect("chmod");
        *blocked.lock().expect("lock") = stuck;
    });
    let ledger = ledger.lock().expect("lock").clone();
    let container = seen.lock().expect("lock").clone();
    let recorded = fs::read_to_string(&ledger).unwrap_or_default();
    let marked = container.join(MARKER).is_file();

    // Before the assertions, so a regression strands no unwritable tree.
    // Removing the container is the whole repair: the gate ignores a ledger
    // entry whose container is gone, and rewriting that shared file would race
    // every other binary appending to it.
    let _ = fs::set_permissions(
        &*blocked.lock().expect("lock"),
        fs::Permissions::from_mode(0o755),
    );
    fs::remove_dir_all(&container)
        .unwrap_or_else(|err| panic!("could not repair {}: {err}", container.display()));

    assert!(leaked.is_err(), "a failed reclaim must fail its test");
    assert!(
        marked,
        "{} kept no marker, so a lost ledger would hide it",
        container.display()
    );
    assert!(
        recorded.contains(&container.display().to_string()),
        "the failed reclaim was not recorded: {recorded:?}"
    );
}

/// A container something else removed first is the outcome the guard wants.
/// Reporting it would redden a passing test and send its reader to a path that
/// no longer exists.
#[test]
fn a_container_already_gone_is_not_reported() {
    let (ledger, container) = {
        let root = plain_repo("fixture", "vanished");
        let container = root.container().to_path_buf();
        fs::remove_dir_all(&container).expect("remove ahead of Drop");
        (container.parent().expect("base").join(LEDGER), container)
    };
    let recorded = fs::read_to_string(&ledger).unwrap_or_default();
    // The full path, not the label: a stale line from another run would
    // otherwise fail this test for a leak it did not cause.
    assert!(
        !recorded.contains(&container.display().to_string()),
        "an already-gone container was reported as a leak: {recorded:?}"
    );
}

/// A marker without its `gitconfig` must refuse: git would otherwise run
/// unisolated, committing as the developer at exit 0.
#[test]
#[should_panic(expected = "has no gitconfig")]
fn a_container_missing_its_gitconfig_refuses_to_resolve() {
    let root = plain_repo("fixture", "no-config");
    fs::remove_file(root.container().join("gitconfig")).expect("remove");
    let _ = sandbox_config(&root);
}

/// The two guards carry different markers on purpose: only this side writes a
/// `gitconfig`, so a unit container answering an integration lookup would hand
/// back a path to a file nothing wrote.
///
/// `catch_unwind` rather than `#[should_panic]`: the probe container is a
/// sibling of the fixture, so an unwind past the cleanup would leave a marked
/// directory behind and fail the leak gate on every run.
#[test]
fn a_unit_marked_container_does_not_answer_an_integration_lookup() {
    let root = plain_repo("fixture", "unit-marker");
    let elsewhere = root
        .container()
        .parent()
        .expect("base")
        .join(format!("oakum-unitish-{}-probe", std::process::id()));
    fs::create_dir_all(&elsewhere).expect("mkdir");
    fs::write(elsewhere.join(".oakum-unit-fixture"), "").expect("marker");

    let caught = std::panic::catch_unwind(|| sandbox_config(&elsewhere));
    fs::remove_dir_all(&elsewhere).expect("cleanup");

    let err = caught.expect_err("a unit marker must not answer an integration lookup");
    let message = err.downcast_ref::<String>().map_or("", String::as_str);
    assert!(
        message.contains("not inside a fixture container"),
        "{message}"
    );
}

/// The ceiling is what makes [`plain_repo`] honest. Containers live under
/// `target/`, inside this checkout, so without it a discovery walk out of the
/// fixture finds the oakum repository and answers about that instead.
#[test]
fn a_fixture_without_a_repository_is_not_inside_this_checkout() {
    let root = plain_repo("fixture", "ceiling");
    let out = git_output(&root, &["rev-parse", "--show-toplevel"]);
    assert!(
        !out.status.success(),
        "the walk escaped the fixture and found {}",
        String::from_utf8_lossy(&out.stdout).trim()
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not a git repository"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The namespace clear reaches the *parent's* environment, so a caller's own
/// `GIT_*` must come after it. Setting one first is honoured on a machine
/// that does not export that name and dropped on one that does, so `git_env`
/// refuses the ordering rather than leaving it to the machine.
///
/// The clear itself is not directly testable in process: planting a variable
/// in the parent environment would race every other test in the binary.
#[test]
#[should_panic(expected = "must run before any GIT_* override")]
fn setting_a_git_variable_before_the_isolation_is_refused() {
    let root = plain_repo("fixture", "ordering");
    let mut command = std::process::Command::new("git");
    command.env("GIT_DIR", "/nonexistent/parent.git");
    let _ = support::fixture::git_env(&mut command, &root);
}

/// The guards and the shell gate share three literals across two languages,
/// with nothing but this holding them together: renaming one silently blinds
/// the gate to a whole class of leak.
#[test]
fn the_leak_check_looks_for_the_names_the_guards_write() {
    const SCRIPT: &str = include_str!("../../../scripts/fixture-leak-check.sh");
    for name in [MARKER, ".oakum-unit-fixture", LEDGER] {
        assert!(
            SCRIPT.contains(name),
            "scripts/fixture-leak-check.sh does not mention {name}, so it cannot see those containers"
        );
    }
}
