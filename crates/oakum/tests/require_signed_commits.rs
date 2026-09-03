//! Pre-push signed-commit hook: unsigned objects cannot leave the machine.

#![allow(clippy::disallowed_methods)]

mod support;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[cfg(windows)]
use support::command_on_path;
use support::fixture::{git, git_env, git_repo, git_stdout, plain_repo, Fixture};
use support::workspace_root;

const ZERO: &str = "0000000000000000000000000000000000000000";

fn hook_path() -> std::path::PathBuf {
    workspace_root().join("scripts/require-signed-commits.sh")
}

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("signed-commits", label)
}

fn run_hook(root: &Path, remote: &str, stdin: &str) -> Output {
    let mut command = hook_command();
    git_env(&mut command, root);
    let mut child = command
        .arg(remote)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("require-signed-commits.sh");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    drop(child.stdin.take());
    child.wait_with_output().expect("hook")
}

fn hook_command() -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new(windows_git_bash());
        command.arg(hook_path());
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(hook_path())
    }
}

/// CreateProcess searches `System32` before PATH, so `Command::new("bash")`
/// is the WSL stub (`bash.exe` that exits 1 with no distro), not Git Bash.
#[cfg(windows)]
fn windows_git_bash() -> PathBuf {
    if let Some(path) = bash_beside_git_exec_path(&git_exec_path()) {
        return path;
    }
    let program = PathBuf::from(command_on_path("bash").get_program());
    assert!(
        program.is_absolute() && !is_wsl_bash_stub(&program),
        "need Git Bash to run scripts/require-signed-commits.sh; \
         CreateProcess prefers the WSL stub over PATH (got {})",
        program.display()
    );
    program
}

