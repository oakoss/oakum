use super::*;

/// The defect: `check` exited 0 with no output at all, so a run that examined
/// a workspace and a run that examined nothing were the same observation. These
/// assert on the raw stdout rather than through [`checked`], which strips the
/// very lines under test.
#[test]
fn a_clean_run_says_what_it_examined_and_from_which_ref() {
    let root = temp_git_repo("scope-clean");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (ok, stdout, stderr) = oakum_output(&root, &["check"]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("check: 1 of 1 package(s) selected"),
        "{stdout}"
    );
    // The exact ref, not the prefix: naming a different one survived the whole
    // suite, and being trustworthy about what was examined is the line's job.
    assert!(
        stdout.contains(&format!("diffing `{}...HEAD`", head_sha(&root))),
        "the report must name the ref it used: {stdout}"
    );
    assert!(
        stdout.contains(
            "check: looking at management, tags, install pin, schema, changelogs, staging, notes, and coverage"
        ),
        "{stdout}"
    );
    // The third outcome, in prose: a look nobody asked for must not read as a
    // look that passed.
    assert!(
        stdout.contains("not looking at the remote"),
        "an unasked look must say so: {stdout}"
    );
    assert!(
        stdout.contains("coverage and notes report without gating"),
        "a non-gating look must say so: {stdout}"
    );
}

/// The report precedes every look, so the run that refuses still says what it
/// examined — otherwise the only runs that describe themselves are the ones a
/// reader already has a message for.
#[test]
fn a_refusing_run_still_says_what_it_examined() {
    let root = temp_git_repo("scope-refusing");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (ok, stdout, stderr) = oakum_output(&root, &["check"]);
    assert!(!ok, "a manifest above its tag is drift: {stdout}");
    assert!(
        stdout.contains("check: 1 of 1 package(s) selected"),
        "{stdout}"
    );
    assert!(!stderr.is_empty(), "the refusal still lands on stderr");
}

/// `--strict` makes coverage a gate and `--remote` adds a look, and the report
/// tracks both rather than describing a fixed set of looks. Both flags are
/// exercised: the sentence named the remote look twice for a while, and this
/// test asserted only `--strict`, so nothing saw it.
#[test]
fn the_report_tracks_which_looks_were_asked_for() {
    let root = temp_git_repo("scope-flags");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (_, strict, _) = oakum_output(&root, &["check", "--strict"]);
    assert!(
        !strict.contains("report without gating"),
        "under --strict coverage gates: {strict}"
    );
    assert!(strict.contains("not looking at the remote"), "{strict}");

    // Asked for, so its own clause says so — once. The list carries the
    // looks that always run; naming it in both places said it twice.
    let (_, remote, _) = oakum_output(&root, &["check", "--remote"]);
    assert!(remote.contains("and coverage; "), "{remote}");
    assert!(
        remote.contains("; looking at the remote\ncoverage: "),
        "{remote}"
    );
    assert_eq!(remote.matches("remote").count(), 1, "named twice: {remote}");
}

/// The compounding case the finding names: a config that selects nothing used
/// to exit 0 in silence, which reads exactly like a clean repository.
#[test]
fn a_selection_that_keeps_no_package_says_so_in_the_count() {
    let root = temp_git_repo("scope-empty-selection");
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (_, stdout, _) = oakum_output(&root, &["check"]);
    assert!(
        stdout.contains("check: 0 of 1 package(s) selected"),
        "{stdout}"
    );
}

/// An explicit `--from` is named as given, so the announced ref and the diff
/// are the same ref.
#[test]
fn an_explicit_from_is_the_ref_the_report_names() {
    let root = temp_git_repo("scope-explicit-from");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (_, stdout, _) = oakum_output(&root, &["check", "--from", "v0.1.0"]);
    assert!(stdout.contains("diffing `v0.1.0...HEAD`"), "{stdout}");
}

