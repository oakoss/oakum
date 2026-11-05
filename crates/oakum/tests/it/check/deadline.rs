use super::*;

/// A credential helper runs with oakum's environment applied and blocks
/// anyway — `GIT_ASKPASS` and `GIT_TERMINAL_PROMPT` both reach it and neither
/// stops it — so only the wall-clock deadline bounds it. Expiry is the third
/// outcome, never a completed look.
#[cfg(unix)]
#[test]
fn a_blocking_credential_helper_meets_the_deadline() {
    let _turn = deadline_turn();
    let root = tagged_cargo("deadline-helper", &["0.1.0"]);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/demo/demo.git/info/refs");
        then.status(401)
            .header("WWW-Authenticate", "Basic realm=\"git\"");
    });
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            &format!("{}/demo/demo.git", server.base_url()),
        ],
    );
    git(
        &root,
        &["config", "credential.helper", "!f() { sleep 60; }; f"],
    );

    let started = std::time::Instant::now();
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("OAKUM_REMOTE_DEADLINE", "2")
        .output()
        .expect("oakum");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the deadline must bound the helper's sleep"
    );
    assert!(!out.status.success(), "an expired deadline must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
    assert!(stderr.contains("credential helper"), "{stderr}");
    assert!(stderr.contains("OAKUM_REMOTE_DEADLINE"), "{stderr}");
}

/// Runs `command` for at most 30 seconds. A run still going then is the hang
/// the deadline exists to prevent: `release` frees whatever the child waits
/// on, and the test fails instead of stalling the suite.
#[cfg(unix)]
fn run_bounded(command: &mut Command, release: impl Fn()) -> (Option<i32>, String) {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("oakum");
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            release();
            panic!("oakum did not finish: a discovery child hung");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let output = child.wait_with_output().expect("output");
    (
        status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A pnpm workspace whose `pnpm` blocks on `root -w` and `list`; the shim
/// `exec`s, so the child oakum kills is the one that blocks.
#[cfg(unix)]
fn blocking_pnpm_repo(label: &str) -> (Fixture, std::ffi::OsString) {
    let root = temp_git_repo(label);
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"web\",\n  \"version\": \"0.1.0\"\n}\n",
    )
    .expect("package.json");
    fs::write(root.join("pnpm-workspace.yaml"), "packages: []\n").expect("workspace");
    let shims = support::tools::path_shim(
        &root,
        "pnpm",
        "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in root|list) sleep 60; exit;; esac; done\necho 12.0.0\n",
    );
    let path = support::tools::path_prefixed_by(&shims);
    (root, path)
}

/// Each pnpm child runs under the same deadline, and every command that
/// discovers reports it unverified.
#[cfg(unix)]
#[test]
fn a_pnpm_child_that_never_answers_is_unverified() {
    let (root, path) = blocking_pnpm_repo("discovery-pnpm");
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    for args in [
        vec!["check"],
        vec!["add", "--packages", "web:patch", "--message", "m"],
    ] {
        let (code, stderr) = run_bounded(
            support::fixture::oakum(&root)
                .args(&args)
                .env("PATH", &path)
                .env("OAKUM_REMOTE_DEADLINE", "2"),
            || {},
        );
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.contains("`pnpm root -w` gave no answer within 2s"),
            "{args:?}: {stderr}"
        );
        // The shim waits on `sleep` rather than becoming it, as a wrapper does.
        assert!(
            stderr.contains("so oakum killed it and what was still running under it (`sleep`)"),
            "{args:?}: {stderr}"
        );
    }
}

/// `init` discovers before it writes, and a child that never answered leaves
/// it unverified rather than refused.
#[cfg(unix)]
#[test]
fn init_reports_a_discovery_child_that_never_answers_as_unverified() {
    let (root, path) = blocking_pnpm_repo("discovery-pnpm-init");
    commit(&root, "init");
    let (code, stderr) = run_bounded(
        support::fixture::oakum(&root)
            .args([
                "init",
                "--versioning",
                "semver",
                "--change-files",
                "true",
                "--conventional-commits",
                "false",
            ])
            .env("PATH", &path)
            .env("OAKUM_REMOTE_DEADLINE", "2"),
        || {},
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("gave no answer within 2s"), "{stderr}");
    assert!(!root.join(".changeset").exists(), "init wrote nothing");
}

