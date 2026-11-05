use super::*;

#[test]
fn matching_manifest_is_clean() {
    let root = temp_git_repo("match");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// A workspace whose only package is private, with `private-packages` left at
/// its default, version-manages nothing. Every plan is then empty and a release
/// does nothing while reporting success, so `check` says so instead of passing.
/// Measured shape: a repository migrated from a tool that had opted private
/// packages in, where the setting was not carried.
#[test]
fn a_config_that_manages_nothing_is_unverified_not_clean() {
    let root = temp_git_repo("manages-nothing");
    write_pinned_config(&root, BINARY_VERSION, "");
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a config that manages nothing must not pass: {stdout}");
    assert!(
        stderr.contains("manages no package on either axis"),
        "names the condition: {stderr}"
    );
    assert!(
        stderr.contains("private-packages.version = true"),
        "names the fix: {stderr}"
    );
}

/// The management look reports without preempting the others: an unfinished
/// write means a file may be stale, and that diagnosis may not be hidden
/// behind a config-level one.
#[test]
fn a_config_that_manages_nothing_does_not_hide_the_other_looks() {
    let root = temp_git_repo("manages-nothing-and-pin");
    write_pinned_config(&root, BINARY_VERSION, "");
    private_npm_package(&root, "demo", "0.1.0");
    fs::write(
        root.join(".changeset/.foo.oakum-write.123.456.0"),
        "partial",
    )
    .expect("staging file");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "expected a refusal");
    assert!(
        stderr.contains("every selected package is private"),
        "reports the management fault: {stderr}"
    );
    assert!(
        stderr.contains("oakum staging file"),
        "and still names the unfinished write: {stderr}"
    );
}

/// A stale install pin is the more fundamental diagnosis, since a different
/// oakum may not have the other looks at all — but it must not be the only one
/// a run reports. Both faults live behind different early returns, so this pins
/// the pair the management test cannot reach.
#[test]
fn a_stale_install_pin_does_not_hide_an_unfinished_write() {
    let root = temp_git_repo("pin-and-staging");
    write_config(&root, "");
    write_install_pin(&root, BINARY_VERSION);
    write_stale_pin(&root);
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join(".changeset/.x.oakum-write.1.2.0"), "partial").expect("staging file");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "expected a refusal");
    assert!(
        stderr.contains("install pins disagree"),
        "names the stale pin: {stderr}"
    );
    assert!(
        stderr.contains("oakum staging file"),
        "and still names the unfinished write: {stderr}"
    );
}

/// The pin look lives one level down, inside the tag evaluation, so binding
/// the looks in `run` alone left it hiding the coverage refusal.
#[test]
fn a_stale_install_pin_does_not_hide_an_uncovered_change() {
    let root = temp_git_repo("pin-and-coverage");
    write_config(&root, "");
    write_install_pin(&root, BINARY_VERSION);
    write_stale_pin(&root);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    let (ok, _stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "expected a refusal");
    assert!(
        stderr.contains("install pins disagree"),
        "names the stale pin: {stderr}"
    );
    assert!(
        stderr.contains("changed with no covering intent"),
        "and still names the uncovered change: {stderr}"
    );
}

/// A version-only opt-in is managed. Without this, dropping the version axis
/// from the predicate leaves the whole suite green: the both-axes test is
/// carried by `tag_managed` alone.
#[test]
fn opting_only_the_version_axis_in_clears_the_management_refusal() {
    let root = temp_git_repo("manages-version-only");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = true, tag = false }\n",
    );
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(ok, "a version-only opt-in is managed: {stderr}");
}

