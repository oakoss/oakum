use super::*;

#[test]
fn remote_off_by_default_when_local_tags_are_missing_from_the_remote() {
    let origin = tagged_cargo("remote-default-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-default-dest");
    git(&origin, &["tag", "-d", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&dest);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(!ok, "missing remote tag must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.0"), "{stderr}");
}

#[test]
fn remote_reports_unverified_when_advertised_tags_are_missing_locally() {
    let origin = tagged_cargo("remote-missing-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-missing-dest");
    git(&dest, &["tag", "-d", "v0.1.0"]);
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(
        !ok,
        "missing advertised tags must be unverified, got: {stdout}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("git fetch --tags"), "{stderr}");
}

#[test]
fn remote_ok_when_advertised_tags_match_local() {
    let origin = tagged_cargo("remote-match-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-match-dest");
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn remote_fails_closed_without_a_remote() {
    let root = tagged_cargo("remote-none", &["0.1.0"]);
    let (ok, stdout, stderr) = checked(&root, &["check", "--remote"]);
    assert!(!ok, "no remotes must be unverified, got: {stdout}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no remotes"), "{stderr}");
}

#[test]
fn remote_lookback_ignores_older_missing_tags() {
    let origin = tagged_cargo(
        "remote-lookback-origin",
        &["0.1.0", "0.2.0", "0.3.0", "0.4.0"],
    );
    git(&origin, &["tag", "-d", "v0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-dest");
    git(&dest, &["tag", "v0.1.0", "HEAD~3"]);
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(
        ok,
        "default lookback of 3 should skip v0.1.0, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote", "--remote-lookback", "4"]);
    assert!(!ok, "lookback 4 must see missing v0.1.0, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.0"), "{stderr}");
    assert!(stderr.contains("git push --tags"), "{stderr}");
}