/// One tool timing out beside another failing outright is a finding, not a
/// look that did not happen: the failure decides.
#[cfg(unix)]
#[test]
fn a_failing_tool_outranks_one_that_never_answered() {
    let root = temp_git_repo("discovery-mixed");
    cargo_members(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"web\",\n  \"version\": \"0.1.0\"\n}\n",
    )
    .expect("package.json");
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    let manifest = root.join("beta/Cargo.toml");
    fs::remove_file(&manifest).expect("remove manifest");
    let made = Command::new("mkfifo")
        .arg(&manifest)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo: {made}");
    let shims = support::tools::path_shim(
        &root,
        "pnpm",
        "#!/bin/sh\necho 'ERR_PNPM_BROKEN: config is broken' >&2\nexit 1\n",
    );
    let release = || {
        let fifo = manifest.clone();
        std::thread::spawn(move || drop(fs::OpenOptions::new().write(true).open(fifo)));
    };

    let (code, stderr) = run_bounded(
        support::fixture::oakum(&root)
            .args(["check"])
            .env("PATH", support::tools::path_prefixed_by(&shims))
            .env("OAKUM_REMOTE_DEADLINE", "2"),
        release,
    );

    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("`cargo metadata` gave no answer"),
        "{stderr}"
    );
    assert!(stderr.contains("ERR_PNPM_BROKEN"), "{stderr}");
}

/// The same rule from the other side: cargo refusing a broken manifest beside
/// a pnpm that never answers still exits as a finding.
#[cfg(unix)]
#[test]
fn a_failing_cargo_outranks_a_pnpm_that_never_answered() {
    let (root, path) = blocking_pnpm_repo("discovery-mixed-mirror");
    cargo_members(&root, &[("alpha", "0.1.0")]);
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    fs::write(root.join("alpha/Cargo.toml"), "[package\n").expect("break manifest");

    let (code, stderr) = run_bounded(
        support::fixture::oakum(&root)
            .args(["check"])
            .env("PATH", &path)
            .env("OAKUM_REMOTE_DEADLINE", "2"),
        || {},
    );

    assert!(stderr.contains("gave no answer within 2s"), "{stderr}");
    assert_eq!(code, Some(1), "{stderr}");
}

/// A malformed deadline means no discovery child ran: the look did not
/// happen, as for git's children.
#[test]
fn a_malformed_deadline_leaves_discovery_unverified() {
    let root = temp_git_repo("discovery-bad-deadline");
    cargo_members(&root, &[("alpha", "0.1.0")]);
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    let out = support::fixture::oakum(&root)
        .args(["add", "--packages", "alpha:patch", "--message", "m"])
        .env("OAKUM_REMOTE_DEADLINE", "abc")
        .output()
        .expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("cargo: OAKUM_REMOTE_DEADLINE must be a positive whole number"),
        "{stderr}"
    );
}

/// okm-ifc9: `cargo metadata` blocks opening a member manifest that is a FIFO.
/// Discovery runs it under the deadline, so the look ends unverified instead
/// of hanging. The test bounds the run itself, so a hang fails rather than
/// stalling the suite.
#[cfg(unix)]
#[test]
fn a_discovery_child_that_never_answers_is_unverified() {
    let root = temp_git_repo("discovery-fifo");
    cargo_members(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    let manifest = root.join("beta/Cargo.toml");
    fs::remove_file(&manifest).expect("remove manifest");
    let made = Command::new("mkfifo")
        .arg(&manifest)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo: {made}");

    // On a hang, writing the FIFO once releases the orphaned `cargo metadata`.
    let release = || {
        let fifo = manifest.clone();
        std::thread::spawn(move || drop(fs::OpenOptions::new().write(true).open(fifo)));
    };
    let (code, stderr) = run_bounded(
        support::fixture::oakum(&root)
            .args(["check"])
            .env("OAKUM_REMOTE_DEADLINE", "2"),
        release,
    );

    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("`cargo metadata` gave no answer within 2s"),
        "{stderr}"
    );
    assert!(stderr.contains("OAKUM_REMOTE_DEADLINE"), "{stderr}");
}

