use super::*;

/// A partial clone's `diff` lazily fetches from the promisor remote, dialing
/// ssh from a child oakum types as local — so the composed transport must
/// ride every git child, not only the remote-classed ones.
#[cfg(unix)]
#[test]
fn a_local_child_carries_the_composed_transport() {
    let root = tagged_cargo("local-transport", &["0.1.0"]);
    let log = sibling(&root, "oakum-local-transport.log");
    let _ = fs::remove_file(&log);
    let real = ambient_tool("git");
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"${{GIT_SSH_COMMAND-}}\" >> {log}\n\
             exec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.display()
        ),
    );

    let out = oakum(&root)
        .arg("check")
        .env("PATH", path_prefixed_by(&shim_dir))
        .env("GIT_SSH_COMMAND", "ssh -i /dev/null")
        .output()
        .expect("oakum");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let spawns = fs::read_to_string(&log).unwrap_or_default();
    let local = spawns
        .lines()
        .filter(|line| {
            let verb = line.split('|').next().unwrap_or_default();
            !matches!(verb, "config" | "ls-remote" | "push")
        })
        .collect::<Vec<_>>();
    assert!(!local.is_empty(), "no local child ran:\n{spawns}");
    for line in local {
        assert!(
            line.ends_with("ssh -i /dev/null -o BatchMode=yes"),
            "a local child ran without the composed transport: {line}\n{spawns}"
        );
    }
}

/// `marker://addr` selects `git-remote-marker` exactly as `marker::addr`
/// does — same helper, same hazard — so the scheme spelling gets the same
/// note the `::` spelling does.
#[cfg(unix)]
#[test]
fn a_helper_named_by_url_scheme_gets_the_note_too() {
    let root = tagged_cargo("remote-scheme-helper", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "marker://addr/demo.git"],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("runs a helper command"),
        "a scheme-selected helper gets the helper note: {stderr}"
    );
    assert!(
        !stderr.contains("does not fetch over ssh") && !stderr.contains("does not push over ssh"),
        "a helper transport must not read as established safe: {stderr}"
    );
}

/// The reach read costs one `remote -v` listing per run, cached for every
/// remote and direction — never a per-remote `get-url` child. Read even when
/// the transport composed, because a helper remote owes its note regardless.
#[cfg(unix)]
#[test]
fn the_reach_read_costs_one_listing_child_per_run() {
    let root = tagged_cargo("remote-composed-lazy", &["0.1.0"]);
    let bare = sibling(&root, "remote-composed-lazy.git");
    let _ = fs::remove_dir_all(&bare);
    git(&root, &["init", "--bare", bare.to_str().expect("utf-8")]);
    git(
        &root,
        &["remote", "add", "origin", bare.to_str().expect("utf-8")],
    );
    git(&root, &["push", "--tags", "origin", "HEAD"]);

    let log = sibling(&root, "oakum-lazy-url.log");
    let _ = fs::remove_file(&log);
    let real = ambient_tool("git");
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\nexec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.display()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path_prefixed_by(&shim_dir))
        // Composable, so `BatchMode` is appended and nothing is ever warned.
        .env("GIT_SSH_COMMAND", "ssh -i /dev/null")
        .output()
        .expect("oakum");

    let argv = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        argv.lines().any(|line| line.starts_with("ls-remote")),
        "the remote was never contacted, so this proves nothing:\n{argv}"
    );
    // The reach is read even under a composed transport — a helper remote
    // owes its note regardless — but it costs one `remote -v` listing for the
    // whole run, never a per-remote `get-url` child.
    assert_eq!(
        argv.lines().filter(|line| *line == "remote -v").count(),
        1,
        "one listing fills the reach cache for every remote:\n{argv}"
    );
    assert!(
        !argv.lines().any(|line| line.starts_with("remote get-url")),
        "no per-remote URL child:\n{argv}"
    );
}

/// The reach is read for the remote in hand, not for a name assumed to be
/// `origin` — measured: that hardcode passed the whole suite, because every
/// other ssh-note test names its remote `origin`. `preferred_remote` takes the
/// only remote there is, so a repository with just `upstream` drives this.
#[cfg(unix)]
#[test]
fn the_note_reads_the_remote_in_hand_not_one_named_origin() {
    let root = tagged_cargo("remote-ssh-upstream", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "upstream",
            "git@example.invalid:demo/demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("\"upstream\""),
        "the note must name the remote it read: {stderr}"
    );
    assert!(
        stderr.contains("A prompt can still block."),
        "the reach was established, so the note must not hedge: {stderr}"
    );
    assert!(
        !stderr.contains("could not establish whether ssh is involved"),
        "the reach was read for a real remote: {stderr}"
    );
}

