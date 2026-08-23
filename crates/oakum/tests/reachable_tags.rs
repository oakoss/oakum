//! `oakum reachable-tags`: tags reachable from HEAD, not every tag.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn git_stdout(root: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn temp_git_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "oakum-reachable-tags-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    let hooks = dir.join("no-hooks");
    fs::create_dir(&hooks).expect("no-hooks");
    git(&dir, &["config", "core.hooksPath", "no-hooks"]);
    dir
}

fn commit(root: &std::path::Path, message: &str) {
    fs::write(root.join("f"), message).expect("file");
    git(root, &["add", "f"]);
    git(root, &["commit", "--no-verify", "-m", message]);
}

fn head_hash(root: &std::path::Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn reachable(root: &std::path::Path) -> (bool, String, String) {
    let out = bin()
        .arg("reachable-tags")
        .current_dir(root)
        .output()
        .expect("oakum");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn no_tags_is_ok_not_unverified() {
    let root = temp_git_repo("empty");
    commit(&root, "init");
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        !stderr.contains("unverified"),
        "empty tag history is a look, not unverified: {stderr}"
    );
}

#[test]
fn not_a_repo_is_error_not_empty() {
    let dir = std::env::temp_dir().join(format!(
        "oakum-reachable-tags-norepo-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("dir");
    let (ok, stdout, stderr) = reachable(&dir);
    assert!(!ok, "expected failure in a non-repo");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("rev-parse") || stderr.contains("for-each-ref"),
        "{stderr}"
    );
}

#[test]
fn lightweight_and_annotated_tags_on_head() {
    let root = temp_git_repo("both");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(
        &root,
        &["tag", "-a", "--no-sign", "v0.1.0-ann", "-m", "annotated"],
    );
    assert_eq!(git_stdout(&root, &["cat-file", "-t", "v0.1.0-ann"]), "tag");
    let sha = head_hash(&root);
    assert_ne!(git_stdout(&root, &["rev-parse", "v0.1.0-ann"]), sha);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    let mut lines: Vec<String> = stdout.lines().map(String::from).collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        vec![format!("{sha}\tv0.1.0"), format!("{sha}\tv0.1.0-ann")]
    );
}

#[test]
fn tag_on_another_branch_is_not_reachable() {
    let root = temp_git_repo("branch");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["checkout", "-b", "release"]);
    commit(&root, "hotfix");
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["checkout", "main"]);
    let sha = head_hash(&root);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_eq!(stdout.trim(), format!("{sha}\tv0.1.0"));
    assert!(!stdout.contains("v0.1.1"), "{stdout}");
}

#[test]
fn ancestor_tag_is_reachable() {
    let root = temp_git_repo("ancestor");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let first = head_hash(&root);
    commit(&root, "later");
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_eq!(stdout.trim(), format!("{first}\tv0.1.0"));
}

#[test]
fn tag_name_is_not_prefixed_when_a_branch_shares_it() {
    let root = temp_git_repo("collision");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["branch", "v0.1.0"]);
    let sha = head_hash(&root);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_eq!(stdout.trim(), format!("{sha}\tv0.1.0"));
    assert!(!stdout.contains("tags/v0.1.0"), "{stdout}");
}