/// A rejected `OAKUM_REMOTE_DEADLINE` refuses loudly, naming the variable —
/// never a silent fall-back to the default that would quietly discard the
/// deadline the user asked for.
#[cfg(unix)]
#[test]
fn a_malformed_deadline_refuses_loudly() {
    let root = tagged_cargo("deadline-malformed", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "https://host.invalid/r.git"],
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("OAKUM_REMOTE_DEADLINE", "abc")
        .output()
        .expect("oakum");
    assert!(!out.status.success(), "a rejected deadline must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("positive whole number"),
        "the refusal must name the constraint: {stderr}"
    );
    assert!(stderr.contains("OAKUM_REMOTE_DEADLINE"), "{stderr}");
    assert!(
        !stderr.contains("could not run git"),
        "a config mistake must not read as a spawn failure: {stderr}"
    );
}

/// The child can exit while something it spawned holds the pipes open; that
/// is not a kill, and the report says what actually happened — the exit
/// status in hand, the output uncollectable.
///
/// Wider deadline than its siblings: the exit status must be reaped before
/// expiry, which lost 1 run in 4 to spawn latency at two seconds
/// (`okm-404.53`). The drain never completes, so the wait is spent either way.
/// That budget is why it runs outside `deadline_turn`: contention the 2s
/// siblings cannot absorb, 10s can.
#[cfg(unix)]
#[test]
fn a_grandchild_holding_the_pipes_meets_the_deadline_without_a_kill_claim() {
    let root = tagged_cargo("deadline-drain", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "https://host.invalid/r.git"],
    );
    let real = ambient_tool("git");
    // Outlives the 10s deadline and little more: an orphan the kill cannot reach.
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\ncase \"$1\" in ls-remote) sleep 15 & exit 0 ;; esac\nexec {real} \"$@\"\n",
            real = real.display()
        ),
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path_prefixed_by(&shim_dir))
        .env("OAKUM_REMOTE_DEADLINE", "10")
        .output()
        .expect("oakum");
    assert!(!out.status.success(), "a stalled drain must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("still held its output open"), "{stderr}");
    assert!(stderr.contains("exit status: 0"), "{stderr}");
    assert!(
        !stderr.contains("killed"),
        "nothing was killed and the report must not claim it: {stderr}"
    );
}

/// An interactive `ProxyCommand` is spawned and waited on even under
/// `BatchMode=yes` (measured), so it is the other prompt source only the
/// deadline covers.
#[cfg(unix)]
#[test]
fn a_blocking_proxy_command_meets_the_deadline() {
    let _turn = deadline_turn();
    let root = tagged_cargo("deadline-proxy", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "ssh://git@host.invalid/demo.git"],
    );

    let started = std::time::Instant::now();
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH_COMMAND", "ssh -o \"ProxyCommand=sleep 60\"")
        .env("OAKUM_REMOTE_DEADLINE", "2")
        .output()
        .expect("oakum");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the deadline must bound the proxy's sleep"
    );
    assert!(!out.status.success(), "an expired deadline must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
}

/// A commits-only repo with a fix on a feature branch, whose `git` runs
/// `on_word` for any call carrying `word`, under the deadline `deadline`;
/// returns the exit code and stderr.
#[cfg(unix)]
fn with_git_shim(
    label: &str,
    word: &str,
    on_word: &str,
    args: &[&str],
    deadline: &str,
    envs: &[(&str, &str)],
) -> (Option<i32>, String) {
    let root = temp_git_repo(label);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = false\nconventional-commits = true\n",
    );
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    let real = ambient_tool("git");
    let on_word = on_word.replace("REAL", &real.display().to_string());
    let shim = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do [ \"$arg\" = {word} ] && {{ {on_word}; }}; done\nexec {real} \"$@\"\n",
            real = real.display()
        ),
    );
    let _turn = deadline_turn();
    run_bounded(
        oakum(&root)
            .args(args)
            .env("PATH", path_prefixed_by(&shim))
            .env("OAKUM_REMOTE_DEADLINE", deadline)
            .env_remove("GITHUB_REPOSITORY")
            .envs(envs.iter().copied()),
        || (),
    )
}

