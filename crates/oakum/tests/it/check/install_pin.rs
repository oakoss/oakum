use super::*;

#[test]
fn tool_version_mismatch_does_not_block_check() {
    let root = temp_git_repo("pin");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    write_install_pin(&root, "9.9.9");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    let mut lines = stderr.lines();
    assert_eq!(lines.next(), Some(TOOL_VERSION_WARNING), "{stderr}");
    assert!(lines.all(|line| !line.contains("upgrade")), "{stderr}");
}

#[test]
fn both_intent_mechanisms_off_fails() {
    let root = temp_git_repo("no-intent");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = false\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = refused_before_the_report(&root, &["check"]);
    assert!(!ok, "both mechanisms off must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("change-files"), "{stderr}");
    assert!(stderr.contains("conventional-commits"), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn one_intent_mechanism_on_is_ready() {
    let root = temp_git_repo("commits-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = false\nconventional-commits = true\n",
    );
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);

    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = true\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn check_and_tag_drift_share_tag_evaluation() {
    let root = temp_git_repo("share");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "tool-version = \"9.9.9\"\n");
    write_install_pin(&root, "9.9.9");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let check_run = check(&root);
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert!(check_run.0, "{}", check_run.2);
    assert_eq!(check_run.0, drift_run.0);
    assert_eq!(check_run.2, drift_run.2);
    assert_eq!(check_run.2.trim_end(), TOOL_VERSION_WARNING);
}

#[test]
fn mismatched_tool_version_still_runs_tag_drift() {
    let root = temp_git_repo("drift-toolver");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    write_install_pin(&root, "9.9.9");
    let (ok, _stdout, stderr) = oakum_output(&root, &["tag-drift"]);
    assert!(ok, "{stderr}");
    assert_eq!(stderr.trim_end(), TOOL_VERSION_WARNING);
}

#[test]
fn matching_install_pin_is_ready() {
    let root = temp_git_repo("pin-match");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_binstall_version_flag_pin_is_ready() {
    let root = temp_git_repo("pin-binstall-ver");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall oakum --version {BINARY_VERSION}\n"),
    );
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_binstall_version_flag_pin_is_unverified() {
    let root = temp_git_repo("pin-binstall-ver-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall oakum --version {MISMATCHED_PIN}\n"),
    );
    write_install_pin(&root, BINARY_VERSION);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a binstall --version pin that disagrees with another must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn matching_cargo_install_oakum_at_pin_is_ready() {
    let root = temp_git_repo("pin-cargo-at");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum@{BINARY_VERSION}\n"),
    );
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_cargo_install_oakum_at_pin_is_unverified() {
    let root = temp_git_repo("pin-cargo-at-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum@{MISMATCHED_PIN}\n"),
    );
    write_install_pin(&root, BINARY_VERSION);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a cargo install@ pin that disagrees with another must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn matching_cargo_install_version_flag_pin_is_ready() {
    let root = temp_git_repo("pin-cargo-ver");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum --version {BINARY_VERSION}\n"),
    );
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_cargo_install_version_flag_pin_is_unverified() {
    let root = temp_git_repo("pin-cargo-ver-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum --version {MISMATCHED_PIN}\n"),
    );
    write_install_pin(&root, BINARY_VERSION);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a cargo install --version pin that disagrees with another must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn matching_install_action_tool_pin_is_ready() {
    let root = temp_git_repo("pin-install-action");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("- uses: oakoss/install-action@v1\n  with:\n    tool: oakum@{BINARY_VERSION}\n"),
    );
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn unversioned_install_action_tool_pin_is_unverified() {
    let root = temp_git_repo("pin-install-action-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        "- uses: oakoss/install-action@v1\n  with:\n    tool: oakum\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an unversioned install-action tool pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "unversioned install-action tool must not read as a missing pin: {stderr}"
    );
    assert!(
        stderr.contains(".github/workflows/ci.yml"),
        "unversioned pin must name the workflow: {stderr}"
    );
}

#[test]
fn unversioned_binstall_oakum_pin_is_unverified() {
    let root = temp_git_repo("pin-binstall-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(&root, "ci.yml", "run: cargo binstall oakum\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an unversioned binstall oakum pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "unversioned binstall must not read as a missing pin: {stderr}"
    );
}

#[test]
fn unversioned_cargo_install_oakum_pin_is_unverified() {
    let root = temp_git_repo("pin-cargo-install-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(&root, "ci.yml", "run: cargo install oakum\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an unversioned cargo install oakum pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "unversioned cargo install must not read as a missing pin: {stderr}"
    );
}

#[test]
fn mismatched_install_action_tool_pin_is_unverified() {
    let root = temp_git_repo("pin-install-action-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("- uses: oakoss/install-action@v1\n  with:\n    tool: oakum@{MISMATCHED_PIN}\n"),
    );
    write_install_pin(&root, BINARY_VERSION);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an install-action tool pin that disagrees with another must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn missing_install_pin_is_unverified() {
    let root = temp_git_repo("pin-missing");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a missing pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
    assert!(
        !stderr.contains("without a version"),
        "a missing pin must not read as unversioned: {stderr}"
    );
}

/// A config without `tool-version` still owes a pin: the key never decided
/// whether the pin look runs (ADR-0038).
#[test]
fn a_config_without_tool_version_still_owes_a_pin() {
    let root = temp_git_repo("pin-missing-no-tool-version");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "change-files = true\n");

    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(2), "{stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let pin = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "install pin")
        .unwrap_or_else(|| panic!("no `install pin` row: {document}"));
    assert_eq!(pin["outcome"], "unverified", "{document}");
}

#[test]
fn matching_workspace_oakum_member_is_ready() {
    let root = temp_git_repo("pin-ws-oakum");
    write_workspace_oakum_member(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", &format!("v{BINARY_VERSION}")]);
    write_config(&root, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_workspace_oakum_member_is_unverified() {
    let root = temp_git_repo("pin-ws-mismatch");
    write_workspace_oakum_member(&root, MISMATCHED_PIN);
    commit(&root, "init");
    write_config(&root, "");
    write_install_pin(&root, BINARY_VERSION);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a workspace oakum version that disagrees with another pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
    assert!(stderr.contains(BINARY_VERSION), "{stderr}");
}

/// The look asks whether the pins agree, not whether they match the config:
/// a Renovate bump of the only pin goes green without a second edit.
#[test]
fn pins_that_agree_pass_whatever_tool_version_says() {
    let root = temp_git_repo("pin-agree-config-differs");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, &format!("tool-version = \"{MISMATCHED_PIN}\"\n"));
    write_install_pin(&root, BINARY_VERSION);
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (code, stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stderr.trim_end(), TOOL_VERSION_WARNING);
    assert_eq!(
        stdout.lines().skip(2).collect::<Vec<_>>(),
        ["coverage: no version-managed package changed"],
        "only the report and the coverage account: {stdout}"
    );
    let (code, row, top, stderr) = install_pin_row(&root);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(row["outcome"], "ok", "{row}");
    assert_eq!(top, "ok");
}

#[test]
fn pins_that_disagree_name_every_source() {
    let root = temp_git_repo("pin-disagree");
    write_scoped_package_json_pin(&root, MISMATCHED_PIN);
    write_config(&root, "");
    write_install_pin(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (code, _stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(
        stderr.trim_end(),
        format!(
            "unverified: install pins disagree: `{BINARY_VERSION}` \
             (.github/workflows/release.yml), `{MISMATCHED_PIN}` (package.json); keep one \
             exact version"
        ),
        "the pin decides, naming every source in the order found"
    );
    let (code, row, top, stderr) = install_pin_row(&root);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(row["outcome"], "unverified", "{row}");
    assert_eq!(top, "unverified");
}

/// The advice names the binary that ran, never `tool-version`, and the pin
/// that fits the repository: a mise entry outside an npm workspace.
#[test]
fn no_pin_advises_a_mise_pin_of_this_binary_outside_npm() {
    let root = temp_git_repo("pin-none-cargo");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &format!("tool-version = \"{MISMATCHED_PIN}\"\n"));
    let (code, _stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "pin this binary's version: add `\"github:oakoss/oakum\" = \"{BINARY_VERSION}\"` \
             under `[tools]` in `.mise.toml`"
        )),
        "{stderr}"
    );
    assert!(!stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn no_pin_advises_a_dev_dependency_of_this_binary_in_npm() {
    let root = temp_git_repo("pin-none-npm");
    fs::write(
        root.join("package.json"),
        r#"{"name":"demo","version":"0.1.0"}"#,
    )
    .expect("package.json");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &format!("tool-version = \"{MISMATCHED_PIN}\"\n"));
    let (code, _stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "pin this binary's version: \
             `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@{BINARY_VERSION}`"
        )),
        "{stderr}"
    );
    assert!(!stderr.contains(MISMATCHED_PIN), "{stderr}");
}

/// Check's own advice must name a pin the look then accepts.
#[test]
fn the_advised_mise_pin_is_ready() {
    let root = temp_git_repo("pin-mise-github");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\n\"github:oakoss/oakum\" = \"{BINARY_VERSION}\"\n"),
    )
    .expect(".mise.toml");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// The `install pin` row of `check --json`, the exit code, and the document's
/// top-level outcome.
fn install_pin_row(root: &Path) -> (Option<i32>, serde_json::Value, String, String) {
    let (code, document, stderr) = oakum_exit(root, &["check", "--json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let row = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "install pin")
        .expect("an install pin row")
        .clone();
    let top = parsed["outcome"].as_str().expect("outcome").to_owned();
    (code, row, top, stderr)
}

#[test]
fn a_matching_pin_does_not_forgive_a_mismatch() {
    let root = temp_git_repo("pin-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".github/workflows/ci.yml"),
        format!("run: cargo binstall --no-confirm oakum@{MISMATCHED_PIN}\n"),
    )
    .expect("ci workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a second mismatched pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_version_flag() {
    let root = temp_git_repo("pin-binstall-ver-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall oakum --version {MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched binstall --version pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_cargo_install_version_flag() {
    let root = temp_git_repo("pin-cargo-ver-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum --version {MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched cargo install --version pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_cargo_install_at() {
    let root = temp_git_repo("pin-cargo-at-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum@{MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched cargo install@ pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_install_action_tool() {
    let root = temp_git_repo("pin-install-action-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("- uses: oakoss/install-action@v1\n  with:\n    tool: oakum@{MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched install-action tool pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn check_only_workflow_is_not_a_pin() {
    let root = temp_git_repo("pin-check-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(root.join(".github/workflows/ci.yml"), "run: oakum check\n").expect("ci workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a check-only workflow must not look pinned");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
    assert!(
        !stderr.contains("without a version"),
        "check-only workflow must not read as unversioned: {stderr}"
    );
}

fn write_package_json_pin(root: &Path, version: &str) {
    fs::write(
        root.join("package.json"),
        format!(r#"{{"name":"demo","version":"0.1.0","devDependencies":{{"oakum":"{version}"}}}}"#),
    )
    .expect("package.json");
}

fn write_scoped_package_json_pin(root: &Path, spec: &str) {
    fs::write(
        root.join("package.json"),
        format!(
            r#"{{"name":"demo","version":"0.1.0","devDependencies":{{"@oakoss/oakum":"{spec}"}}}}"#
        ),
    )
    .expect("package.json");
}

#[test]
fn matching_scoped_package_json_pin_is_ready() {
    let root = temp_git_repo("pin-npm-scoped");
    write_scoped_package_json_pin(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_scoped_package_json_pin_is_unverified() {
    let root = temp_git_repo("pin-npm-scoped-mismatch");
    write_scoped_package_json_pin(&root, MISMATCHED_PIN);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    write_install_pin(&root, BINARY_VERSION);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an @oakoss/oakum pin that disagrees with another must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
    assert!(stderr.contains("package.json"), "{stderr}");
}

#[test]
fn a_range_on_the_scoped_package_is_unverified() {
    let root = temp_git_repo("pin-npm-scoped-range");
    write_scoped_package_json_pin(&root, "^0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a range is not a pin");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("pins `@oakoss/oakum` as `^0.1.0`, which is not an exact version"),
        "{stderr}"
    );
}

#[test]
fn a_pnpm_add_workflow_line_is_a_pin() {
    let root = temp_git_repo("pin-pnpm-add");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        format!("run: pnpm add -D @oakoss/oakum@{BINARY_VERSION}\n"),
    )
    .expect("workflow");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn a_mismatched_npx_workflow_line_is_unverified() {
    let root = temp_git_repo("pin-npx-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        format!("run: npx @oakoss/oakum@{MISMATCHED_PIN} check --strict\n"),
    )
    .expect("workflow");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall --no-confirm oakum@{BINARY_VERSION}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an npx pin that disagrees with another must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("install pins disagree"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn an_unversioned_npm_install_is_unverified() {
    let root = temp_git_repo("pin-npm-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        "run: npm i -g @oakoss/oakum\n",
    )
    .expect("workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a bare npm install must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "an unversioned install must not read as no install: {stderr}"
    );
}

#[test]
fn a_pnpm_exec_invocation_is_not_a_pin() {
    let root = temp_git_repo("pin-pnpm-exec");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/ci.yml"),
        "run: pnpm exec oakum check --strict\n",
    )
    .expect("workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an invocation is not a pin");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
}

#[test]
fn a_mise_npm_backend_pin_is_ready() {
    let root = temp_git_repo("pin-mise-npm");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\n\"npm:@oakoss/oakum\" = \"{BINARY_VERSION}\"\n"),
    )
    .expect(".mise.toml");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_package_json_pin_without_workflow_is_ready() {
    let root = temp_git_repo("pin-npm");
    write_package_json_pin(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_a_mismatched_package_json_pin() {
    let root = temp_git_repo("pin-npm-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_package_json_pin(&root, MISMATCHED_PIN);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched package.json pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

fn write_mise_pin(root: &Path, version: &str) {
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\noakum = \"{version}\"\n"),
    )
    .expect(".mise.toml");
}

fn write_workspace_oakum_member(root: &Path, version: &str) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/oakum\"]\n",
    )
    .expect("workspace Cargo.toml");
    fs::create_dir_all(root.join("crates/oakum/src")).expect("oakum src");
    fs::write(
        root.join("crates/oakum/Cargo.toml"),
        format!("[package]\nname = \"oakum\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .expect("oakum Cargo.toml");
    fs::write(root.join("crates/oakum/src/lib.rs"), "").expect("lib.rs");
}

#[test]
fn matching_mise_pin_without_workflow_is_ready() {
    let root = temp_git_repo("pin-mise");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    write_mise_pin(&root, BINARY_VERSION);
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_an_inexact_mise_pin() {
    let root = temp_git_repo("pin-mise-latest");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_mise_pin(&root, "latest");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an inexact mise pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("latest"), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_an_inexact_package_json_pin() {
    let root = temp_git_repo("pin-npm-latest");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_package_json_pin(&root, "latest");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an inexact package.json pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("latest"), "{stderr}");
}

/// The pin files below, each planted as a FIFO no one writes to.
/// A blocking open waits for a writer forever; the look has to refuse instead.
#[cfg(unix)]
#[test]
fn a_fifo_at_an_install_pin_path_is_unverified_rather_than_a_hang() {
    for (label, path) in [
        ("fifo-mise", "mise.toml"),
        ("fifo-dot-mise", ".mise.toml"),
        ("fifo-package-json", "package.json"),
        ("fifo-action", ".github/actions/setup/action.yml"),
    ] {
        let root = temp_git_repo(label);
        cargo_package(&root, "demo", "0.1.0");
        commit(&root, "init");
        git(&root, &["tag", "v0.1.0"]);
        write_pinned_config(&root, BINARY_VERSION, "");
        let fifo = root.join(path);
        fs::create_dir_all(fifo.parent().expect("parent")).expect("parent dir");
        let made = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
        assert!(made.success(), "mkfifo {path}: {made}");

        let output = output_within(
            oakum(&root).arg("check"),
            std::time::Duration::from_secs(10),
        )
        .unwrap_or_else(|| panic!("check blocked on a FIFO at `{path}`"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{path}: {stderr}");
        assert!(
            stderr.contains(&format!("unverified: `{path}` is not a regular file")),
            "{path}: {stderr}"
        );
    }
}

/// A pin file the look could not open or read is a look that did not happen,
/// beside a matching pin too; it refuses rather than reading as no pin. The
/// bytes go where no discovery tool reads, so only the pin look can refuse.
#[test]
fn an_unreadable_pin_file_is_unverified_rather_than_no_pin() {
    let cases = [
        (".github/actions/setup/action.yml", false),
        #[cfg(unix)]
        ("mise.toml", true),
        #[cfg(unix)]
        (".mise.toml", true),
        #[cfg(unix)]
        ("package.json", true),
        #[cfg(unix)]
        (".github/actions/setup/action.yml", true),
    ];
    for (path, self_loop) in cases {
        let root = temp_git_repo("unreadable-pin");
        cargo_package(&root, "demo", "0.1.0");
        commit(&root, "init");
        git(&root, &["tag", "v0.1.0"]);
        write_pinned_config(&root, BINARY_VERSION, "");
        let planted = root.join(path);
        fs::create_dir_all(planted.parent().expect("parent")).expect("parent dir");
        if self_loop {
            #[cfg(unix)]
            std::os::unix::fs::symlink(planted.file_name().expect("name"), &planted)
                .expect("self-loop");
        } else {
            fs::write(&planted, b"\xff\xfe").expect("non-UTF-8 bytes");
        }

        let (code, stdout, stderr) = oakum_exit(&root, &["check"]);
        assert_eq!(code, Some(2), "{path}: {stdout}{stderr}");
        assert!(
            stderr.contains(&format!("unverified: failed to read `{path}`")),
            "{path}: {stderr}"
        );
    }
}

/// A dangling pin file beside a matching pin could hide one that disagrees,
/// so it is a look that did not happen rather than no pin (ADR-0038).
#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_an_install_pin_path_is_unverified() {
    let root = temp_git_repo("dangling-mise");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    std::os::unix::fs::symlink("nowhere.toml", root.join("mise.toml")).expect("symlink");

    let (code, stdout, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stdout}{stderr}");
    assert!(
        stderr.contains("unverified: `mise.toml` is a symlink to a missing target"),
        "{stderr}"
    );
}

#[test]
fn matching_undotted_mise_toml_pin_is_ready() {
    let root = temp_git_repo("pin-mise-toml");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    fs::write(
        root.join("mise.toml"),
        format!("[tools]\noakum = \"{BINARY_VERSION}\"\n"),
    )
    .expect("mise.toml");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_a_mismatched_mise_pin() {
    let root = temp_git_repo("pin-mise-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_mise_pin(&root, MISMATCHED_PIN);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched mise pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn yaml_extension_workflow_pin_is_ready() {
    let root = temp_git_repo("pin-yaml");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_config(&root, "");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yaml"),
        format!("run: cargo binstall --no-confirm oakum@{BINARY_VERSION}\n"),
    )
    .expect("yaml workflow");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}