/// No `origin/main`, no `main`, no `master`, no `--from`: there is no base to
/// diff from, and the arm that says so must say it. Replacing that arm with a
/// ref it does not have survived the whole suite — the one arm whose job is to
/// stop oakum claiming a ref, unobserved.
#[test]
fn no_base_ref_is_said_rather_than_invented() {
    let root = temp_git_repo("scope-no-base");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["branch", "-m", "somewhere-else"]);

    let (_, stdout, _) = oakum_output(&root, &["check"]);
    assert!(stdout.contains("no base ref to diff from"), "{stdout}");
    assert!(!stdout.contains("diffing `"), "{stdout}");
    // Both base-reading looks stay in the announced set and say why they
    // cannot run: they are the looks that then raise, so dropping them left
    // the refusal unexplained.
    assert!(
        stdout.contains("coverage and notes cannot run without a base ref"),
        "{stdout}"
    );
}

/// A ref git does not have is reported as that, rather than quoted and then
/// failed on. Measured before: `--from no-such-ref` announced the ref and the
/// diff died with `fatal: ambiguous argument`.
#[test]
fn an_unresolvable_explicit_from_is_not_announced_as_a_base() {
    let root = temp_git_repo("scope-bad-from");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (ok, stdout, _) = oakum_output(&root, &["check", "--from", "no-such-ref-xyz"]);
    assert!(!ok, "an unresolvable base must not pass: {stdout}");
    assert!(stdout.contains("no base ref to diff from"), "{stdout}");
    assert!(stdout.contains("no-such-ref-xyz"), "{stdout}");
    assert!(
        !stdout.contains("diffing `no-such-ref-xyz"),
        "an unconfirmed ref must not be announced as the base: {stdout}"
    );
    // A base git does not have is a look that could not happen, not a check
    // that failed — ADR-0034's whole point, and a shallow CI checkout is the
    // realistic way to hit it.
    let (code, _, stderr) = oakum_exit(&root, &["check", "--from", "no-such-ref-xyz"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("unverified: git has no"), "{stderr}");
}

/// ADR-0034 exists so a caller can tell "we measured a failure" from "we could
/// not look". A run carrying several refusals undoes that unless the finding is
/// the one it carries out — measured before this: adding an unrelated stray
/// staging file to a repository with real tag drift changed exit 1 to exit 2
/// and erased the `error:` line from stderr entirely.
#[test]
fn a_finding_outranks_an_unverified_look_for_the_exit_code() {
    let root = temp_git_repo("severity-order");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, _, drift_only) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(1), "drift alone is a finding: {drift_only}");
    assert!(
        drift_only.contains("error: 1 package(s) bumped without a tag"),
        "{drift_only}"
    );

    fs::write(root.join(".changeset/.foo.md.oakum-write.1.2.3"), "").expect("staging file");
    let (code, _, both) = oakum_exit(&root, &["check"]);
    assert_eq!(
        code,
        Some(1),
        "an unrelated unverified look must not reclass it: {both}"
    );
    assert!(
        both.contains("error: 1 package(s) bumped without a tag"),
        "the finding still decides: {both}"
    );
    assert!(
        both.contains("also unverified: 1 oakum staging file(s) left behind"),
        "a shadowed refusal is reported and marked: {both}"
    );
}

#[test]
fn tags_for_two_versions_on_one_commit_name_the_fix_below_the_summary() {
    let root = temp_git_repo("two-versions-one-commit");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "2.0.0-rc.1");
    commit(&root, "rc");
    git(&root, &["tag", "v2.0.0-rc.1"]);
    git(&root, &["tag", "v2.0.0"]);

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains(
            "keep the tag for the version that commit's manifest names and delete the other"
        ),
        "{stderr}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let tags = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "tags")
        .expect("tags row")
        .clone();
    assert_eq!(
        tags["refusals"][0]["summary"], "leftover tag ambiguity: v2.0.0, v2.0.0-rc.1",
        "the fix stays off the summary line: {document}"
    );
    assert!(
        tags["refusals"][0]["detail"][0]
            .as_str()
            .is_some_and(|line| line.starts_with("one commit carries tags")),
        "the fix is the detail, unindented: {document}"
    );
}

