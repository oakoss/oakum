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
    git, git_output, git_repo, git_stdout, oakum, plain_repo, sandbox_config, sibling, LEDGER,
    MARKER,
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

    let bare = sibling(&root, "origin.git");
    git(&root, &["init", "--bare", bare.to_str().expect("utf-8")]);

    assert!(bare.starts_with(&container), "{}", bare.display());
    assert_eq!(sandbox_config(&bare), sandbox_config(&root));
    drop(root);
    assert!(!bare.exists(), "the sibling outlived the fixture");
}

#[test]
#[should_panic(expected = "one path segment")]
fn sibling_rejects_nested_names() {
    let root = plain_repo("fixture", "sib-nested");
    let _ = sibling(&root, "a/b");
}

#[test]
#[should_panic(expected = "fixture label")]
fn sibling_rejects_name_equal_to_label() {
    let root = plain_repo("fixture", "sib-collide");
    let _ = sibling(&root, "sib-collide");
}

#[test]
#[should_panic(expected = "fixture label")]
fn sibling_rejects_trailing_separator_equal_to_label() {
    let root = plain_repo("fixture", "sib-trail");
    let _ = sibling(&root, "sib-trail/");
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
    let (unit, _) = guard_sources();
    fs::write(elsewhere.join(const_value(&unit, "MARKER")), "").expect("marker");

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

/// Guards and the shell gate share three literals across two languages.
/// Asserts live marked-scan / ledger / unmarked-fail operands so a `-name` only
/// in a comment or a soft-count regression cannot keep this green.
#[test]
fn the_leak_check_looks_for_the_names_the_guards_write() {
    const SCRIPT: &str = include_str!("../../../scripts/fixture-leak-check.sh");
    let (unit, _) = guard_sources();
    let unit_marker = const_value(&unit, "MARKER");

    let marked = SCRIPT
        .split_once("scan \"$scratch/marked\"")
        .expect("marked scan")
        .1
        .split_once("scan \"$scratch/named\"")
        .expect("named scan follows marked")
        .0;
    for name in [MARKER, unit_marker.as_str()] {
        assert!(
            marked.lines().any(|line| {
                let code = line.split_once('#').map_or(line, |(c, _)| c);
                code.contains(&format!("-name {name}"))
            }),
            "the marked scan has no live `find -name {name}` operand"
        );
    }

    let ledger = SCRIPT
        .split_once(": >\"$scratch/live\"")
        .expect("live ledger accum")
        .1
        .split_once("echo \"fixture-leak-check:")
        .expect("summary follows ledger pass")
        .0;
    assert!(
        ledger.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains(&format!("ledger=\"$root/{LEDGER}\""))
        }),
        "the ledger pass has no live `ledger=\"$root/{LEDGER}\"` assignment"
    );

    // Fail-on-unmarked must stay live and before the retain early-exit, or
    // OAKUM_TEST_RETAIN greenwashes stale litter and soft-count regressions pass.
    let after_summary = SCRIPT
        .split_once("echo \"fixture-leak-check:")
        .expect("summary line")
        .1;
    let (before_retain, retain_and_after) = after_summary
        .split_once("OAKUM_TEST_RETAIN is set, so marked containers were kept")
        .expect("retain early-exit follows the unmarked fail");
    assert!(
        before_retain.contains("if ((unconverted > 0))"),
        "unmarked fail must run before the OAKUM_TEST_RETAIN early-exit"
    );
    assert!(
        before_retain.contains("status=1"),
        "unmarked fail must set status=1"
    );
    assert!(
        retain_and_after.contains("if ((marked > 0))"),
        "marked fail still follows the retain early-exit"
    );

    // Post-clean must stay after the retain early-exit and inside a live
    // status==0 gate, or retain wipes marked containers and a red leak-check
    // destroys its evidence. A gate only in a comment, or a call between retain
    // and the gate, still greenwashes both.
    assert!(
        !before_retain.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("fixture-scratch-clean.sh")
        }),
        "post-clean must not run before the OAKUM_TEST_RETAIN early-exit"
    );
    let gate_at = retain_and_after
        .lines()
        .position(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("if ((status == 0)); then")
        })
        .expect("post-clean must be gated on a live `status == 0`");
    let between_retain_and_gate: String = retain_and_after
        .lines()
        .take(gate_at)
        .flat_map(|line| [line, "\n"])
        .collect();
    assert!(
        !between_retain_and_gate.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("fixture-scratch-clean.sh")
        }),
        "post-clean must not run between retain and the status == 0 gate"
    );
    // Walk only the then-branch of the status gate. An `else`/`elif` at this
    // depth, a matching `fi`, or a nested `if` must not count as the green path:
    // clean after `fi`, under `else`, or under a nested never-true `if` would
    // still wipe red-run evidence or never run on green.
    let mut depth = 1usize;
    let mut saw_clean = false;
    let mut closed = false;
    for line in retain_and_after.lines().skip(gate_at + 1) {
        let code = line.split_once('#').map_or(line, |(c, _)| c);
        let trimmed = code.trim();
        if depth == 1 && (trimmed == "else" || trimmed.starts_with("elif ")) {
            // Then-branch ended; do not scan the red path.
            closed = true;
            break;
        }
        if depth == 1 && code.contains("fixture-scratch-clean.sh") {
            saw_clean = true;
        }
        if trimmed.ends_with("; then") {
            depth += 1;
        }
        if trimmed == "fi" {
            depth -= 1;
            if depth == 0 {
                closed = true;
                break;
            }
        }
    }
    assert!(
        closed,
        "status == 0 then-branch must end at `else`/`elif` or `fi`"
    );
    assert!(
        saw_clean,
        "status == 0 then-branch must invoke fixture-scratch-clean.sh"
    );
}