#[test]
fn shallow_clone_is_unverified() {
    let src = temp_git_repo("shallow-src");
    commit(&src, "one");
    commit(&src, "two");
    git(&src, &["tag", "v0.1.0"]);
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "oakum-reachable-tags-shallow-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dest);
    let status = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone --depth=1 failed");
    assert!(
        !git_stdout(&dest, &["tag", "--list", "v0.1.0"]).is_empty(),
        "shallow dest should still carry the HEAD tag"
    );
    let (ok, stdout, stderr) = reachable(&dest);
    assert!(!ok, "shallow clone must not look like never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
}

#[test]
fn no_tags_clone_is_unverified_not_never_released() {
    let src = temp_git_repo("notags-src");
    commit(&src, "one");
    commit(&src, "two");
    git(&src, &["tag", "v0.1.0"]);
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "oakum-reachable-tags-notags-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dest);
    let status = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--no-tags",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone --no-tags failed");
    assert_eq!(
        git_stdout(&dest, &["rev-parse", "--is-shallow-repository"]),
        "false",
        "clone must be full-history so only tag suppression triggers"
    );
    assert!(
        git_stdout(&dest, &["tag", "--list"]).is_empty(),
        "the --no-tags clone should carry no tags"
    );
    let (ok, stdout, stderr) = reachable(&dest);
    assert!(
        !ok,
        "tag-suppressed clone must not look like never-released"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("tagOpt --no-tags"), "{stderr}");
    assert!(
        stderr.contains("git config --replace-all remote.origin.tagOpt --tags"),
        "diagnostic must name the config change that clears the check: {stderr}"
    );
    assert!(
        stderr.contains("git fetch --tags -- origin"),
        "diagnostic must name the corrective fetch: {stderr}"
    );
    assert!(
        !stderr.contains("never released"),
        "diagnostic must not claim release history: {stderr}"
    );
}

#[test]
fn tag_opt_set_after_the_clone_is_unverified() {
    let root = temp_git_repo("tagopt-after");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["config", "remote.origin.tagOpt", "--no-tags"]);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(!ok, "suppression applied after cloning must still fail");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("tagOpt --no-tags"), "{stderr}");
}

#[test]
fn metacharacter_remote_name_is_quoted_with_a_posix_note() {
    let root = temp_git_repo("tagopt-meta");
    commit(&root, "init");
    git(&root, &["config", "remote.foo$bar.tagopt", "--no-tags"]);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(!ok, "suppression on any remote must fail");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("'foo$bar'"), "{stderr}");
    assert!(
        stderr.contains("POSIX shell quoting"),
        "quoted commands must carry the shell caveat: {stderr}"
    );
}

#[test]
fn tag_opt_tags_value_still_lists_tags() {
    let root = temp_git_repo("tagopt-tags");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["config", "remote.origin.tagOpt", "--tags"]);
    let sha = head_hash(&root);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_eq!(stdout.trim(), format!("{sha}\tv0.1.0"));
}

#[test]
fn unborn_head_is_error_not_empty() {
    let root = temp_git_repo("unborn");
    let (ok, stdout, stderr) = reachable(&root);
    assert!(!ok, "unborn HEAD must not look like never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("for-each-ref"), "{stderr}");
}

#[test]
fn two_reachable_tagged_commits_are_both_listed() {
    let root = temp_git_repo("two");
    commit(&root, "one");
    git(&root, &["tag", "v0.1.0"]);
    let first = head_hash(&root);
    commit(&root, "two");
    git(&root, &["tag", "v0.2.0"]);
    let second = head_hash(&root);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    let mut lines: Vec<String> = stdout.lines().map(String::from).collect();
    lines.sort_unstable();
    let mut expected = vec![format!("{first}\tv0.1.0"), format!("{second}\tv0.2.0")];
    expected.sort_unstable();
    assert_eq!(lines, expected);
}

#[test]
fn tag_on_a_merged_branch_is_reachable() {
    let root = temp_git_repo("merge");
    commit(&root, "init");
    git(&root, &["checkout", "-b", "feat"]);
    commit(&root, "feat");
    git(&root, &["tag", "v0.1.0"]);
    let tagged = head_hash(&root);
    git(&root, &["checkout", "main"]);
    git(
        &root,
        &["merge", "--no-ff", "--no-edit", "--no-verify", "feat"],
    );
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_ne!(head_hash(&root), tagged);
    assert_eq!(stdout.trim(), format!("{tagged}\tv0.1.0"));
}

#[test]
fn untagged_shallow_clone_is_unverified() {
    let src = temp_git_repo("shallow-untagged-src");
    commit(&src, "one");
    commit(&src, "two");
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "oakum-reachable-tags-shallow-untagged-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dest);
    let status = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone --depth=1 failed");
    let (ok, stdout, stderr) = reachable(&dest);
    assert!(
        !ok,
        "untagged shallow clone must not look like never-released"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
}

#[test]
fn nested_annotated_tag_peels_to_the_commit() {
    let root = temp_git_repo("nested");
    commit(&root, "init");
    let sha = head_hash(&root);
    git(&root, &["tag", "-a", "--no-sign", "inner", "-m", "inner"]);
    git(
        &root,
        &["tag", "-a", "--no-sign", "outer", "inner", "-m", "outer"],
    );
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    let mut lines: Vec<String> = stdout.lines().map(String::from).collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        vec![format!("{sha}\tinner"), format!("{sha}\touter")]
    );
}

/// Event path must be absolute: git rejects a relative `GIT_TRACE2_EVENT`
/// and still exits 0, so the count would be 0.
fn git_processes(label: &str, root: &std::path::Path) -> Vec<Vec<String>> {
    let events = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-trace2-{label}-{}.event", std::process::id()));
    assert!(
        events.is_absolute(),
        "GIT_TRACE2_EVENT requires an absolute path, got {}",
        events.display()
    );
    let _ = fs::remove_file(&events);
    fs::write(&events, "").expect("truncate event file");
    let out = bin()
        .arg("reachable-tags")
        .current_dir(root)
        .env("GIT_TRACE2_EVENT", &events)
        .output()
        .expect("oakum");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    trace2_start_argvs(&events)
}

fn trace2_start_argvs(path: &std::path::Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut argvs = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).expect("trace2 json");
        if value.get("event").and_then(|event| event.as_str()) != Some("start") {
            continue;
        }
        let argv = value
            .get("argv")
            .and_then(|argv| argv.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        argvs.push(argv);
    }
    argvs
}