#[test]
fn remote_lookback_orders_by_semver_not_lexically() {
    let origin = tagged_cargo(
        "remote-semver-origin",
        &["0.9.0", "0.10.0", "0.11.0", "0.12.0"],
    );
    git(&origin, &["tag", "-d", "v0.9.0"]);
    let dest = clone_of(&origin, "remote-semver-dest");
    git(&dest, &["tag", "v0.9.0", "HEAD~3"]);
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(ok, "semver lookback of 3 should skip v0.9.0, got: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn remote_lookback_zero_is_rejected_by_clap() {
    let origin = tagged_cargo("remote-lookback-zero-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-zero-dest");
    let (ok, stdout, stderr) =
        refused_before_the_report(&dest, &["check", "--remote", "--remote-lookback", "0"]);
    assert!(!ok, "lookback 0 must be a clap error, got: {stdout}");
    assert!(
        !stderr.contains("unverified"),
        "range rejection is not a verification outcome: {stderr}"
    );
}

#[test]
fn remote_lookback_without_remote_is_rejected_by_clap() {
    let origin = tagged_cargo("remote-lookback-requires-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-requires-dest");
    let (ok, stdout, stderr) =
        refused_before_the_report(&dest, &["check", "--remote-lookback", "4"]);
    assert!(
        !ok,
        "--remote-lookback without --remote must be clap, got: {stdout}"
    );
    assert!(
        !stderr.contains("unverified"),
        "missing --remote is not a verification outcome: {stderr}"
    );
}

#[test]
fn remote_ls_remote_failure_is_unverified_when_local_tags_are_empty() {
    let origin = tagged_cargo("remote-ls-fail-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-ls-fail-dest");
    git(&dest, &["tag", "-d", "v0.1.0"]);
    git(
        &dest,
        &["remote", "set-url", "origin", "/no/such/oakum-remote"],
    );
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(!ok, "failed ls-remote must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("ls-remote"), "{stderr}");
}

#[test]
fn remote_ls_remote_failure_is_unverified_when_local_tags_exist() {
    let origin = tagged_cargo("remote-ls-fail-tagged-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-ls-fail-tagged-dest");
    git(
        &dest,
        &["remote", "set-url", "origin", "/no/such/oakum-remote"],
    );
    let (ok, stdout, stderr) = checked(&dest, &["check", "--remote"]);
    assert!(!ok, "failed ls-remote must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("ls-remote"), "{stderr}");
    assert!(
        !stderr.contains("git push --tags"),
        "must not name push when the look failed: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn remote_read_over_ssh_refuses_to_prompt() {
    let root = tagged_cargo("remote-ssh-batchmode", &["0.1.0"]);
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
    let script = recording_fake_ssh(&root, &log);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH_COMMAND", script.to_str().expect("utf-8"))
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("-o\nBatchMode=yes\n"),
        "git did not pass BatchMode to ssh; recorded: {recorded:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an unreachable remote must not pass");
    assert!(
        stderr.contains("unverified"),
        "a remote read that failed is unverified, got: {stderr}"
    );
}

/// A user's own ssh command is composed with, not replaced: the fake ssh still
/// runs, and still receives `BatchMode`.
#[cfg(unix)]
#[test]
fn a_user_ssh_command_keeps_its_own_arguments() {
    let root = tagged_cargo("remote-ssh-compose", &["0.1.0"]);
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
    let script = recording_fake_ssh(&root, &log);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "GIT_SSH_COMMAND",
            format!("{} -i /dev/null", script.to_str().expect("utf-8")),
        )
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("-i\n/dev/null\n") && recorded.contains("-o\nBatchMode=yes\n"),
        "user arguments must survive alongside BatchMode; recorded: {recorded:?}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
}

/// `core.sshCommand` is tier two of git's precedence and no test reached it
/// while every fixture set `GIT_SSH_COMMAND`, which short-circuits the read.
#[cfg(unix)]
#[test]
fn a_config_ssh_command_is_composed_with_not_replaced() {
    let root = tagged_cargo("remote-ssh-config", &["0.1.0"]);
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
    let script = recording_fake_ssh(&root, &log);
    git(
        &root,
        &["config", "core.sshCommand", script.to_str().expect("utf-8")],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.is_empty(),
        "the user's core.sshCommand was replaced, not composed with; recorded nothing"
    );
    assert!(
        recorded.contains("-o\nBatchMode=yes\n"),
        "core.sshCommand did not receive BatchMode; recorded: {recorded:?}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
    // A transport oakum could compose has nothing to warn about.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("cannot refuse ssh prompts"),
        "a composed transport needs no note: {stderr}"
    );
}

/// The sandboxed `GIT_CONFIG_GLOBAL` preserves the global tier production
/// reads; a local `core.sshCommand` alone would not prove that tier.
#[cfg(unix)]
#[test]
fn a_global_config_ssh_command_is_composed_with_not_replaced() {
    let root = tagged_cargo("remote-ssh-global", &["0.1.0"]);
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
    let script = recording_fake_ssh(&root, &log);
    git(
        &root,
        &[
            "config",
            "--global",
            "core.sshCommand",
            script.to_str().expect("utf-8"),
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.is_empty(),
        "the user's global core.sshCommand was replaced, not composed with; recorded nothing"
    );
    assert!(
        recorded.contains("-o\nBatchMode=yes\n"),
        "global core.sshCommand did not receive BatchMode; recorded: {recorded:?}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("cannot refuse ssh prompts"),
        "a composed transport needs no note: {stderr}"
    );
}

/// The measured failure: setting `GIT_SSH_COMMAND` alone used to leave the
/// unreadable-config error in place because the probe ran first. With both
/// env vars set the probe is skipped, so the remedy the message now
/// prescribes actually clears the condition.
#[cfg(unix)]
#[test]
fn an_unreadable_config_is_skipped_when_both_ssh_env_vars_are_set() {
    let root = tagged_cargo("remote-ssh-env-outranks", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let real = ambient_tool("git");
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) ;; *) exec {real} \"$@\" ;; esac\nif [ \"$1\" = config ] && [ \"$2\" = --get-regexp ]; then\n\
             echo 'fatal: unable to read config file: Permission denied' >&2\n exit 128\nfi\nexec {real} \"$@\"\n",
            real = real.display()
        ),
    );

    let path = path_prefixed_by(&shim_dir);
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path)
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=1",
        )
        .env("GIT_SSH_VARIANT", "ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("ssh configuration oakum could not read"),
        "both SSH env vars must skip the unreadable config probe; got: {stderr}"
    );
}

/// A repository git will not open is not an ssh problem. Measured before the
/// fix: the transport probe failed, and `git rev-parse --is-shallow-repository`
/// — a local operation — reported `needs an ssh configuration oakum could not
/// read`, buried the real `found 99` in parentheses, and offered a remedy that
/// was measured to move the failure rather than fix it (setting both variables
/// skips the probe and the child then fails on its own).
#[test]
fn an_unreadable_repository_is_not_reported_as_an_ssh_problem() {
    let root = tagged_cargo("repo-format-version", &["0.1.0"]);
    let config = root.join(".git/config");
    let text = fs::read_to_string(&config).expect("git config");
    fs::write(
        &config,
        text.replace(
            "repositoryformatversion = 0",
            "repositoryformatversion = 99",
        ),
    )
    .expect("rewrite git config");

    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "an unreadable repository must not pass");
    assert!(
        stderr.contains("could not read this repository"),
        "the refusal must name what actually failed: {stderr}"
    );
    assert!(
        stderr.contains("found 99"),
        "the refusal must carry git's own reason: {stderr}"
    );
    assert!(
        !stderr.contains("ssh"),
        "a non-ssh failure must not be dressed as one: {stderr}"
    );
}