/// One managed member is enough. Every other fixture reaching this gate holds a
/// single package, where `any` and `all` agree — so without a mixed workspace,
/// a predicate that demanded every member be managed would refuse the ordinary
/// monorepo shape (public packages beside a private tooling crate) and the
/// suite would not notice.
#[test]
fn a_mixed_workspace_passes_when_one_member_is_publishable() {
    let root = temp_git_repo("manages-mixed-workspace");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for (name, extra) in [("alpha", ""), ("beta", "publish = false\n")] {
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
    let (ok, _stdout, stderr) = check(&root);
    assert!(ok, "one managed member is enough: {stderr}");
}

/// The stated-exclusion contract, named rather than covered by accident. Two
/// unrelated tests happen to use fixtures whose selection is empty, so a
/// fixture change in either would remove this coverage silently.
#[test]
fn a_selection_emptied_by_exclude_stays_silent() {
    let root = temp_git_repo("manages-excluded-silent");
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(
        ok,
        "an exclusion is a decision oakum does not argue with: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");

    // The divergence is deliberate and belongs in one place, so it is asserted
    // here beside the silence it diverges from: `status` is not a gate, so it
    // says what this config is, while `check` declines to refuse a decision the
    // config states (`okm-404.32`).
    let (_, status_out, _) = oakum_output(&root, &["status"]);
    assert!(
        status_out.contains("`include`/`exclude` leave no package selected"),
        "status reports what check stays silent on: {status_out}"
    );
    let (_, json, _) = oakum_output(&root, &["status", "--json"]);
    assert!(json.contains("\"selection_empty\": true"), "{json}");
    assert!(
        json.contains("\"manages_nothing\": false"),
        "the two reasons stay distinct on the wire: {json}"
    );
}

/// ADR-0027 records private-package silence as something a changesets migratee
/// keeps without a config change, and the printed workflow runs `check
/// --strict`, so gating here would turn every private-package change red until
/// someone wrote config. `status` reports it instead; `check` decides, and
/// there is nothing here to decide.
#[test]
fn a_changed_package_the_config_leaves_unmanaged_does_not_fail_the_gate() {
    let root = mixed_workspace("unmanaged-changed", "");
    fs::write(root.join("beta/src/lib.rs"), "// changed\n").expect("change");
    commit(&root, "touch beta");
    let (ok, _stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(
        !stderr.contains("beta"),
        "the gate says nothing about a package nobody asked it to version: {stderr}"
    );
}

/// The other half of the same asymmetry: a bump file naming a package the
/// config cannot plan. `status` refuses through `retain_managed` while
/// composing; `check` composes no plan, so it asks directly — and refuses
/// without `--strict`, because the file is wrong however the run is invoked.
#[test]
fn a_bump_file_naming_an_unmanaged_package_refuses_as_status_does() {
    let root = mixed_workspace("unmanaged-named", "");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: minor\n---\nnote\n",
    )
    .expect("bump");
    commit(&root, "name beta");
    let (ok, _stdout, stderr) = checked(&root, &["check", "--from", "HEAD~1"]);
    assert!(!ok, "intent oakum cannot honour is not clean: {stderr}");
    assert!(
        stderr.contains("`beta` is named by intent but is not version-managed"),
        "the same sentence status uses: {stderr}"
    );
}

/// `retain_managed` refuses on any intent-named package it cannot version, an
/// exclusion included, so a gate asking the narrower "selected and unmanaged"
/// passes a tree `status` and `version` both reject — a green check followed by
/// a failed version PR on the same file. Measured: restoring the narrow
/// question left the whole suite green.
#[test]
fn a_bump_file_naming_an_excluded_package_refuses_as_version_does() {
    let root = mixed_workspace("named-and-excluded", "exclude = [\"beta\"]\n");
    fs::write(
        root.join(".changeset/beta.md"),
        "---\nbeta: minor\n---\nnote\n",
    )
    .expect("bump");
    commit(&root, "name beta");
    let (ok, _stdout, stderr) = checked(&root, &["check", "--from", "HEAD~1"]);
    assert!(
        !ok,
        "a bump file oakum can never honour is not clean, excluded or not: {stderr}"
    );
    assert!(
        stderr.contains("`beta` is named by intent but is not version-managed"),
        "{stderr}"
    );
}

/// An exclusion is a decision someone wrote down, and the coverage look must
/// not argue with it — the distinction `version_managed` alone cannot express,
/// since it already folds the selection in.
#[test]
fn a_changed_package_removed_by_exclude_stays_silent() {
    let root = mixed_workspace("unmanaged-excluded", "exclude = [\"beta\"]\n");
    fs::write(root.join("beta/src/lib.rs"), "// changed\n").expect("change");
    commit(&root, "touch beta");
    let (ok, stdout, stderr) = checked(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("beta"), "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
}

/// A workspace with one publishable member is every existing user's shape, and
/// nothing else asserts the gate stays silent for it.
#[test]
fn a_workspace_with_one_publishable_member_passes_the_management_look() {
    let root = temp_git_repo("manages-mixed");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// Tagging alone is work, and ADR-0027 makes the axes independent, so a
/// repository that only tags its private packages is not the silent no-op the
/// refusal exists to catch.
#[test]
fn opting_only_the_tag_axis_in_clears_the_management_refusal() {
    let root = temp_git_repo("manages-tag-only");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = false, tag = true }\n",
    );
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(ok, "a tag-only opt-in is managed: {stderr}");
}

/// The same workspace with the packages opted in is clean again, so the refusal
/// tracks the config rather than the manifest being private.
#[test]
fn opting_private_packages_in_clears_the_management_refusal() {
    let root = temp_git_repo("manages-private");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = true, tag = true }\n",
    );
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
}

