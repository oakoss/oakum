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

/// One tagged package rendering through `template`, or the built-in entry,
/// with `.changeset/fix.md` pending when a note is given.
fn templated(label: &str, template: Option<&str>, note: Option<&str>) -> Fixture {
    let root = temp_git_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        &template.map(template_key).unwrap_or_default(),
    );
    if let Some(note) = note {
        fs::write(
            root.join(".changeset/fix.md"),
            format!("---\ndemo: patch\n---\n\n{note}\n"),
        )
        .expect("bump file");
    }
    root
}

fn template_key(template: &str) -> String {
    format!("template = {}\n", toml_string(template))
}

fn toml_string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/// `check` and `version` on one fixture: `check` first, since `version` writes.
fn check_then_version(root: &Path) -> ((Option<i32>, String), (Option<i32>, String)) {
    let (check_code, _, check_err) = oakum_exit(root, &["check"]);
    let (version_code, _, version_err) = oakum_exit(root, &["version"]);
    ((check_code, check_err), (version_code, version_err))
}

#[test]
fn a_template_with_an_unreadable_heading_is_refused_while_a_bump_file_is_pending() {
    let root = templated(
        "entry-h3-heading",
        Some("### {{ version }}\n\n{{ notes[0] }}\n"),
        Some("fix demo"),
    );
    let ((code, stderr), (version_code, version_err)) = check_then_version(&root);
    let refusal = "the changelog template renders no heading oakum can read back for demo (cargo) 0.1.1; start the section with `## {{ version }}`";
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("1 pending changelog entry(ies) `oakum version` would refuse to write"),
        "{stderr}"
    );
    assert!(stderr.contains(refusal), "{stderr}");
    assert_eq!(version_code, Some(1), "{version_err}");
    assert!(version_err.contains(refusal), "{version_err}");
}

/// The malformed `GITHUB_REPOSITORY` refuses any run that reads `repo` for a
/// template, so a quiet run here is one that read nothing for it.
#[test]
fn a_template_is_not_rendered_without_a_pending_bump_file() {
    let root = templated(
        "entry-h3-idle",
        Some("### {{ version }} {{ repo.url }}\n\n{{ notes[0] }}\n"),
        None,
    );
    commit(&root, "config");
    let out = oakum(&root)
        .arg("check")
        .env("GITHUB_REPOSITORY", "not-a-slug")
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// A cascade carries no notes; only the real plan has one, so a template
/// that writes only when there are notes renders nothing for it.
#[test]
fn a_cascade_under_a_notes_only_template_is_refused() {
    let root = temp_git_repo("entry-empty-cascade");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"core\", \"app\"]\n",
    )
    .expect("workspace");
    for (name, deps) in [
        ("core", ""),
        (
            "app",
            "[dependencies]\ncore = { path = \"../core\", version = \"=0.1.0\" }\n",
        ),
    ] {
        fs::create_dir_all(root.join(name).join("src")).expect("src");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps}"
            ),
        )
        .expect("member");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    git(&root, &["tag", "core@0.1.0"]);
    git(&root, &["tag", "app@0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        &template_key("{% if notes %}## {{ version }}\n\n{{ notes[0] }}\n{% endif %}"),
    );
    fs::write(
        root.join(".changeset/core.md"),
        "---\ncore: minor\n---\n\nadd to core\n",
    )
    .expect("bump file");

    let ((code, stderr), (version_code, version_err)) = check_then_version(&root);
    let refusal = "renders nothing for app (cargo) 0.1.1 (cascade, 0 note(s))";
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(refusal), "{stderr}");
    assert!(!stderr.contains("core (cargo) 0.2.0"), "{stderr}");
    assert_eq!(version_code, Some(1), "{version_err}");
    assert!(version_err.contains(refusal), "{version_err}");
}