/// The mixed run ADR-0035 could not serve: the exit code carries the finding,
/// and the shadowed unverified look — which survives only as `also` prose on
/// stderr — reaches a caller as data. Reading the document must not require
/// knowing which look lost.
#[test]
fn the_document_carries_the_shadowed_look_the_exit_code_cannot() {
    let root = temp_git_repo("json-mixed-run");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join(".changeset/.foo.md.oakum-write.1.2.3"), "").expect("staging file");

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(1), "the finding still decides: {stderr}");

    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    assert_eq!(parsed["outcome"], "error", "{document}");
    assert_eq!(parsed["schema_version"], 1, "{document}");

    let looks = parsed["looks"].as_array().expect("looks");
    let of = |name: &str| {
        looks
            .iter()
            .find(|row| row["look"] == name)
            .unwrap_or_else(|| panic!("no `{name}` row: {document}"))
            .clone()
    };
    let tags = of("tags");
    assert_eq!(tags["outcome"], "error", "the deciding look: {document}");
    assert_eq!(tags["refusals"][0]["deciding"], true, "{document}");
    assert_eq!(
        tags["refusals"][0]["summary"], "1 package(s) bumped without a tag",
        "the document states what drifted, not merely that something did: {document}"
    );
    assert!(
        tags["refusals"][0]["detail"][0]
            .as_str()
            .expect("detail")
            .contains("manifest 0.2.0 is above tagged 0.1.0"),
        "detail is the evidence, not a repeat of the summary: {document}"
    );

    let staging = of("staging");
    assert_eq!(
        staging["outcome"], "unverified",
        "the shadowed look reaches the document: {document}"
    );
    // One run, two different words: a refusal carries its own class, not the
    // run's. Pinning only the row leaves the per-refusal field forgeable.
    assert_eq!(tags["refusals"][0]["outcome"], "error", "{document}");
    assert_eq!(
        staging["refusals"][0]["outcome"], "unverified",
        "{document}"
    );
    assert_eq!(
        staging["refusals"][0]["deciding"], false,
        "shadowed, not deciding: {document}"
    );
}

/// Every look the command can perform appears, so an absent refusal reads as
/// "looked and found nothing" rather than "nobody looked" — and a look nobody
/// asked for is neither. Derived from the plan, so a seventh look cannot reach
/// the prose report and miss the document.
#[test]
fn the_document_names_every_look_including_the_ones_not_asked_for() {
    let root = temp_git_repo("json-every-look");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(0), "a clean repository: {stderr}");

    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let named: Vec<String> = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .map(|row| row["look"].as_str().expect("a name").to_owned())
        .collect();
    assert_eq!(
        named,
        [
            "management",
            "tags",
            "install pin",
            "schema",
            "changelogs",
            "staging",
            "notes",
            "coverage",
            "remote"
        ],
        "{document}"
    );
    // Without the scope a document of clean rows cannot distinguish a run that
    // examined one package from one that examined none.
    assert_eq!(parsed["scope"]["selected"], 1, "{document}");
    assert_eq!(parsed["scope"]["packages"], 1, "{document}");
    // Named rather than answered yes or no: `--strict` decides two looks, and
    // a consumer that can only ask about coverage cannot tell which refused.
    assert_eq!(
        parsed["scope"]["gating"],
        serde_json::json!([]),
        "{document}"
    );
    assert!(
        parsed["scope"]["base"].is_string(),
        "the base the coverage verdict is attributable to: {document}"
    );

    let rows = parsed["looks"].as_array().expect("looks");
    for row in rows.iter().filter(|row| row["look"] != "remote") {
        assert_eq!(
            row["outcome"], "ok",
            "a look that ran and found nothing is `ok`: {document}"
        );
    }
    let remote = rows
        .iter()
        .find(|row| row["look"] == "remote")
        .expect("a remote row");
    assert_eq!(
        remote["outcome"], "not-asked",
        "a look nobody asked for is not `ok`: {document}"
    );
}

/// Each look is one block: its summary, then its detail beneath. The deciding
/// block comes first, the rest marked `also` — a reader meets the verdict
/// before what is subordinate to it, and evidence sits under the line it
/// supports. Measured before this shape: the management guidance was line 1
/// and its summary line 5, with three unrelated lines between; and `also`
/// printed before the line it was also-to.
#[test]
fn the_verdict_leads_and_each_look_is_one_block() {
    let root = temp_git_repo("blocks");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join(".changeset/.foo.md.oakum-write.1.2.3"), "").expect("staging file");
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(1), "{stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines[0], "error: 1 package(s) bumped without a tag",
        "the deciding line leads: {stderr}"
    );
    assert!(
        lines[1].starts_with("  demo (cargo): manifest 0.2.0 is above tagged 0.1.0"),
        "its detail sits beneath it, indented: {stderr}"
    );
    assert_eq!(
        lines[2], "also unverified: 1 oakum staging file(s) left behind",
        "the shadowed refusal follows, marked: {stderr}"
    );
    assert!(
        lines[3].starts_with("  `.changeset/.foo.md.oakum-write.1.2.3` is"),
        "with its own detail beneath it: {stderr}"
    );
    assert_eq!(lines.len(), 4, "{stderr}");
}