#[test]
fn the_scratch_clean_keeps_the_changeset_foreign_cache() {
    const SCRIPT: &str = include_str!("../../../scripts/fixture-scratch-clean.sh");
    let keep = SCRIPT
        .split_once("for dir in \"$tmp\"/oakum-*")
        .expect("oakum-* walk")
        .1
        .split_once("rm -rf \"$dir\"")
        .expect("removal follows the keep check")
        .0;
    // Name and continue must share the same live if-body, and continue must be
    // unconditional: a name only in a comment, a bare continue under another
    // branch, or continue nested under a different condition still deletes the
    // cache whenever that nested guard is false.
    let mut in_foreign = false;
    let mut saw_continue = false;
    let mut closed = false;
    for line in keep.lines() {
        let code = line.split_once('#').map_or(line, |(c, _)| c);
        let trimmed = code.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !in_foreign {
            if trimmed.contains("oakum-changeset-foreign") && trimmed.contains("if ") {
                in_foreign = true;
            }
            continue;
        }
        if !saw_continue {
            assert!(
                trimmed == "continue",
                "the oakum-changeset-foreign branch must continue unconditionally"
            );
            saw_continue = true;
            continue;
        }
        assert!(
            trimmed == "fi",
            "the oakum-changeset-foreign continue must be followed by `fi`"
        );
        closed = true;
        break;
    }
    assert!(
        in_foreign,
        "the oakum-* walk must have a live if naming oakum-changeset-foreign"
    );
    assert!(
        saw_continue && closed,
        "the oakum-changeset-foreign branch must continue before rm -rf"
    );
}

