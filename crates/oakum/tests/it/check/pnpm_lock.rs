use super::*;

use crate::pnpm_lockfile::{nested_members, pnpm, text, workspace, workspace_with_yaml, MEMBERS};

const CATALOG: &str = "catalog:\n  '@oakum-fixture/a': ^1.0.0\n";

/// `check --json`'s exit code, its `lockfiles` row, and stderr.
fn lockfiles(dir: &Path) -> (Option<i32>, serde_json::Value, String) {
    let (code, document, stderr) = oakum_exit(dir, &["check", "--json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}\n{stderr}"));
    let row = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "lockfiles")
        .unwrap_or_else(|| panic!("no lockfiles row: {document}"))
        .clone();
    (code, row, stderr)
}

fn assert_silent(code: Option<i32>, row: &serde_json::Value, stderr: &str) {
    assert_eq!(row["outcome"], "ok", "{row}\n{stderr}");
    assert!(row["refusals"].is_null(), "{row}");
    assert!(row["reports"].is_null(), "{row}");
    assert_eq!(code, Some(0), "{row}\n{stderr}");
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).expect("read");
    assert!(
        text.contains(from),
        "{from} not in {}:\n{text}",
        path.display()
    );
    fs::write(path, text.replacen(from, to, 1)).expect("write");
}

/// The refusal decides the run, so the exit code is this look's and not one an
/// unrelated look produced. `expected` holds one set of parts per drifted entry.
fn assert_drift(code: Option<i32>, row: &serde_json::Value, stderr: &str, expected: &[&[&str]]) {
    assert_eq!(code, Some(1), "{row}\n{stderr}");
    assert_eq!(row["outcome"], "error", "{row}");
    let refusal = &row["refusals"][0];
    assert_eq!(refusal["deciding"], true, "{row}");
    assert!(
        refusal["summary"]
            .as_str()
            .expect("summary")
            .contains("pnpm install"),
        "the fix is named: {row}"
    );
    let detail: Vec<&str> = refusal["detail"]
        .as_array()
        .expect("detail")
        .iter()
        .map(|line| line.as_str().expect("a line"))
        .collect();
    assert_eq!(detail.len(), expected.len(), "{row}");
    for (line, parts) in detail.iter().zip(expected) {
        for part in *parts {
            assert!(line.contains(part), "{part} not in {line}");
        }
    }
    assert!(stderr.contains("pnpm install"), "{stderr}");
}

/// Under `js`, so each manifest is read relative to the lockfile's directory,
/// one of them from outside it.
#[test]
fn a_range_edited_without_pnpm_install_is_named() {
    let members = nested_members();
    let (root, dir) = workspace("check-range", "js", &["packages/*", "../tools/*"], &members);
    for member in ["packages/b", "../tools/e"] {
        edit(
            &dir.join(member).join("package.json"),
            "\"workspace:^1.0.0\"",
            "\"workspace:*\"",
        );
    }
    commit(&root, "edit two ranges");

    let (code, row, stderr) = lockfiles(&dir);
    assert_drift(
        code,
        &row,
        &stderr,
        &[
            &[
                "`importers/../tools/e/dependencies/@oakum-fixture/a`",
                "`workspace:^1.0.0`",
                "`workspace:*`",
            ],
            &[
                "`importers/packages/b/dependencies/@oakum-fixture/a`",
                "`workspace:^1.0.0`",
                "`workspace:*`",
            ],
        ],
    );
}

/// pnpm refuses a frozen install for a member with dependencies and no
/// importer, so the look reads the workspace's members as well as the lock's.
#[test]
fn a_member_the_lock_does_not_record_is_named() {
    let (root, dir) = workspace("check-member", "", &["packages/*"], &MEMBERS);
    fs::create_dir_all(dir.join("packages/n")).expect("member dir");
    fs::write(
        dir.join("packages/n/package.json"),
        "{\n  \"name\": \"@oakum-fixture/n\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"workspace:*\"\n  }\n}\n",
    )
    .expect("member manifest");
    commit(&root, "add a member");
    git(&root, &["tag", "-a", "@oakum-fixture/n/v1.0.0", "-m", "n"]);

    let (code, row, stderr) = lockfiles(&dir);
    assert_drift(
        code,
        &row,
        &stderr,
        &[&[
            "`packages/n/package.json` declares `workspace:*`",
            "`importers/packages/n/dependencies/@oakum-fixture/a`",
        ]],
    );
}