/// Among refusals of one class the announced order decides, and tags are
/// announced before the install pin: a git that cannot run at all is the
/// first line a reader meets, not a pin string that differs. Measured
/// before the split: the pin was verified inside the tag look, before git was
/// touched, so `install pin is 9.9.9` decided for a repository where git did
/// not work.
#[cfg(unix)]
#[test]
fn a_git_that_cannot_run_outranks_a_stale_install_pin() {
    let root = temp_git_repo("dead-git-and-pin");
    write_config(&root, "");
    write_install_pin(&root, BINARY_VERSION);
    write_stale_pin(&root);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let shim_dir = path_shim(&root, "git", "#!/bin/sh\necho 'fatal: git is not working today' >&2\necho 'hint: a second line' >&2\nexit 128\n",
    );
    let path = path_prefixed_by(&shim_dir);
    let out = oakum(&root)
        .args(["check"])
        .env("PATH", &path)
        .output()
        .expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    let first = stderr.lines().next().unwrap_or_default();
    assert!(
        first.starts_with("unverified: ") && first.contains("git is not working today"),
        "the dead git decides: {stderr}"
    );
    assert_eq!(
        stderr.matches("\n  hint: a second line").count(),
        2,
        "git's second line is detail under each summary it belongs to — the deciding one and the coverage look's `also`: {stderr}"
    );
    assert!(
        stderr.contains("also unverified: install pins disagree"),
        "the stale pin is still reported, subordinate: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().count(),
        2,
        "the scope report quotes one line of why: {stdout}"
    );
}

/// A look that answers while a sibling refuses must still be read. Collapsing
/// the tag, coverage and remote looks into one `Result` discarded a tag
/// evaluation that had answered, so the drift refusal was never constructed —
/// measured: real drift plus an unresolvable `--from` printed neither the drift
/// detail nor its summary and exited 2.
#[test]
fn a_sibling_refusal_does_not_discard_a_look_that_answered() {
    let root = temp_git_repo("sibling-refusal");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    for refusing in [
        vec!["check", "--from", "no-such-ref-xyz"],
        vec!["check", "--remote"],
    ] {
        let (code, _, stderr) = oakum_exit(&root, &refusing);
        assert_eq!(code, Some(1), "{refusing:?}: {stderr}");
        assert!(
            stderr.contains("error: 1 package(s) bumped without a tag"),
            "{refusing:?}: the drift still decides: {stderr}"
        );
        assert!(
            stderr.contains("also unverified:"),
            "{refusing:?}: the sibling refusal is still reported: {stderr}"
        );
    }
}

/// The finding is at index 1 and the unverified look at index 4. A test that
/// places the finding *last* cannot tell "carry the highest-severity refusal"
/// from "carry the last refusal"; this one places it first, so a `carry`
/// returning the last refusal fails it. It is also the common CI shape: an
/// uncovered change alongside an unrelated leftover.
#[test]
fn a_finding_before_an_unverified_look_in_source_order_still_decides() {
    let root = temp_git_repo("severity-order-reversed");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    fs::write(root.join(".changeset/.foo.md.oakum-write.1.2.3"), "").expect("staging file");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(
        code,
        Some(1),
        "the finding decides, not the later look: {stderr}"
    );
    assert!(
        stderr.contains("error: 1 package(s) changed with no covering intent"),
        "{stderr}"
    );
    assert!(
        stderr.contains("also unverified: 1 oakum staging file(s) left behind"),
        "{stderr}"
    );
}