#[test]
fn discovery_subprocess_count_is_constant_in_the_tag_count() {
    let repo_one = temp_git_repo("bounded-one");
    commit(&repo_one, "init");
    git(&repo_one, &["tag", "v0.1.0"]);
    let repo_many = temp_git_repo("bounded-many");
    commit(&repo_many, "init");
    for i in 0..12 {
        git(&repo_many, &["tag", &format!("v0.{i}.0")]);
        git(
            &repo_many,
            &["tag", "-a", "--no-sign", &format!("ann-{i}"), "-m", "ann"],
        );
    }
    let one = git_processes("one", &repo_one);
    let many = git_processes("many", &repo_many);
    assert_eq!(
        many.len(),
        3,
        "discovery contract: three git processes; got {many:?}"
    );
    assert_eq!(one, many, "git argv lists must not grow with tag count");
    let cmds: Vec<&str> = many
        .iter()
        .map(|argv| argv.get(1).map_or("", String::as_str))
        .collect();
    assert_eq!(
        cmds,
        ["rev-parse", "config", "for-each-ref"],
        "discovery contract: shallow check + tagOpt check + for-each-ref; got {many:?}"
    );
}

#[test]
fn tag_on_a_blob_is_omitted_by_merged_filtering() {
    let root = temp_git_repo("blob-tag");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let sha = head_hash(&root);
    let blob = git_stdout(&root, &["hash-object", "-w", "f"]);
    git(&root, &["tag", "blob-tag", &blob]);
    // Pins observed git behavior: --merged=HEAD only matches refs that peel
    // to commits, so a blob tag never reaches the parser. If a git version
    // ever lists it, the parser refuses it as unverified — fail closed
    // either way, never a silently wrong version.
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_eq!(stdout.trim(), format!("{sha}\tv0.1.0"));
    assert!(!stdout.contains("blob-tag"), "{stdout}");
}

#[test]
fn detached_head_still_lists_reachable_tags() {
    let root = temp_git_repo("detach");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let first = head_hash(&root);
    commit(&root, "later");
    git(&root, &["checkout", "--detach", "HEAD"]);
    let (ok, stdout, stderr) = reachable(&root);
    assert!(ok, "{stderr}");
    assert_eq!(stdout.trim(), format!("{first}\tv0.1.0"));
}
