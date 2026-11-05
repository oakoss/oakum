use super::*;

#[test]
fn a_changesets_changelog_title_is_unverified_and_names_the_fix() {
    let root = temp_git_repo("changelog-foreign-title");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# demo\n\n## 0.1.0\n\n### Patch Changes\n\n- first\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a title version would refuse must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "CHANGELOG.md does not start with `# Changelog`; oakum will not append without a recognized heading; change the first line to `# Changelog` (the old title can stay as a line under it)\n"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("unverified: 1 changelog(s) `oakum version` would refuse to append to"),
        "{stderr}"
    );
}

/// A changelog that is a link to nowhere is one `version` cannot write, not
/// one it will create.
#[cfg(unix)]
#[test]
fn a_dangling_changelog_symlink_is_unverified() {
    let root = temp_git_repo("changelog-dangling");
    cargo_package(&root, "demo", "0.1.0");
    std::os::unix::fs::symlink("docs/CHANGELOG.md", root.join("CHANGELOG.md")).expect("link");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".changeset/fix.md"),
        "---\ndemo: patch\n---\nfix\n",
    )
    .expect("bump");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains(
            "CHANGELOG.md is a symlink whose target does not exist; point it at a file, or remove it"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("unverified: 1 changelog(s) `oakum version` would refuse to append to"),
        "{stderr}"
    );
    let (code, _, stderr) = oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(1), "version cannot write through it: {stderr}");
    assert!(
        stderr.contains("`CHANGELOG.md` is a symlink whose target does not exist"),
        "{stderr}"
    );
}

/// Two packages sharing one changelog get one report for it, not two.
#[cfg(unix)]
#[test]
fn a_shared_dangling_changelog_is_reported_once() {
    let root = temp_git_repo("changelog-dangling-shared");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("package.json"),
        "{\"name\":\"web\",\"version\":\"0.1.0\"}\n",
    )
    .expect("package.json");
    std::os::unix::fs::symlink("docs/CHANGELOG.md", root.join("CHANGELOG.md")).expect("link");
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");

    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("unverified: 1 changelog(s) `oakum version` would refuse to append to"),
        "{stderr}"
    );
    assert_eq!(
        stderr
            .matches("CHANGELOG.md is a symlink whose target does not exist")
            .count(),
        1,
        "{stderr}"
    );
}

/// okm-vf53: a first `0.1.0` a `## <name>@<version>` template wrote is owed a
/// tag, not mistaken for the placeholder it was bumped from.
#[test]
fn a_first_version_a_name_at_version_template_wrote_owes_a_tag() {
    let root = temp_git_repo("template-name-at-first");
    cargo_package(&root, "demo", "0.0.0");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "template = \"## {{ package }}@{{ version }}\\n\\n{{ notes[0] }}\\n\"\n",
    );
    commit(&root, "init");
    fs::write(
        root.join(".changeset/first.md"),
        "---\ndemo: minor\n---\n\nfirst\n",
    )
    .expect("bump file");
    let (code, _, stderr) = support::fixture::oakum_exit(&root, &["version"]);
    assert_eq!(code, Some(0), "{stderr}");
    commit(&root, "version");

    let (ok, _stdout, stderr) = check(&root);

    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("demo (cargo): never released; 0.1.0 has a changelog section"),
        "{stderr}"
    );
}

#[test]
fn a_changelog_titled_changelog_is_ready() {
    let root = temp_git_repo("changelog-ok-title");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\ndemo\n\n## 0.1.0 (2026-09-01)\n\n### Added\n\n- first\n",
    )
    .expect("changelog");
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
fn an_unmanaged_packages_changelog_title_is_not_checked() {
    let root = temp_git_repo("changelog-unmanaged");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("CHANGELOG.md"), "# demo\n").expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(
        ok,
        "an excluded package never gets a version write: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        !stderr.contains("CHANGELOG.md"),
        "an excluded package's changelog is not oakum's concern: {stderr}"
    );
}

fn two_member_workspace(root: &Path, titles: [&str; 2]) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for (name, title) in ["alpha", "beta"].into_iter().zip(titles) {
        let path = root.join(name);
        fs::create_dir_all(path.join("src")).expect("src");
        fs::write(
            path.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
        fs::write(
            path.join("CHANGELOG.md"),
            format!("{title}\n\n## 0.1.0\n\n- first\n"),
        )
        .expect("changelog");
    }
}