/// The shim `exec`s into the sleep, so the child oakum kills is the one that
/// blocks.
#[cfg(unix)]
fn with_hanging_git(label: &str, word: &str, args: &[&str]) -> (Option<i32>, String) {
    with_git_shim(label, word, "exec sleep 60", args, "2", &[])
}

/// okm-3cbq: a git call that never answers settles nothing, so the run is
/// unverified whichever command made it and whatever the call was for.
#[cfg(unix)]
fn assert_unanswered(code: Option<i32>, stderr: &str, word: &str) {
    assert_eq!(code, Some(2), "`{word}` hung: {stderr}");
    assert!(stderr.contains("unverified: git"), "{stderr}");
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_hung_git_log_leaves_check_unverified() {
    let (code, stderr) = with_hanging_git("hang-check-log", "log", &["check", "--strict"]);
    assert_unanswered(code, &stderr, "log");
    assert!(stderr.contains("unverified: git log"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_hung_tag_listing_leaves_version_unverified() {
    let (code, stderr) = with_hanging_git("hang-version-tags", "for-each-ref", &["version"]);
    assert_unanswered(code, &stderr, "for-each-ref");
}

#[cfg(unix)]
#[test]
fn a_hung_tag_listing_leaves_status_unverified() {
    let (code, stderr) = with_hanging_git("hang-status-tags", "for-each-ref", &["status"]);
    assert_unanswered(code, &stderr, "for-each-ref");
}

/// Commits-only intent reads from the tag, so only the coverage look needs the
/// merge-base, and `status` reports a look that could not run rather than
/// refusing.
#[cfg(unix)]
#[test]
fn a_hung_merge_base_leaves_status_coverage_unchecked() {
    let (code, stderr) = with_hanging_git("hang-status-merge-base", "merge-base", &["status"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("coverage not checked: git merge-base main failed: gave up after 2s"),
        "{stderr}"
    );
}

/// The config probe is every command's first git child; a hang there is no
/// answer, not a git that never ran.
#[cfg(unix)]
#[test]
fn a_hung_config_probe_leaves_version_unverified() {
    let (code, stderr) = with_hanging_git("hang-version-config", "config", &["version"]);
    assert_unanswered(code, &stderr, "config");
    assert!(stderr.contains("could not ask git"), "{stderr}");

    let (code, stderr) = with_hanging_git("hang-check-config", "config", &["check", "--strict"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        !stderr.contains("unverified: unverified:"),
        "one verdict, said once: {stderr}"
    );
}

/// A caller that adds context to a git error keeps its class, so the remote
/// read `ci version-pr` falls back on stays unverified when it never answers.
#[cfg(unix)]
#[test]
fn a_hung_remote_read_leaves_ci_version_pr_unverified() {
    let (code, stderr) = with_git_shim(
        "hang-ci-remote",
        "get-url",
        "exec sleep 60",
        &["ci", "version-pr"],
        "2",
        &[("GITHUB_TOKEN", "token")],
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("unverified: needs GITHUB_REPOSITORY or a git `origin` remote"),
        "{stderr}"
    );
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
    assert_eq!(stderr.matches("unverified: ").count(), 1, "{stderr}");
}

/// A git that exits while something it spawned holds its pipes leaves no
/// answer either. Ten seconds, as in the drain test above: at two the run
/// sometimes reached the kill first.
#[cfg(unix)]
#[test]
fn a_stalled_git_log_leaves_check_unverified() {
    let (code, stderr) = with_git_shim(
        "stall-check-log",
        "log",
        "sleep 15 & exec REAL \"$@\"",
        &["check", "--strict"],
        "10",
        &[],
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("unverified: git log"), "{stderr}");
    assert!(stderr.contains("still held its output open"), "{stderr}");
}