/// A manifest written through a link stages beside the file it names, so the
/// staging look sweeps that directory too.
#[cfg(unix)]
#[test]
fn a_staging_file_beside_a_link_target_is_named() {
    let root = temp_git_repo("staging-link-target");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::create_dir_all(root.join("shared")).expect("shared");
    fs::write(
        root.join("shared/demo.json"),
        "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n",
    )
    .expect("manifest");
    std::os::unix::fs::symlink("shared/demo.json", root.join("package.json")).expect("link");
    commit(&root, "init");
    fs::write(root.join("shared/.demo.json.oakum-write.1.2.3"), "").expect("staging file");

    let (_, _, stderr) = oakum_exit(&root, &["check"]);
    assert!(
        stderr.contains("`shared/.demo.json.oakum-write.1.2.3`"),
        "{stderr}"
    );
}

/// The workspace-wide files are found where discovery puts them, so a lock
/// linked from a nested cargo workspace has its file's directory swept too.
#[cfg(unix)]
#[test]
fn a_staging_file_beside_a_nested_lock_target_is_named() {
    let root = temp_git_repo("staging-nested-lock-target");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::create_dir_all(root.join("rust/a/src")).expect("crate");
    fs::create_dir_all(root.join("shared")).expect("shared");
    fs::write(
        root.join("rust/Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"a\"]\n",
    )
    .expect("workspace");
    fs::write(
        root.join("rust/a/Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("manifest");
    fs::write(root.join("rust/a/src/lib.rs"), "").expect("lib");
    fs::write(
        root.join("shared/Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\n",
    )
    .expect("lock");
    std::os::unix::fs::symlink("../shared/Cargo.lock", root.join("rust/Cargo.lock")).expect("link");
    commit(&root, "init");
    fs::write(root.join("shared/.Cargo.lock.oakum-write.1.2.3"), "").expect("staging file");

    let (_, _, stderr) = oakum_exit(&root.join("rust"), &["check"]);
    assert!(
        stderr.contains("`shared/.Cargo.lock.oakum-write.1.2.3`"),
        "{stderr}"
    );
}

/// A regular workspace-wide file in a nested workspace stages beside itself,
/// in a directory no package lives in.
#[test]
fn a_staging_file_beside_a_nested_workspace_lock_is_named() {
    let root = temp_git_repo("staging-nested-lock");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::create_dir_all(root.join("rust/a/src")).expect("crate");
    fs::write(
        root.join("rust/Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"a\"]\n",
    )
    .expect("workspace");
    fs::write(
        root.join("rust/a/Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("manifest");
    fs::write(root.join("rust/a/src/lib.rs"), "").expect("lib");
    fs::write(
        root.join("rust/Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\n",
    )
    .expect("lock");
    commit(&root, "init");
    fs::write(root.join("rust/.Cargo.lock.oakum-write.1.2.3"), "").expect("staging file");

    let (code, _, stderr) = oakum_exit(&root.join("rust"), &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("`rust/.Cargo.lock.oakum-write.1.2.3`"),
        "{stderr}"
    );
}

/// The same for a nested pnpm workspace: its lock and catalog sit in a
/// directory the private root package does not make a package directory.
#[cfg(unix)]
#[test]
fn a_staging_file_beside_a_nested_pnpm_lock_is_named() {
    let root = temp_git_repo("staging-nested-pnpm");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::create_dir_all(root.join("js/pkgs/a")).expect("pkg");
    fs::write(
        root.join("js/package.json"),
        "{\"name\":\"root\",\"private\":true}\n",
    )
    .expect("root");
    fs::write(
        root.join("js/pnpm-workspace.yaml"),
        "packages:\n  - 'pkgs/*'\n",
    )
    .expect("workspace");
    fs::write(
        root.join("js/pkgs/a/package.json"),
        "{\"name\":\"a\",\"version\":\"0.1.0\"}\n",
    )
    .expect("package");
    commit(&root, "init");
    fs::write(root.join("js/.pnpm-lock.yaml.oakum-write.1.2.3"), "").expect("staging file");

    let (code, _, stderr) = oakum_exit(&root.join("js"), &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("`js/.pnpm-lock.yaml.oakum-write.1.2.3`"),
        "{stderr}"
    );
}

/// The uncovered count is said inline when an unmanaged-intent refusal is the
/// one that travels. Dropping the line, or dropping just its `also` prefix,
/// both survived — the branch's whole reason for existing was unguarded.
#[test]
fn an_unmanaged_intent_refusal_still_reports_the_uncovered_count() {
    let root = mixed_workspace("unmanaged-and-uncovered", "");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join("alpha/src/lib.rs"), "// changed\n").expect("edit alpha");
    commit(&root, "name beta and touch alpha");

    let (ok, _stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "expected a refusal: {stderr}");
    assert!(
        stderr.contains("`beta` is named by intent but is not version-managed"),
        "{stderr}"
    );
    assert!(
        stderr.contains("also error: 1 package(s) changed with no covering intent"),
        "the shadowed uncovered count is still reported and marked: {stderr}"
    );
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(
        lines[0].starts_with("error: `beta` is named by intent")
            && lines[1] == "also error: 1 package(s) changed with no covering intent"
            && lines[2].starts_with("  alpha (cargo): changed with no covering intent; "),
        "each refusal owns its own evidence, even two from one look: {stderr}"
    );
}

/// Two refusals from one look both reach the document. The prose has carried
/// both since `carry` gained `also`; the document reported the first and
/// dropped the second, so for this run the machine channel was strictly less
/// informative than the stderr it replaces.
#[test]
fn both_refusals_from_one_look_reach_the_document() {
    let root = mixed_workspace("json-two-refusals", "");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join("alpha/src/lib.rs"), "// changed\n").expect("edit alpha");
    commit(&root, "name beta and touch alpha");

    let (code, document, stderr) =
        oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1", "--json"]);
    assert_eq!(code, Some(1), "{stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let coverage = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "coverage")
        .expect("a coverage row")
        .clone();

    let refusals = coverage["refusals"].as_array().expect("refusals");
    assert_eq!(refusals.len(), 2, "one look, two refusals: {document}");
    let summaries: Vec<&str> = refusals
        .iter()
        .map(|refusal| refusal["summary"].as_str().expect("a summary"))
        .collect();
    assert!(
        summaries[0].contains("`beta` is named by intent but is not version-managed"),
        "{document}"
    );
    assert!(
        summaries[1].contains("1 package(s) changed with no covering intent"),
        "the shadowed refusal is carried, not dropped: {document}"
    );
    assert!(
        refusals[1]["detail"][0]
            .as_str()
            .expect("detail")
            .contains("alpha (cargo): changed with no covering intent"),
        "each refusal keeps its own evidence: {document}"
    );
    assert_eq!(refusals[0]["deciding"], true, "{document}");
    assert_eq!(refusals[1]["deciding"], false, "{document}");
}

/// The scope's two counts are adjacent `usize` filled from one struct, so a
/// transposition is silent on any fixture where they agree. This one excludes a
/// package so they cannot.
#[test]
fn the_scope_counts_differ_when_a_package_is_excluded() {
    let root = mixed_workspace("json-scope-counts", "exclude = [\"beta\"]\n");

    let (_, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {stderr}"));
    assert_eq!(
        parsed["scope"]["packages"], 2,
        "both members are present: {document}"
    );
    assert_eq!(
        parsed["scope"]["selected"], 1,
        "only one survives `exclude`, and the counts must not be swappable: {document}"
    );
}

/// A refused stdout write must not restate the run. Gated to unix: the Windows
/// job's broken-pipe semantics and error text differ, and the outcome class is
/// what this pins, not the OS string.
#[cfg(unix)]
#[test]
fn a_refused_document_write_does_not_outrank_the_finding() {
    use std::process::Command;

    let root = temp_git_repo("json-delivery");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    // `| true` closes the reader immediately, so the document write fails.
    let out = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -o pipefail; '{}' check --json | true",
            env!("CARGO_BIN_EXE_oakum")
        ))
        .current_dir(&root)
        .output()
        .expect("bash");
    let code = out.status.code();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        code,
        Some(1),
        "the established finding keeps its class and its code: {stderr}"
    );
    assert!(
        stderr.contains("error: 1 package(s) bumped without a tag"),
        "the finding still leads: {stderr}"
    );
    assert!(
        stderr.contains("also unverified: report could not be delivered to stdout"),
        "the delivery failure rides beneath it: {stderr}"
    );

    // With nothing else refusing, the undelivered document is the whole verdict.
    let clean = temp_git_repo("json-delivery-clean");
    write_pinned_config(&clean, BINARY_VERSION, "");
    cargo_package(&clean, "demo", "0.1.0");
    commit(&clean, "init");
    git(&clean, &["tag", "v0.1.0"]);
    let out = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -o pipefail; '{}' check --json | true",
            env!("CARGO_BIN_EXE_oakum")
        ))
        .current_dir(&clean)
        .output()
        .expect("bash");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a run that wrote nothing did not answer the question: {stderr}"
    );
    assert!(
        stderr.contains("report could not be delivered to stdout"),
        "{stderr}"
    );
}

/// A look that said something without refusing reads `reported`, not `ok` —
/// the distinction `AGENTS.md` asks for by name: "a test drives the run that
/// could not check X and asserts it reads differently from the run that checked
/// and found nothing." The headline field carries it too, since a consumer that
/// branches on `.outcome` would otherwise see the same word for both.
#[test]
fn a_look_that_reported_without_refusing_is_not_ok() {
    let root = temp_git_repo("json-reported");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    // Uncommitted, so the coverage look reports what it could not read from
    // `HEAD` without refusing over it.
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(0), "advisory, not a refusal: {stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));

    let coverage = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "coverage")
        .expect("a coverage row")
        .clone();
    assert_eq!(
        coverage["outcome"], "reported",
        "a look that said something is not a look that found nothing: {document}"
    );
    assert!(
        !coverage["reports"].as_array().expect("reports").is_empty(),
        "what it said travels with it: {document}"
    );
    assert_eq!(
        parsed["outcome"], "reported",
        "the headline field must not undo the distinction: {document}"
    );
    // Keyed by look, so a sibling that said nothing stays `ok` and carries no
    // reports — the reason `said` is a pair rather than a flat list.
    let staging = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "staging")
        .expect("a staging row")
        .clone();
    assert_eq!(staging["outcome"], "ok", "{document}");
    assert!(staging["reports"].is_null(), "{document}");
}