/// Commits-only intent, `beta` and `gamma` private and unchanged, and a
/// `fix(beta)` commit touching only `README.md`: intent that releases nothing.
/// `gamma` is the unversioned package no commit names.
fn commit_naming_private_beta(label: &str, config_extra: &str) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        &format!("change-files = false\nconventional-commits = true\n{config_extra}"),
    );
    cargo_members(
        &root,
        &[("alpha", "0.1.0"), ("beta", "0.1.0"), ("gamma", "0.1.0")],
    );
    for name in ["beta", "gamma"] {
        let manifest = root.join(name).join("Cargo.toml");
        let text = fs::read_to_string(&manifest).expect("manifest");
        fs::write(&manifest, format!("{text}publish = false\n")).expect("private");
    }
    commit(&root, "init");
    fs::write(root.join("README.md"), "named\n").expect("readme");
    commit(&root, "fix(beta): a private bug");
    root
}

/// `version` consumes the intent, so it says what it dropped; only the named
/// package is reported, wherever it is read.
#[test]
fn only_the_named_package_is_reported_and_version_says_so_too() {
    let root = commit_naming_private_beta("named-only", "");
    let (code, stdout, stderr) = oakum_exit(&root, &["status", "--json", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        json["named_unmanaged"],
        serde_json::json!([{"name": "beta", "ecosystem": "cargo"}]),
        "{json}"
    );
    let (_, _, stderr) = oakum_exit(&root, &["check", "--from", "HEAD~1"]);
    assert!(!stderr.contains("gamma"), "{stderr}");

    // `ci version-pr` stops before any token is needed when nothing releases,
    // which is the case a commit naming only `beta` produces.
    for verb in [["version"].as_slice(), ["ci", "version-pr"].as_slice()] {
        let args = [verb, &["--from", "HEAD~1"]].concat();
        let (code, _, stderr) = oakum_exit(&root, &args);
        assert_eq!(code, Some(0), "{args:?}: {stderr}");
        assert!(
            stderr.contains("beta (cargo): named by a commit but not version-managed"),
            "{args:?}: {stderr}"
        );
    }
}

/// `check` refuses a bump file exactly where `status` and `version` do: a
/// releasing entry for an unversioned package, never a `none` one.
#[test]
fn a_none_entry_for_an_unversioned_package_passes_check() {
    // The mixed file pins that a `none` sibling does not excuse its file.
    for (entries, expected) in [
        ("beta: none", Some(0)),
        ("beta: patch", Some(1)),
        ("beta: none\ngamma: patch", Some(1)),
    ] {
        let root = temp_git_repo("none-unversioned");
        write_pinned_config(&root, BINARY_VERSION, "include = [\"alpha\"]\n");
        cargo_members(
            &root,
            &[("alpha", "0.1.0"), ("beta", "0.1.0"), ("gamma", "0.1.0")],
        );
        commit(&root, "init");
        fs::write(
            root.join(".changeset/b.md"),
            format!("---\n{entries}\n---\n\nm\n"),
        )
        .expect("bump file");
        commit(&root, "bump file");
        for verb in ["check", "status", "version"] {
            let (code, _, stderr) = oakum_exit(&root, &[verb, "--from", "HEAD~1"]);
            assert_eq!(code, expected, "{verb} {entries:?}: {stderr}");
        }
    }
}

/// Reported, never gated: the commit releases nothing, which is worth saying
/// and not worth failing a gate over, `--strict` included.
#[test]
fn a_commit_naming_an_unversioned_package_is_reported_without_gating() {
    let root = commit_naming_private_beta("named-unmanaged", "");
    for args in [
        ["check", "--from", "HEAD~1"].as_slice(),
        ["check", "--strict", "--from", "HEAD~1"].as_slice(),
    ] {
        let (code, _, stderr) = oakum_exit(&root, args);
        assert_eq!(code, Some(0), "{args:?}: {stderr}");
        assert!(
            stderr.contains(
                "beta (cargo): named by a commit but not version-managed; adjust `include`/`exclude` or set `private-packages.version = true`"
            ),
            "{args:?}: {stderr}"
        );
    }

    let excluded = commit_naming_private_beta("named-excluded", "exclude = [\"beta\"]\n");
    let (code, _, stderr) = oakum_exit(&excluded, &["check", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("named by a commit"), "{stderr}");
}