#[cfg(windows)]
fn git_exec_path() -> PathBuf {
    let output = Command::new("git")
        .arg("--exec-path")
        .output()
        .expect("git --exec-path");
    assert!(
        output.status.success(),
        "git --exec-path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}

/// Git for Windows: `<prefix>/mingw64/libexec/git-core` → `<prefix>/bin/bash.exe`.
fn bash_beside_git_exec_path(exec_path: &Path) -> Option<PathBuf> {
    let git_root = exec_path.ancestors().nth(3)?;
    for rel in ["bin/bash.exe", "usr/bin/bash.exe"] {
        let candidate = git_root.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn is_wsl_bash_stub(path: &Path) -> bool {
    let Some(file) = path.file_name() else {
        return false;
    };
    if !file.eq_ignore_ascii_case("bash.exe") && !file.eq_ignore_ascii_case("bash") {
        return false;
    }
    path.parent().and_then(Path::file_name).is_some_and(|dir| {
        dir.eq_ignore_ascii_case("System32") || dir.eq_ignore_ascii_case("SysWOW64")
    })
}

fn push_line(local_sha: &str, remote_sha: &str) -> String {
    format!("refs/heads/topic {local_sha} refs/heads/topic {remote_sha}\n")
}

fn write_commit(root: &Path, object: &str) -> String {
    let mut hash_object = Command::new("git");
    git_env(&mut hash_object, root);
    let mut child = hash_object
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hash-object");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(object.as_bytes())
        .expect("write object");
    let written = child.wait_with_output().expect("hash-object");
    assert!(
        written.status.success(),
        "hash-object: {}",
        String::from_utf8_lossy(&written.stderr)
    );
    String::from_utf8(written.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

fn commit_object(tree: &str, parent: Option<&str>, extra_headers: &str, message: &str) -> String {
    let mut object = format!("tree {tree}\n");
    if let Some(parent) = parent {
        object.push_str("parent ");
        object.push_str(parent);
        object.push('\n');
    }
    object.push_str(
        "author oakum test <oakum@test.invalid> 1700000000 +0000\n\
         committer oakum test <oakum@test.invalid> 1700000000 +0000\n",
    );
    object.push_str(extra_headers);
    object.push('\n');
    object.push_str(message);
    if !message.ends_with('\n') {
        object.push('\n');
    }
    object
}

fn gpgsig_header(kind: &str) -> String {
    format!("{kind} -----BEGIN SSH SIGNATURE-----\n fake\n -----END SSH SIGNATURE-----\n")
}

fn seed_tree(root: &Path) -> String {
    std::fs::write(root.join("f"), "x").expect("file");
    git(root, &["add", "-A"]);
    git(root, &["commit", "--no-verify", "-m", "unsigned seed"]);
    git_stdout(root, &["rev-parse", "HEAD^{tree}"])
}

#[test]
fn an_unsigned_commit_is_refused() {
    let root = temp_git_repo("unsigned");
    let tree = seed_tree(&root);
    let parent = git_stdout(&root, &["rev-parse", "HEAD"]);
    let sha = write_commit(
        &root,
        &commit_object(&tree, Some(&parent), "", "unsigned\n"),
    );
    let out = run_hook(&root, "origin", &push_line(&sha, &parent));
    assert!(
        !out.status.success(),
        "unsigned commit must fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("has no gpgsig header"),
        "stderr should name the missing header: {stderr}"
    );
    assert!(
        !stderr.contains("GIT_CONFIG_GLOBAL"),
        "hook stderr is for git push, not fixture isolation: {stderr}"
    );
}

#[test]
fn a_gpgsig_line_in_the_message_is_not_a_signature() {
    let root = temp_git_repo("spoof");
    let tree = seed_tree(&root);
    let parent = git_stdout(&root, &["rev-parse", "HEAD"]);
    let sha = write_commit(
        &root,
        &commit_object(
            &tree,
            Some(&parent),
            "",
            "gpgsig -----BEGIN SSH SIGNATURE-----\n",
        ),
    );
    let out = run_hook(&root, "origin", &push_line(&sha, &parent));
    assert!(
        !out.status.success(),
        "body spoof must fail, got exit success: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("has no gpgsig header"),
        "stderr should name the missing header: {stderr}"
    );
}

#[test]
fn gpgsig_and_gpgsig_sha256_headers_pass() {
    let root = temp_git_repo("headers");
    let tree = seed_tree(&root);
    let parent = git_stdout(&root, &["rev-parse", "HEAD"]);
    for kind in ["gpgsig", "gpgsig-sha256"] {
        let sha = write_commit(
            &root,
            &commit_object(&tree, Some(&parent), &gpgsig_header(kind), "ok\n"),
        );
        let out = run_hook(&root, "origin", &push_line(&sha, &parent));
        assert!(
            out.status.success(),
            "{kind} header must pass: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_large_signed_message_does_not_sigpipe_into_a_reject() {
    let root = temp_git_repo("large");
    let tree = seed_tree(&root);
    let parent = git_stdout(&root, &["rev-parse", "HEAD"]);
    let message = "x".repeat(1_000_000) + "\n";
    let sha = write_commit(
        &root,
        &commit_object(&tree, Some(&parent), &gpgsig_header("gpgsig"), &message),
    );
    let out = run_hook(&root, "origin", &push_line(&sha, &parent));
    assert!(
        out.status.success(),
        "large signed message must pass: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn delete_ref_and_empty_range_pass() {
    let root = temp_git_repo("skip");
    seed_tree(&root);
    let head = git_stdout(&root, &["rev-parse", "HEAD"]);
    let delete = run_hook(
        &root,
        "origin",
        &format!("refs/heads/topic {ZERO} refs/heads/topic {head}\n"),
    );
    assert!(
        delete.status.success(),
        "delete-ref must pass: {}",
        String::from_utf8_lossy(&delete.stderr)
    );
    let empty = run_hook(&root, "origin", &push_line(&head, &head));
    assert!(
        empty.status.success(),
        "empty range must pass: {}",
        String::from_utf8_lossy(&empty.stderr)
    );
}

#[test]
fn an_unsigned_commit_on_another_remote_is_still_checked_for_origin() {
    let root = temp_git_repo("other-remote");
    seed_tree(&root);
    let unsigned = git_stdout(&root, &["rev-parse", "HEAD"]);
    git(&root, &["update-ref", "refs/remotes/other/main", &unsigned]);
    let out = run_hook(&root, "origin", &push_line(&unsigned, ZERO));
    assert!(
        !out.status.success(),
        "unsigned commit already on another remote must still fail a new origin branch: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&unsigned) && stderr.contains("has no gpgsig header"),
        "stderr should name the unsigned commit {unsigned}: {stderr}"
    );
}

fn push_tag(local_sha: &str, remote_sha: &str) -> String {
    format!("refs/tags/v1 {local_sha} refs/tags/v1 {remote_sha}\n")
}

#[test]
fn a_signed_tip_does_not_cover_an_unsigned_ancestor_on_a_new_branch() {
    let root = temp_git_repo("unsigned-ancestor");
    let tree = seed_tree(&root);
    let unsigned = git_stdout(&root, &["rev-parse", "HEAD"]);
    let tip = write_commit(
        &root,
        &commit_object(
            &tree,
            Some(&unsigned),
            &gpgsig_header("gpgsig"),
            "signed tip\n",
        ),
    );
    let new_branch = run_hook(&root, "origin", &push_line(&tip, ZERO));
    assert!(
        !new_branch.status.success(),
        "new branch must still refuse the unsigned ancestor: {}",
        String::from_utf8_lossy(&new_branch.stderr)
    );
    let stderr = String::from_utf8_lossy(&new_branch.stderr);
    assert!(
        stderr.contains(&unsigned),
        "stderr should name the unsigned parent {unsigned}: {stderr}"
    );
    let incremental = run_hook(&root, "origin", &push_line(&tip, &unsigned));
    assert!(
        incremental.status.success(),
        "range from the unsigned parent must only inspect the signed child: {}",
        String::from_utf8_lossy(&incremental.stderr)
    );
}

#[test]
fn an_unsigned_tag_is_refused() {
    let root = temp_git_repo("unsigned-tag");
    seed_tree(&root);
    let unsigned = git_stdout(&root, &["rev-parse", "HEAD"]);
    let out = run_hook(&root, "origin", &push_tag(&unsigned, ZERO));
    assert!(
        !out.status.success(),
        "unsigned tag must fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refs/tags/v1"),
        "stderr should name the tag ref: {stderr}"
    );
}

#[test]
fn a_later_unsigned_ref_is_still_checked() {
    let root = temp_git_repo("two-refs");
    let tree = seed_tree(&root);
    let parent = git_stdout(&root, &["rev-parse", "HEAD"]);
    let unsigned = write_commit(
        &root,
        &commit_object(&tree, Some(&parent), "", "unsigned\n"),
    );
    let stdin = format!(
        "refs/heads/keep {ZERO} refs/heads/keep {parent}\n{}",
        push_line(&unsigned, &parent)
    );
    let out = run_hook(&root, "origin", &stdin);
    assert!(
        !out.status.success(),
        "second unsigned ref must fail after a delete: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&unsigned) && stderr.contains("has no gpgsig header"),
        "stderr should name the unsigned commit {unsigned}: {stderr}"
    );
}

#[test]
fn a_signed_replacement_does_not_launder_an_unsigned_object() {
    let root = temp_git_repo("replace");
    let tree = seed_tree(&root);
    let unsigned = git_stdout(&root, &["rev-parse", "HEAD"]);
    let signed = write_commit(
        &root,
        &commit_object(&tree, None, &gpgsig_header("gpgsig"), "replacement\n"),
    );
    git(&root, &["replace", &unsigned, &signed]);
    let out = run_hook(&root, "origin", &push_line(&unsigned, ZERO));
    assert!(
        !out.status.success(),
        "unsigned object with a signed replacement must still fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&unsigned) && stderr.contains("has no gpgsig header"),
        "stderr should name the unsigned object {unsigned}: {stderr}"
    );
}

fn git_for_windows_git_core(label: &str) -> (Fixture, PathBuf) {
    let root = plain_repo("signed-commits", label);
    let git_core = root.join("mingw64").join("libexec").join("git-core");
    std::fs::create_dir_all(&git_core).expect("git-core");
    (root, git_core)
}

#[test]
fn git_for_windows_layout_finds_bash() {
    let (root, git_core) = git_for_windows_git_core("git-bash-layout");
    let bash = root.join("bin").join("bash.exe");
    std::fs::create_dir_all(bash.parent().expect("bin")).expect("bin");
    std::fs::write(&bash, []).expect("bash.exe");
    assert_eq!(
        bash_beside_git_exec_path(&git_core).as_deref(),
        Some(bash.as_path())
    );
}

#[test]
fn git_for_windows_usr_bin_bash_is_found_when_bin_is_absent() {
    let (root, git_core) = git_for_windows_git_core("git-bash-usr");
    let bash = root.join("usr").join("bin").join("bash.exe");
    std::fs::create_dir_all(bash.parent().expect("usr/bin")).expect("usr/bin");
    std::fs::write(&bash, []).expect("usr bash.exe");
    assert_eq!(
        bash_beside_git_exec_path(&git_core).as_deref(),
        Some(bash.as_path())
    );
}

#[test]
fn git_for_windows_layout_without_bash_is_none() {
    let (_root, git_core) = git_for_windows_git_core("git-bash-missing");
    assert!(bash_beside_git_exec_path(&git_core).is_none());
}

#[cfg(windows)]
#[test]
fn windows_hook_runner_is_not_the_wsl_stub() {
    let program = Path::new(hook_command().get_program());
    assert!(
        program.is_absolute() && program.is_file() && !is_wsl_bash_stub(program),
        "hook runner must be Git Bash, not the WSL stub: {}",
        program.display()
    );
}

#[test]
fn system32_bash_is_the_wsl_stub() {
    assert!(is_wsl_bash_stub(
        &Path::new("C:")
            .join("Windows")
            .join("System32")
            .join("bash.exe")
    ));
    assert!(is_wsl_bash_stub(
        &Path::new("C:")
            .join("Windows")
            .join("SysWOW64")
            .join("bash")
    ));
    assert!(!is_wsl_bash_stub(
        &Path::new("C:")
            .join("Program Files")
            .join("Git")
            .join("bin")
            .join("bash.exe")
    ));
}