/// `repo` is read from the remote, as `version` reads it: the heading fails
/// to render without one and renders on a GitHub remote.
#[test]
fn a_repo_url_heading_renders_on_a_github_remote() {
    let template =
        "## [{{ version }}]({{ repo.url }}/releases/tag/v{{ version }})\n\n{{ notes[0] }}\n";
    let root = templated("entry-repo-url", Some(template), Some("fix demo"));
    let check = |root: &Path| {
        let out = oakum(root)
            .arg("check")
            .env_remove("GITHUB_REPOSITORY")
            .output()
            .expect("check");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    let (code, stderr) = check(&root);
    assert_eq!(code, Some(1), "no remote, no `repo`: {stderr}");
    assert!(stderr.contains("pending changelog entry"), "{stderr}");

    git(
        &root,
        &["remote", "add", "origin", "git@github.com:acme/demo.git"],
    );
    let (code, stderr) = check(&root);
    assert_eq!(code, Some(0), "{stderr}");
    let out = oakum(&root)
        .arg("version")
        .env_remove("GITHUB_REPOSITORY")
        .output()
        .expect("version");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = fs::read_to_string(root.join("CHANGELOG.md")).expect("changelog");
    assert!(
        log.contains("## [0.1.1](https://github.com/acme/demo/releases/tag/v0.1.1)"),
        "{log}"
    );
}

#[test]
fn a_note_leaving_a_pre_block_open_is_refused_naming_the_bump_file() {
    let root = templated("entry-open-pre", None, Some("<pre>\nexample"));
    let refusal = "the changelog entry for demo (cargo) 0.1.1 leaves a `<pre>` block open, which would hide every heading after it; close it in the note of `fix.md`";
    let (_, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let row = parsed["looks"]
        .as_array()
        .and_then(|looks| looks.iter().find(|row| row["look"] == "changelogs"))
        .unwrap_or_else(|| panic!("{document}"));
    assert_eq!(row["outcome"], "error", "{document}");
    assert_eq!(
        row["refusals"][0]["detail"][0], refusal,
        "{document}{stderr}"
    );

    let ((code, stderr), (version_code, version_err)) = check_then_version(&root);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(refusal), "{stderr}");
    assert_eq!(version_code, Some(1), "{version_err}");
    assert!(version_err.contains(refusal), "{version_err}");
}

#[test]
fn a_note_leaving_a_comment_open_is_refused_naming_the_bump_file() {
    let root = templated("entry-open-comment", None, Some("<!-- draft\nexample"));
    let ((code, stderr), (version_code, version_err)) = check_then_version(&root);
    let refusal = "the changelog entry for demo (cargo) 0.1.1 leaves an HTML comment open, which would hide every heading after it; close it in the note of `fix.md`";
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(refusal), "{stderr}");
    assert_eq!(version_code, Some(1), "{version_err}");
    assert!(version_err.contains(refusal), "{version_err}");
}

fn without_github_repository(root: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = oakum(root)
        .args(args)
        .env_remove("GITHUB_REPOSITORY")
        .output()
        .expect("oakum");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
    )
}

const STOOD_IN: &str =
    "changelogs: 1 pending entry(ies) rendered with stand-ins for the commit and pull request";