/// The note is the only signal that oakum's prompt refusal does not apply, and
/// it must not repeat: `release` builds one remote child per tag plus two.
#[cfg(unix)]
#[test]
fn an_opaque_transport_is_reported_once() {
    let root = tagged_cargo("remote-ssh-opaque", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let notes = stderr
        .lines()
        .filter(|line| line.contains("cannot refuse ssh prompts"))
        .count();
    assert_eq!(notes, 1, "expected exactly one note, got: {stderr}");
    assert!(
        stderr.contains("/usr/local/bin/my-ssh"),
        "the note must name the transport: {stderr}"
    );
}

/// Git says "no match" as exit 1 with both streams empty. Any other non-zero is
/// a failure to look, and reading it as absence is the collapse AGENTS.md forbids.
#[cfg(unix)]
#[test]
fn a_diagnosed_config_failure_is_not_read_as_absence() {
    let root = tagged_cargo("shim-config-diagnosed", &["0.1.0"]);
    let dir = git_shim(
        &root,
        "*--get-regexp*tagopt*",
        "echo 'fatal: bad config line 9' >&2; exit 2",
    );
    let (ok, stderr) = with_shim(&root, &dir, &["check"]);
    assert!(!ok, "a config probe that failed must not read as clean");
    assert!(
        stderr.contains("unverified"),
        "a diagnosed failure must be unverified, got: {stderr}"
    );
}

/// A tag listing that exits zero while warning has not reported "no tags"; it
/// has reported that it could not finish looking.
#[cfg(unix)]
#[test]
fn a_warning_on_a_successful_look_is_not_an_empty_answer() {
    let root = tagged_cargo("shim-warned-look", &["0.1.0"]);
    let dir = git_shim(
        &root,
        "*for-each-ref*",
        "echo 'error: refs/tags: unable to read ref database' >&2; exit 0",
    );
    let (ok, stderr) = with_shim(&root, &dir, &["check"]);
    assert!(!ok, "a warned look must not pass as an empty tag list");
    assert!(
        stderr.contains("unverified"),
        "an incomplete look must be unverified, got: {stderr}"
    );
}

/// `GIT_SSH` is the usual Windows transport. Oakum cannot append `-o`.
#[cfg(windows)]
#[test]
fn git_ssh_on_windows_is_unprotected_and_is_not_given_dash_o() {
    let root = tagged_cargo("remote-git-ssh-windows", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let bat = root.join("fake-ssh.cmd");
    fs::write(
        &bat,
        format!(
            "@echo off\r\necho %*>>\"{}\"\r\nexit /b 255\r\n",
            log.display()
        ),
    )
    .expect("fake ssh");

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH", &bat)
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "GIT_SSH must be reported as unprotected: {stderr}"
    );
    assert!(
        stderr.contains("GIT_SSH names"),
        "the note must name GIT_SSH: {stderr}"
    );
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.is_empty(),
        "git must have invoked GIT_SSH; log empty, stderr: {stderr}"
    );
    assert!(
        !recorded.to_ascii_lowercase().contains("batchmode"),
        "oakum must not inject -o into GIT_SSH: {recorded}"
    );
}

#[cfg(windows)]
#[test]
fn a_plink_path_in_git_ssh_is_named_in_the_note() {
    let root = tagged_cargo("remote-plink-windows", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH", r"C:\PuTTY\plink.exe")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "a plink GIT_SSH must be reported: {stderr}"
    );
    assert!(
        stderr.contains(r"C:\PuTTY\plink.exe"),
        "the note must name the transport: {stderr}"
    );
}

#[test]
fn an_explicit_ssh_variant_is_unprotected() {
    let root = tagged_cargo("remote-variant-simple", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    git(&root, &["config", "ssh.variant", "simple"]);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_SSH_VARIANT")
        .env_remove("GIT_SSH")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "ssh.variant must be reported as unprotected: {stderr}"
    );
    assert!(
        stderr.contains("ssh.variant is `simple`"),
        "the note must name the variant: {stderr}"
    );
}