/// The config probe ran and refused; the repository question could not be put.
/// Reporting that as `Unasked` said git was never asked about the very thing it
/// answered, and threw away the second probe's reason. Driven through the real
/// classifier, because a test that builds the variant by hand passes whatever
/// the mapping does — which is how the bug survived its own fix.
#[cfg(unix)]
#[test]
fn a_refused_config_probe_with_an_unaskable_repository_keeps_both_facts() {
    let root = tagged_cargo("probe-both-facts", &["0.1.0"]);
    // Both probes must misbehave, and differently: the config probe exits with
    // a reason, the repository probe dies before it can answer. A shim that
    // only breaks the first takes the `Ok(())` path and never reaches this arm.
    let dir = git_shim(
        &root,
        "*sshcommand*|*--git-dir*",
        "case \"$*\" in *sshcommand*) echo 'fatal: unable to read config file: Permission denied' >&2; exit 128 ;; *) kill -TERM $$ ;; esac",
    );
    let (ok, stderr) = with_shim(&root, &dir, &["check"]);
    assert!(!ok, "an unreadable config must refuse: {stderr}");
    assert!(
        stderr.contains("unable to read config file"),
        "the config probe's own reason survives: {stderr}"
    );
    assert!(
        stderr.contains("could not be established either"),
        "the second probe's failure is not swallowed: {stderr}"
    );
    assert!(
        !stderr.contains("did not run"),
        "a probe that answered is not reported as never asked: {stderr}"
    );
}

/// The tag look and the coverage look both run `rev-parse
/// --is-shallow-repository`, so one broken git yields two byte-identical
/// refusals — and `also` would read as a second, different problem.
#[cfg(unix)]
#[test]
fn one_failure_reaching_two_looks_is_said_once() {
    let root = tagged_cargo("dedup-identical", &["0.1.0"]);
    let dir = git_shim(
        &root,
        "*--is-shallow-repository*",
        "echo 'fatal: SHALLOW-PROBE-REFUSED' >&2; exit 128",
    );
    let (ok, stderr) = with_shim(&root, &dir, &["check"]);
    assert!(!ok, "{stderr}");
    assert_eq!(
        stderr.matches("SHALLOW-PROBE-REFUSED").count(),
        1,
        "one failure, said once: {stderr}"
    );
}

/// One env var is not enough: variant still falls back to config, so the probe
/// still runs and an unreadable config must still surface.
#[cfg(unix)]
#[test]
fn an_unreadable_config_still_fails_when_only_git_ssh_command_is_set() {
    let root = tagged_cargo("remote-ssh-env-command-only", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let real = ambient_tool("git");
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) ;; *) exec {real} \"$@\" ;; esac\nif [ \"$1\" = config ] && [ \"$2\" = --get-regexp ]; then\n\
             echo 'fatal: unable to read config file: Permission denied' >&2\n exit 128\nfi\nexec {real} \"$@\"\n",
            real = real.display()
        ),
    );

    let path = path_prefixed_by(&shim_dir);
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path)
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=1",
        )
        .env_remove("GIT_SSH_VARIANT")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "command alone must not skip the probe"
    );
    assert!(
        stderr.contains("unverified") && stderr.contains("ssh configuration"),
        "command alone must still report the unreadable probe; got: {stderr}"
    );
}

/// A signal leaves both streams empty, and the probe rendered its error from
/// stderr alone: the message became `could not read the ssh configuration ()`.
/// The `Op` path already said `terminated by a signal`; this is the same
/// rendering, shared rather than re-derived.
#[cfg(unix)]
#[test]
fn a_signalled_config_probe_names_the_signal() {
    let root = tagged_cargo("remote-ssh-signalled", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let real = ambient_tool("git");
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            // Only the ssh probe: `Op::RemoteTagConfig` runs `config --get-regexp`
            // too, and killing that one produces the same phrase from the other
            // code path, which would let this test pass for the wrong reason.
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) kill -TERM $$ ;; esac\nexec {real} \"$@\"\n",
            real = real.display()
        ),
    );

    let path = path_prefixed_by(&shim_dir);
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path)
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_SSH_VARIANT")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a killed probe must not pass");
    assert!(
        stderr.contains("terminated by a signal"),
        "a signalled probe must say so rather than render an empty detail; got: {stderr}"
    );
}