#[test]
fn mise_test_pre_cleans_fixture_scratch() {
    const MISE: &str = include_str!("../../../.mise.toml");
    let clean = MISE
        .split_once("[tasks.fixture-scratch-clean]")
        .expect("tasks.fixture-scratch-clean")
        .1
        .split_once("[tasks.")
        .map_or_else(
            || {
                MISE.split_once("[tasks.fixture-scratch-clean]")
                    .expect("tasks.fixture-scratch-clean")
                    .1
            },
            |(body, _)| body,
        );
    assert!(
        clean.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("scripts/fixture-scratch-clean.sh")
        }),
        "tasks.fixture-scratch-clean must run scripts/fixture-scratch-clean.sh"
    );
    let test = MISE
        .split_once("[tasks.test]")
        .expect("tasks.test")
        .1
        .split_once("[tasks.")
        .map_or_else(
            || MISE.split_once("[tasks.test]").expect("tasks.test").1,
            |(body, _)| body,
        );
    assert!(
        test.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("depends = [\"fixture-scratch-clean\"]")
        }),
        "tasks.test must live-depend on fixture-scratch-clean"
    );
    assert!(
        test.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("depends_post = [\"fixture-leak-check\"]")
        }),
        "tasks.test must live-post-depend on fixture-leak-check"
    );
}

#[test]
fn a_find_name_only_in_a_comment_is_not_a_live_operand() {
    let marked = "\\\n  \\( -name .oakum-fixture -o \\\n# -name .oakum-unit-fixture \\\n  \\)\n";
    assert!(
        !marked.lines().any(|line| {
            let code = line.split_once('#').map_or(line, |(c, _)| c);
            code.contains("-name .oakum-unit-fixture")
        }),
        "a commented -name must not satisfy the live-operand check"
    );
}

#[test]
#[should_panic(expected = "nested block comment")]
fn nested_block_comments_are_refused() {
    let mut in_block = false;
    let _ = strip_block_comments("/* outer /* inner */", &mut in_block);
}

#[test]
#[should_panic(expected = "attribute syntax")]
fn a_multiline_attribute_closer_with_a_line_comment_is_refused() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "oakum-attr-refuse-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::write(
        &path,
        "#[cfg(all(\n    unix\n))] // rationale\nfn base() -> PathBuf {\n    PathBuf::new()\n}\n",
    )
    .expect("write");
    let result = std::panic::catch_unwind(|| item_body(&path, "fn base() -> PathBuf {"));
    let _ = fs::remove_file(&path);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    panic!("expected attribute-syntax refuse");
}

/// The guard exists twice, and deliberately: `src/test_fixture.rs` may name no
/// process type, because `git_boundary.rs` walks `src/` and fails any file
/// there that does, while this side adds the git layer on top. Seventy code
/// lines are shared, and that half drifted three times in two commits with
/// nothing but review to catch it.
///
/// `Fixture::new` is excluded — only this side writes a `gitconfig` — and so is
/// `MARKER`, which the test below pins as *different* on purpose.
#[test]
fn the_two_fixture_guards_share_one_implementation() {
    const SHARED: [&str; 6] = [
        "fn base() -> PathBuf {",
        "impl Deref for Fixture {",
        "impl AsRef<Path> for Fixture {",
        "impl AsRef<OsStr> for Fixture {",
        "impl Drop for Fixture {",
        "fn record_leak(container: &Path, err: &std::io::Error) -> std::io::Result<()> {",
    ];

    let (unit, integration) = guard_sources();
    for item in SHARED {
        let left = item_body(&unit, item);
        let right = item_body(&integration, item);
        if let Some((line, (from_unit, from_integration))) = left
            .iter()
            .zip(&right)
            .enumerate()
            .find(|(_, (l, r))| l != r)
        {
            panic!(
                "`{item}` differs at body line {line}:\n  \
                 src/test_fixture.rs:       {from_unit}\n  \
                 tests/support/fixture.rs:  {from_integration}\n\
                 Edit both. If the divergence is deliberate, drop `{item}` from \
                 SHARED and say there why the two may differ."
            );
        }
        assert_eq!(
            left.len(),
            right.len(),
            "`{item}` is {} lines in src/test_fixture.rs and {} in \
             tests/support/fixture.rs",
            left.len(),
            right.len()
        );
    }
}