#[test]
fn every_foreign_member_changelog_is_named_in_one_refusal() {
    let root = temp_git_repo("changelog-members");
    two_member_workspace(&root, ["# @scope/alpha", "# @scope/beta"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "member changelogs are checked where they live: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("alpha/CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("beta/CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("unverified: 2 changelog(s) `oakum version` would refuse to append to"),
        "both members ride one refusal: {stderr}"
    );
}

#[test]
fn a_foreign_member_changelog_is_found_beside_a_clean_root() {
    let root = temp_git_repo("changelog-one-member");
    two_member_workspace(&root, ["# Changelog", "# @scope/beta"]);
    fs::write(root.join("CHANGELOG.md"), "# Changelog\n").expect("root changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("beta/CHANGELOG.md does not start with"),
        "{stderr}"
    );
    assert!(stderr.contains("unverified: 1 changelog(s)"), "{stderr}");
    assert!(!stderr.contains("alpha/CHANGELOG.md"), "{stderr}");
}

#[test]
fn a_bom_changelog_names_its_own_fix() {
    let root = temp_git_repo("changelog-bom");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("CHANGELOG.md"), "\u{FEFF}# Changelog\n").expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "CHANGELOG.md starts with a UTF-8 BOM; oakum will not splice a changelog it cannot recognize; remove the byte-order mark from the start of the file"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("unverified: 1 changelog(s)"), "{stderr}");
    assert!(
        !stderr.contains("change the first line"),
        "the title fix does not apply to a BOM: {stderr}"
    );
}

#[test]
fn tag_drift_is_still_reported_beside_a_foreign_changelog() {
    let root = temp_git_repo("changelog-and-drift");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.2.0");
    fs::write(root.join("CHANGELOG.md"), "# demo\n").expect("changelog");
    commit(&root, "bump without tag");
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("manifest 0.2.0 is above tagged 0.1.0"),
        "drift is printed even when the changelog also refuses: {stderr}"
    );
    assert!(
        stderr.contains("CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(stderr.contains("unverified: 1 changelog(s)"), "{stderr}");
}

#[test]
fn a_stray_staging_file_is_unverified_and_named() {
    let root = temp_git_repo("staging-file");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".changeset/.one.md.oakum-write.4242.123456.0"),
        "---\ndemo: patch\n---\n",
    )
    .expect("staging file");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a leftover staging file is not a clean tree: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "`.changeset/.one.md.oakum-write.4242.123456.0` is an oakum staging file; if no oakum run is in progress, remove it"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("unverified: 1 oakum staging file(s) left behind"),
        "{stderr}"
    );
    assert!(
        root.join(".changeset/.one.md.oakum-write.4242.123456.0")
            .is_file(),
        "check names the file; it does not sweep it"
    );
}

#[test]
fn every_staging_file_is_named_in_order_with_a_count() {
    let root = temp_git_repo("staging-two");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    for name in [
        ".zeta.md.oakum-write.1.2.0",
        "._config.toml.oakum-write.1.2.0",
    ] {
        fs::write(root.join(".changeset").join(name), "partial").expect("staging");
    }
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    let config_at = stderr
        .find("._config.toml.oakum-write")
        .expect("config line");
    let zeta_at = stderr.find(".zeta.md.oakum-write").expect("zeta line");
    assert!(config_at < zeta_at, "sorted by name: {stderr}");
    assert!(
        stderr.contains("unverified: 2 oakum staging file(s) left behind"),
        "{stderr}"
    );
}

#[test]
fn a_staging_file_beside_a_manifest_is_unverified_too() {
    let root = temp_git_repo("staging-manifest");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".Cargo.toml.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging");
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("`.Cargo.toml.oakum-write.4242.123456.0` is an oakum staging file"),
        "version stages beside the manifest, so check looks there: {stderr}"
    );
}

#[test]
fn a_staging_file_beside_a_declared_extra_file_is_unverified_too() {
    let root = temp_git_repo("staging-extra-file");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join("docs")).expect("docs");
    fs::write(root.join("docs/app.json"), "{\"version\": \"0.1.0\"}\n").expect("extra file");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "[[packages.demo.extra-files]]\npath = \"/docs/app.json\"\nformat = \"json\"\nkey = \"version\"\n",
    );
    fs::write(
        root.join("docs/.app.json.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging");
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("`docs/.app.json.oakum-write.4242.123456.0` is an oakum staging file"),
        "version stages beside a declared extra file, so check looks there: {stderr}"
    );
}

#[test]
fn a_directory_named_like_a_staging_file_is_not_oakums() {
    let root = temp_git_repo("staging-dir");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::create_dir(root.join(".changeset/.one.md.oakum-write.4242.123456.0")).expect("dir");
    commit(&root, "pin");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "the writer only creates regular files: {stderr}");
    assert!(stdout.is_empty() && stderr.is_empty(), "{stdout}{stderr}");
}

#[test]
fn a_staging_file_is_still_named_beside_a_foreign_changelog() {
    let root = temp_git_repo("staging-and-changelog");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("CHANGELOG.md"), "# demo\n").expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".changeset/.one.md.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging file");
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("`.changeset/.one.md.oakum-write.4242.123456.0` is an oakum staging file"),
        "one unverified state does not hide another: {stderr}"
    );
}