/// Git's trace channels write to stderr, and the probe reads a non-empty stderr
/// as a failure. Inherited from the caller's shell, an exported `GIT_TRACE`
/// stopped every remote operation with a message blaming the ssh configuration.
#[cfg(unix)]
#[test]
fn an_inherited_trace_is_not_read_as_a_broken_ssh_config() {
    let root = tagged_cargo("remote-traced", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    for trace in ["GIT_TRACE", "GIT_TRACE_PACKET"] {
        let out = oakum(&root)
            .args(["check", "--remote"])
            .env(trace, "1")
            .env_remove("GIT_SSH_COMMAND")
            .output()
            .expect("oakum");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("ssh configuration"),
            "{trace} must not read as a broken ssh config; got: {stderr}"
        );
        assert!(
            !stderr.contains("trace:"),
            "{trace} must not reach oakum's diagnostics; got: {stderr}"
        );
    }
}

/// `GIT_TERMINAL_PROMPT=0` does not reach git's askpass chain: with prompts
/// disabled, git still runs an askpass helper for an https credential, and a
/// GUI helper blocks forever. Editors export `GIT_ASKPASS` routinely.
#[cfg(unix)]
#[test]
fn an_https_remote_does_not_reach_the_askpass_helper() {
    let root = tagged_cargo("remote-askpass", &["0.1.0"]);
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

    let log = sibling(&root, "askpass-calls.log");
    let _ = fs::remove_file(&log);
    let script = sibling(&root, "fake-askpass");
    install_executable(
        &script,
        format!(
            "#!/bin/sh\nprintf 'ASKPASS %s\\n' \"$*\" >> {}\necho hunter2\n",
            log.display()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_ASKPASS", script.to_str().expect("utf-8"))
        .env("GIT_TERMINAL_PROMPT", "1")
        .output()
        .expect("oakum");

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.is_empty(),
        "git reached the askpass helper, which can block indefinitely; calls: {calls:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a 401 remote must not pass");
    assert!(stderr.contains("unverified"), "got: {stderr}");
}

/// oakum disables git's prompt chain on every git child, so the credential
/// failure that produces is a state oakum caused: the report must say so and
/// name the remedy, with git's own text kept as the evidence.
#[test]
fn a_credential_starved_remote_read_names_the_fix() {
    let root = tagged_cargo("remote-starved", &["0.1.0"]);
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
    let out = oakum(&root)
        .args(["check", "--remote"])
        .output()
        .expect("oakum");
    assert!(!out.status.success(), "a 401 remote must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("failed:"), "{stderr}");
    assert!(stderr.contains("credential helper"), "{stderr}");
    assert!(stderr.contains("gh auth setup-git"), "{stderr}");
}

/// The transport already chose `BatchMode`, and ssh takes the first value of a
/// repeated option — so oakum's append is inert and a prompt can still block.
/// The note is the only signal that happens, and it has to carry why while the
/// user's own ssh command still runs with its own arguments.
#[cfg(unix)]
#[test]
fn an_inert_batch_mode_still_runs_the_user_ssh_and_says_why() {
    let root = tagged_cargo("remote-ssh-inert", &["0.1.0"]);
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
    let script = recording_fake_ssh(&root, &log);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "GIT_SSH_COMMAND",
            format!("{} -o BatchMode=no", script.display()),
        )
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "an inert append must still be reported: {stderr}"
    );
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("BatchMode=no"),
        "the user's own ssh arguments must still reach ssh: {recorded}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
}

/// The refusal comes before any remote child. A transport oakum could not read
/// must never reach `ls-remote` without `BatchMode`, which is the prompt hang
/// okm-6mz fixed — and a refusal issued after the spawn would look identical
/// from stderr.
#[cfg(unix)]
#[test]
fn a_transport_failure_with_an_unreadable_url_refuses_before_any_remote_child() {
    let root = tagged_cargo("remote-both-probes-fail", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = sibling(&root, "oakum-both-probes.log");
    let _ = fs::remove_file(&log);
    let real = ambient_tool("git");
    // Both probes fail; everything else, including the argv log, passes through.
    let shim_dir = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\n\
             case \"$*\" in\n\
             *sshcommand*) echo 'fatal: unable to read config file' >&2; exit 128 ;;\n\
             esac\nexec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.display()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path_prefixed_by(&shim_dir))
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "an unreadable ssh config must refuse"
    );
    assert!(
        stderr.contains("ssh configuration oakum could not read"),
        "the refusal must name the cause: {stderr}"
    );
    let argv = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !argv.lines().any(|line| line.starts_with("ls-remote")),
        "no remote child may run without a resolved transport:\n{argv}"
    );
}