/// A run whose only refusal could not look reads as `unverified`, not `ok`.
/// Without this the two are interchangeable to the suite, and a shallow clone
/// publishes a clean document to every machine consumer.
#[test]
fn a_run_that_only_could_not_look_says_so_in_the_document() {
    let root = temp_git_repo("json-unverified-only");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join(".changeset/.foo.md.oakum-write.1.2.3"), "").expect("staging file");

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(2), "a look that did not happen: {stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    assert_eq!(
        parsed["outcome"], "unverified",
        "the document must not read `ok`: {document}"
    );
    assert_eq!(parsed["tool_version"], BINARY_VERSION, "{document}");
    let staging = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "staging")
        .expect("a staging row")
        .clone();
    assert_eq!(staging["outcome"], "unverified", "{document}");
    assert_eq!(
        staging["refusals"][0]["outcome"], "unverified",
        "{document}"
    );
    assert_eq!(staging["refusals"][0]["deciding"], true, "{document}");
}

/// The refusal names the first offender; the detail names the others, not the
/// first again — a repeated name reads as a third package.
#[test]
fn a_second_unmanaged_name_is_listed_once_beneath_the_first() {
    let root = temp_git_repo("two-unmanaged");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\", \"gamma\"]\n",
    )
    .expect("workspace");
    for (name, extra) in [
        ("alpha", ""),
        ("beta", "publish = false\n"),
        ("gamma", "publish = false\n"),
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.join("src")).expect("src");
        fs::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"
            ),
        )
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    fs::write(
        root.join(".changeset/two.md"),
        "---\nbeta: minor\ngamma: minor\n---\nnote\n",
    )
    .expect("bump");
    commit(&root, "name beta and gamma");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert_eq!(code, Some(1), "{stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(
        lines[0].starts_with("error: `beta` is named by intent"),
        "{stderr}"
    );
    assert!(
        lines[1].starts_with("  `gamma` is named by intent"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("`beta` is named").count(), 1, "{stderr}");
}

/// Without `--strict` the uncovered change is a report, not a refusal: said
/// before the verdict, and the run still exits 0.
#[test]
fn an_uncovered_change_without_strict_is_reported_not_refused() {
    let root = temp_git_repo("advisory");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("demo (cargo): changed with no covering intent; commit `"),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "\"fix: touch demo\" (fix) asks for a release; add a bump file with `oakum add --packages demo:patch`"
        ),
        "the commit names the level the fix asks for: {stderr}"
    );
    assert!(!stderr.contains("error:"), "{stderr}");
}