/// `RETAIN` and `LEDGER` are reached by identifier from bodies the test above
/// compares, so a changed *value* leaves those bodies byte-identical while the
/// two guards read different environments and write different ledgers. Measured
/// before this existed: both drifts passed.
///
/// `MARKER` is the one that must differ — see
/// [`a_unit_marked_container_does_not_answer_an_integration_lookup`].
#[test]
fn the_two_fixture_guards_agree_on_the_names_they_share() {
    let (unit, integration) = guard_sources();
    for name in ["RETAIN", "LEDGER"] {
        assert_eq!(
            const_value(&unit, name),
            const_value(&integration, name),
            "`{name}` differs between the two fixture guards, so they read \
             different environments or write different ledgers while `impl Drop` \
             stays byte-identical"
        );
    }
    // Bind the parse to the compiled value so a wrong-line read cannot pass.
    assert_eq!(const_value(&integration, "LEDGER"), LEDGER);
    assert_ne!(
        const_value(&unit, "MARKER"),
        const_value(&integration, "MARKER"),
        "the two markers must differ: this side resolves a `gitconfig` by \
         finding its marker, so a shared name would answer with a path to a \
         file nothing wrote"
    );
}

fn guard_sources() -> (PathBuf, PathBuf) {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    (
        crate_root.join("src/test_fixture.rs"),
        crate_root.join("tests/support/fixture.rs"),
    )
}

fn guard_text(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("{} should be readable: {err}", path.display()))
}

/// The string a `const NAME: &str = "…";` declares (any visibility).
/// Skips block comments so a stale commented declaration cannot pass.
fn const_value(path: &Path, name: &str) -> String {
    let text = guard_text(path);
    let mut in_block = false;
    let mut declarations = Vec::new();
    for line in text.lines() {
        let Some(code) = strip_block_comments(line, &mut in_block) else {
            continue;
        };
        let line = code.trim_start();
        let line = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        let Some(value) = line.strip_prefix(&format!("const {name}: &str = ")) else {
            continue;
        };
        let Some(parsed) = value
            .split_once("//")
            .map_or(value, |(code, _)| code)
            .trim_end()
            .strip_suffix(';')
            .map(str::trim)
            .and_then(|v| v.strip_prefix('"'))
            .and_then(|v| v.strip_suffix('"'))
        else {
            continue;
        };
        declarations.push(String::from(parsed));
    }
    assert_eq!(
        declarations.len(),
        1,
        "{} carries {} parsable `const {name}: &str` declarations, not one",
        path.display(),
        declarations.len()
    );
    declarations.remove(0)
}

/// Live code on `line`, updating `in_block` across lines. `None` = no code.
/// Nested `/*` is refused: Rust nests, and the first `*/` would otherwise
/// treat a stale declaration between inner-close and outer-close as live.
fn strip_block_comments<'a>(line: &'a str, in_block: &mut bool) -> Option<&'a str> {
    if *in_block {
        assert!(
            !line.contains("/*"),
            "a nested block comment, which const_value cannot read safely: {line:?}"
        );
        return match line.find("*/") {
            Some(end) => {
                *in_block = false;
                let after = &line[end + 2..];
                if after.trim().is_empty() {
                    None
                } else {
                    Some(after)
                }
            }
            None => None,
        };
    }
    match line.find("/*") {
        None => Some(line),
        Some(start) => {
            let before = &line[..start];
            let rest = &line[start + 2..];
            assert!(
                !rest.contains("/*"),
                "a nested block comment, which const_value cannot read safely: {line:?}"
            );
            if let Some(end) = rest.find("*/") {
                let after = &rest[end + 2..];
                if before.trim().is_empty() && after.trim().is_empty() {
                    None
                } else if before.trim().is_empty() {
                    Some(after)
                } else if after.trim().is_empty() {
                    Some(before)
                } else {
                    panic!(
                        "a line carries code on both sides of a block comment, \
                         which const_value cannot read safely: {line:?}"
                    );
                }
            } else {
                *in_block = true;
                if before.trim().is_empty() {
                    None
                } else {
                    Some(before)
                }
            }
        }
    }
}