/// On the branch, the bump file's commit has no `(#N)` yet; the squash merge
/// gives it one, and `version` then writes the entry.
#[test]
fn a_pull_request_the_merge_has_not_made_passes_with_a_stand_in() {
    let root = templated(
        "entry-pr-unsettled",
        Some("## {{ version }}\n{% for c in changes %}\n- {{ c.note }} (#{{ c.pr.number }})\n{% endfor %}"),
        None,
    );
    git(
        &root,
        &["remote", "add", "origin", "git@github.com:acme/demo.git"],
    );
    commit(&root, "config");
    fs::write(
        root.join(".changeset/fix.md"),
        "---\ndemo: patch\n---\n\nfix demo\n",
    )
    .expect("bump file");
    commit(&root, "fix: demo");

    let (code, stderr) = without_github_repository(&root, &["check", "--from", "HEAD~1"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains(STOOD_IN), "{stderr}");
    let (code, json) = without_github_repository(&root, &["check", "--from", "HEAD~1", "--json"]);
    assert_eq!(code, Some(0), "{json}");
    assert!(json.contains("rendered with stand-ins"), "{json}");
    assert!(
        stderr.contains("demo (cargo) 0.1.1: undefined value: `c.pr.number` is undefined"),
        "{stderr}"
    );

    git(
        &root,
        &["commit", "--amend", "--no-verify", "-m", "fix: demo (#12)"],
    );
    let (code, stderr) = without_github_repository(&root, &["version"]);
    assert_eq!(code, Some(0), "{stderr}");
    let log = fs::read_to_string(root.join("CHANGELOG.md")).expect("changelog");
    assert!(log.contains("- fix demo (#12)"), "{log}");
}

#[test]
fn an_uncommitted_bump_file_has_no_commit_yet() {
    let root = templated(
        "entry-commit-unsettled",
        Some("## {{ version }}\n{% for c in changes %}\n- {{ c.note }} ({{ c.commit.short }})\n{% endfor %}"),
        Some("fix demo"),
    );
    let (code, stderr) = without_github_repository(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains(STOOD_IN), "{stderr}");
}

const PR_NUMBER_TEMPLATE: &str =
    "## {{ version }}\n{% for c in changes %}\n- {{ c.note }} (#{{ c.pr.number }})\n{% endfor %}";

/// `main` holding the config, with `fix.md` committed as "fix: demo" on `feat`.
fn pull_request_branch(label: &str) -> Fixture {
    let root = templated(label, Some(PR_NUMBER_TEMPLATE), None);
    git(
        &root,
        &["remote", "add", "origin", "git@github.com:acme/demo.git"],
    );
    commit(&root, "config");
    git(&root, &["checkout", "-q", "-b", "feat"]);
    fs::write(
        root.join(".changeset/fix.md"),
        "---\ndemo: patch\n---\n\nfix demo\n",
    )
    .expect("bump file");
    commit(&root, "fix: demo");
    root
}

/// A merge commit's subject carries no `(#N)`, so the bump file the base now
/// holds keeps its real provenance and `version` refuses the entry.
#[test]
fn a_merged_bump_file_keeps_its_real_provenance() {
    let root = pull_request_branch("entry-pr-merged");
    git(&root, &["checkout", "-q", "main"]);
    git(
        &root,
        &[
            "merge",
            "--no-ff",
            "--no-verify",
            "-m",
            "Merge pull request #12 from x/feat",
            "feat",
        ],
    );
    let (code, stderr) = without_github_repository(&root, &["check", "--strict", "--from", "main"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("undefined value: `c.pr.number` is undefined"),
        "{stderr}"
    );
    assert!(!stderr.contains(STOOD_IN), "{stderr}");
    let (code, stderr) = without_github_repository(&root, &["version"]);
    assert_eq!(code, Some(1), "{stderr}");
}

#[test]
fn a_squashed_bump_file_renders_its_real_pull_request() {
    let root = pull_request_branch("entry-pr-squashed");
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["merge", "--squash", "feat"]);
    git(
        &root,
        &["commit", "-q", "--no-verify", "-m", "fix: demo (#12)"],
    );
    let (code, stderr) = without_github_repository(&root, &["check", "--strict", "--from", "main"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains(STOOD_IN), "{stderr}");
}

/// In a shallow clone git names the graft boundary as the commit that added a
/// bump file below it, so the base's provenance is unread, not refused.
#[test]
fn a_shallow_clone_reads_no_provenance_from_its_graft() {
    let root = pull_request_branch("entry-pr-shallow");
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["merge", "--squash", "feat"]);
    git(
        &root,
        &["commit", "-q", "--no-verify", "-m", "fix: demo (#12)"],
    );
    fs::write(root.join("TIDY.md"), "tidy\n").expect("tidy");
    commit(&root, "chore: tidy");
    git(&root, &["checkout", "-q", "-b", "feat2"]);
    fs::write(root.join("MORE.md"), "more\n").expect("more");
    commit(&root, "docs: more");
    let clone = root.join("shallow");
    let source = format!("file://{}", root.display());
    git(
        &root,
        &[
            "clone",
            "-q",
            "--depth",
            "2",
            "--single-branch",
            "-b",
            "feat2",
            &source,
            clone.to_str().expect("utf-8 path"),
        ],
    );
    git(
        &clone,
        &[
            "fetch",
            "-q",
            "--depth",
            "1",
            "origin",
            "main:refs/remotes/origin/main",
        ],
    );

    let (code, stderr) =
        without_github_repository(&clone, &["check", "--strict", "--from", "origin/main"]);
    assert!(
        !stderr.contains("would refuse to write"),
        "a graft's subject is no pull request: {stderr}"
    );
    assert!(stderr.contains(STOOD_IN), "{stderr}");
    assert_eq!(code, Some(2), "the shallow clone itself: {stderr}");
}

/// The stand-ins lift `c.commit.short`, not the open `<pre>`, so the `<pre>`
/// is what survives the merge.
#[test]
fn a_render_failing_with_stand_ins_names_what_they_do_not_lift() {
    let root = templated(
        "entry-open-pre-unsettled",
        Some("## {{ version }}\n\n{{ notes[0] }}\n{% for c in changes %}\n- {{ c.commit.short }}\n{% endfor %}"),
        Some("<pre>\nleft open"),
    );
    let (code, stderr) = without_github_repository(&root, &["check"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("leaves a `<pre>` block open"), "{stderr}");
    assert!(!stderr.contains(STOOD_IN), "{stderr}");
}

/// Provenance cannot lift a heading oakum cannot read, so the template reading
/// it is still refused outright.
#[test]
fn a_template_refused_whatever_the_merge_records_is_an_error() {
    let root = templated(
        "entry-provenance-unreadable",
        Some("### {{ version }}\n{% for c in changes %}\n- {{ c.note }} (#{{ c.pr.number }})\n{% endfor %}"),
        Some("fix demo"),
    );
    let (code, stderr) = without_github_repository(&root, &["check"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("1 pending changelog entry(ies) `oakum version` would refuse to write"),
        "{stderr}"
    );
    assert!(!stderr.contains(STOOD_IN), "{stderr}");
}

/// A supplied body replaces the notes, so a note `version --notes-file` never
/// renders cannot refuse `check --notes-file`.
#[test]
fn a_supplied_body_replaces_the_notes_check_renders() {
    let root = templated("entry-notes-file", None, Some("<pre>\nexample"));
    fs::write(root.join("NOTES.md"), "supplied body\n").expect("notes");

    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(1), "{stderr}");
    let (code, _, stderr) = oakum_exit(&root, &["check", "--notes-file", "NOTES.md"]);
    assert_eq!(code, Some(0), "{stderr}");
    let (code, _, stderr) = oakum_exit(&root, &["version", "--notes-file", "NOTES.md"]);
    assert_eq!(code, Some(0), "{stderr}");
}