#[test]
fn a_catalog_entry_changed_without_pnpm_install_is_named() {
    let (root, dir) = workspace("check-catalog", "", &["packages/*"], &MEMBERS);
    edit(
        &dir.join("pnpm-workspace.yaml"),
        "'@oakum-fixture/a': ^1.0.0",
        "'@oakum-fixture/a': ~1.0.0",
    );
    commit(&root, "edit the catalog");

    let (code, row, stderr) = lockfiles(&dir);
    assert_drift(
        code,
        &row,
        &stderr,
        &[&[
            "`catalogs/default/@oakum-fixture/a`",
            "`^1.0.0`",
            "`~1.0.0`",
        ]],
    );
}

/// Every shape pnpm records apart from the manifest's own spelling: a member
/// outside the lock dir, auto-installed and shadowed peers, one key in two
/// sections, a catalog alias.
#[test]
fn a_lock_pnpm_wrote_reads_clean() {
    let members = nested_members();
    let (_root, dir) = workspace("check-clean", "js", &["packages/*", "../tools/*"], &members);
    let (code, row, stderr) = lockfiles(&dir);
    assert_silent(code, &row, &stderr);
}

/// pnpm records an overridden dependency's specifier after the override, and
/// a frozen install accepts that lockfile.
#[test]
fn an_overridden_dependency_is_skipped_and_counted() {
    let yaml = format!("{CATALOG}overrides:\n  '@oakum-fixture/a': workspace:*\n");
    let (_root, dir) = workspace_with_yaml("check-override", "", &["packages/*"], &MEMBERS, &yaml);
    let frozen = pnpm(&dir, &["install", "--offline", "--frozen-lockfile"]);
    assert!(frozen.status.success(), "{}", text(&frozen));
    let (code, row, stderr) = lockfiles(&dir);
    assert_eq!(code, Some(0), "{row}\n{stderr}");
    assert_eq!(row["outcome"], "reported", "a skip is never silent: {row}");
    assert!(row["refusals"].is_null(), "{row}");
    let line = row["reports"][0].as_str().expect("a report");
    assert!(
        line.contains("compared importer and catalog specifiers; ")
            && line.contains("under overrides")
            && line.ends_with(" skipped"),
        "{line}"
    );
    assert!(stderr.contains(line), "{stderr}");
}

/// pnpm lists an override whose parent is another package, but `b`'s
/// specifier stays as declared, so editing it is drift a frozen install
/// refuses.
#[test]
fn an_override_pnpm_did_not_apply_still_compares() {
    let yaml = format!("{CATALOG}overrides:\n  '@oakum-fixture/c>@oakum-fixture/a': workspace:*\n");
    let (root, dir) = workspace_with_yaml("check-unapplied", "", &["packages/*"], &MEMBERS, &yaml);
    let lock = fs::read_to_string(dir.join("pnpm-lock.yaml")).expect("lock");
    assert!(
        lock.contains("'@oakum-fixture/c>@oakum-fixture/a'"),
        "{lock}"
    );
    edit(
        &dir.join("packages/b/package.json"),
        "\"workspace:^1.0.0\"",
        "\"workspace:~1.0.0\"",
    );
    commit(&root, "edit a range");
    let (code, row, stderr) = lockfiles(&dir);
    assert_drift(
        code,
        &row,
        &stderr,
        &[&["`importers/packages/b/dependencies/@oakum-fixture/a`"]],
    );
}

/// A `readPackage` hook can rewrite any specifier, so nothing was compared.
#[test]
fn a_pnpmfile_is_unverified() {
    let (root, dir) = workspace("check-pnpmfile", "", &["packages/*"], &MEMBERS);
    fs::write(
        dir.join(".pnpmfile.cjs"),
        "module.exports = { hooks: { readPackage(pkg) { return pkg } } }\n",
    )
    .expect("pnpmfile");
    let install = pnpm(&dir, &["install", "--offline", "--no-frozen-lockfile"]);
    assert!(install.status.success(), "{}", text(&install));
    commit(&root, "add a pnpmfile");
    let (code, row, stderr) = lockfiles(&dir);
    assert_eq!(code, Some(2), "{row}\n{stderr}");
    assert_eq!(row["outcome"], "unverified", "{row}");
    assert!(
        row["refusals"][0]["summary"]
            .as_str()
            .expect("summary")
            .contains("pnpmfile"),
        "{row}"
    );
}