/// The body of one top-level item, from its opening line to the brace that
/// closes it, with comments, blank lines and indentation dropped so wording and
/// layout may differ where the code does not.
///
/// Braces are counted outside strings, char literals and line comments. Text
/// this scan cannot follow — an unterminated string, a block comment — is
/// refused by name rather than guessed at: a `}` inside one truncates the
/// comparison silently, which is the one way a parity test stops working
/// without saying so.
///
/// Two shared surfaces stay out of reach by construction, and neither is
/// guarded here: `Fixture::container` is indented, and `Fixture::new` differs
/// legitimately, so the ~25 lines they share are checked by nothing.
fn item_body(path: &Path, opening: &str) -> Vec<String> {
    let text = guard_text(path);
    let openings = text
        .lines()
        .filter(|line| line.trim_start() == opening)
        .count();
    assert_eq!(
        openings,
        1,
        "{} has {openings} lines reading `{opening}`, so the comparison would \
         pick one of them silently",
        path.display()
    );

    // Outer attrs sit above the opening; omit them and a solo `#[cfg]` would
    // fall outside the compared region. Multiline attrs are refused.
    let preceding: Vec<&str> = text
        .lines()
        .take_while(|line| line.trim_start() != opening)
        .collect();
    let mut attrs: Vec<String> = Vec::new();
    for line in preceding.iter().rev() {
        let trimmed = line.trim_start();
        let code = trimmed
            .split_once("//")
            .map_or(trimmed, |(c, _)| c)
            .trim_end();
        if code.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if code.starts_with("#[") {
            assert!(
                code.ends_with(']'),
                "`{opening}` in {} has a multiline attribute, which this \
                 comparison cannot attach safely",
                path.display()
            );
            attrs.push(String::from(code));
            continue;
        }
        // Closer of a multiline `#[cfg(all(`; omitting this check leaves copies equal.
        assert!(
            !(code.ends_with(']') || code.starts_with('#')),
            "`{opening}` in {} has attribute syntax this comparison \
             cannot attach safely: {code}",
            path.display()
        );
        break;
    }
    attrs.reverse();
    let mut body = attrs;

    let mut depth = 0i32;
    for line in text.lines().skip_while(|line| line.trim_start() != opening) {
        let (delta, refusal) = brace_delta(line);
        if let Some(reason) = refusal {
            panic!(
                "`{opening}` in {} contains {reason}, which this comparison \
                 cannot read safely",
                path.display()
            );
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            body.push(String::from(trimmed));
        }
        depth += delta;
        if depth == 0 {
            return body;
        }
    }
    panic!("`{opening}` in {} is never closed", path.display())
}

/// Net brace depth contributed by one line, plus the name of anything on it
/// this scan cannot follow.
fn brace_delta(line: &str) -> (i32, Option<&'static str>) {
    let chars: Vec<char> = line.chars().collect();
    let mut depth = 0;
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '/' if chars.get(index + 1) == Some(&'/') => return (depth, None),
            '/' if chars.get(index + 1) == Some(&'*') => return (depth, Some("a block comment")),
            '"' => {
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    index += if chars[index] == '\\' { 2 } else { 1 };
                }
                if index >= chars.len() {
                    return (depth, Some("a string left open at the end of a line"));
                }
            }
            // A char literal closes within one or two characters; a lifetime
            // does not, and falls through with its quote consumed harmlessly.
            '\'' => {
                let close = index
                    + if chars.get(index + 1) == Some(&'\\') {
                        3
                    } else {
                        2
                    };
                if chars.get(close) == Some(&'\'') {
                    index = close;
                }
            }
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    (depth, None)
}