/// An unversioned member is a workspace project all the same; pnpm refuses a
/// frozen install without its importer.
#[test]
fn a_private_member_the_lock_does_not_record_is_named() {
    let (root, dir) = workspace("check-private", "", &["packages/*"], &MEMBERS);
    fs::create_dir_all(dir.join("packages/p")).expect("member dir");
    fs::write(
        dir.join("packages/p/package.json"),
        "{\n  \"name\": \"@oakum-fixture/p\",\n  \"private\": true,\n  \"dependencies\": {\n    \"@oakum-fixture/a\": \"workspace:*\"\n  }\n}\n",
    )
    .expect("member manifest");
    commit(&root, "add a private member");
    let (code, row, stderr) = lockfiles(&dir);
    assert_drift(
        code,
        &row,
        &stderr,
        &[&["`importers/packages/p/dependencies/@oakum-fixture/a`"]],
    );
}

/// A member the globs no longer include keeps its importer, which a frozen
/// install ignores however its manifest changes.
#[test]
fn a_member_left_out_of_the_globs_is_not_compared() {
    let (root, dir) = workspace("check-excluded", "", &["packages/*"], &MEMBERS);
    edit(
        &dir.join("pnpm-workspace.yaml"),
        "  - 'packages/*'\n",
        "  - 'packages/*'\n  - '!packages/b'\n",
    );
    edit(
        &dir.join("packages/b/package.json"),
        "\"workspace:^1.0.0\"",
        "\"workspace:*\"",
    );
    commit(&root, "drop b from the workspace");
    let frozen = pnpm(&dir, &["install", "--offline", "--frozen-lockfile"]);
    assert!(frozen.status.success(), "{}", text(&frozen));
    let (code, row, stderr) = lockfiles(&dir);
    assert_silent(code, &row, &stderr);
}

/// pnpm 12 installs from `lockfileVersion: '9.1'`, a format oakum has not
/// measured, so the look did not happen; the same tree at `'9.0'` reads clean.
#[test]
fn an_unmeasured_lockfile_version_is_unverified() {
    let (_root, dir) = workspace("check-format", "", &["packages/*"], &MEMBERS);
    let (code, row, stderr) = lockfiles(&dir);
    assert_silent(code, &row, &stderr);

    edit(
        &dir.join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'",
        "lockfileVersion: '9.1'",
    );
    let (code, row, stderr) = lockfiles(&dir);
    assert_eq!(code, Some(2), "{row}\n{stderr}");
    assert_eq!(row["outcome"], "unverified", "{row}");
    assert!(
        row["refusals"][0]["summary"]
            .as_str()
            .expect("summary")
            .contains("9.1"),
        "{row}"
    );
}

/// A manifest the look cannot read is one it did not compare, so it cannot
/// read as clean. `pnpm list` accepts a range that is not a string.
#[test]
fn a_manifest_it_cannot_read_is_unverified() {
    let (_root, dir) = workspace("check-unread", "", &["packages/*"], &MEMBERS);
    edit(
        &dir.join("package.json"),
        "\"private\": true",
        "\"private\": true,\n  \"devDependencies\": {\n    \"x\": 1\n  }",
    );
    let (code, row, stderr) = lockfiles(&dir);
    assert_eq!(code, Some(2), "{row}\n{stderr}");
    assert_eq!(row["outcome"], "unverified", "{row}");
    let detail = row["refusals"][0]["detail"][0].as_str().unwrap_or_default();
    assert!(
        detail.starts_with(
            "`package.json` does not parse as a manifest pnpm reads (invalid type: integer `1`"
        ),
        "{row}"
    );
}

#[test]
fn a_repository_without_a_pnpm_lock_says_nothing() {
    // A stray lockfile no pnpm workspace owns is not read.
    let root = temp_git_repo("lockfiles-cargo");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '5.4'\n").expect("lock");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (code, row, stderr) = lockfiles(&root);
    assert_silent(code, &row, &stderr);

    let (root, dir) = workspace("check-unlocked", "", &["packages/*"], &MEMBERS);
    fs::remove_file(dir.join("pnpm-lock.yaml")).expect("remove lock");
    commit(&root, "drop the lock");
    let (code, row, stderr) = lockfiles(&dir);
    assert_silent(code, &row, &stderr);
}
