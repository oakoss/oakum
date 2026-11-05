use super::*;

/// A clean, pinned repository whose config opens with `directive` and whose
/// `.changeset/_schema.json` holds `schema`, or is absent.
fn schema_fixture(label: &str, directive: &str, schema: Option<&str>) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, directive);
    if let Some(bytes) = schema {
        fs::write(root.join(".changeset/_schema.json"), bytes).expect("schema");
    }
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    root
}

const NAMES_SCHEMA: &str = "#:schema ./_schema.json\n";

fn schema_row(root: &Path) -> serde_json::Value {
    let (code, document, stderr) = oakum_exit(root, &["check", "--json"]);
    assert_eq!(code, Some(0), "{stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "schema")
        .unwrap_or_else(|| panic!("a schema row: {document}"))
        .clone()
}

/// A stale schema degrades editor hints only, so it is a note naming the fix
/// and never an exit code — `--strict` included, which gates other looks.
#[test]
fn a_stale_schema_is_a_note_that_leaves_the_exit_code_alone() {
    let current = schema_fixture(
        "schema-current-exit",
        NAMES_SCHEMA,
        Some(&oakum::config::schema_json()),
    );
    let stale = schema_fixture("schema-stale-exit", NAMES_SCHEMA, Some("{}\n"));
    for args in [&["check"][..], &["check", "--strict"][..]] {
        let (current_code, _, current_err) = oakum_exit(&current, args);
        let (stale_code, _, stale_err) = oakum_exit(&stale, args);
        assert_eq!(current_code, Some(0), "{args:?}: {current_err}");
        assert_eq!(stale_code, current_code, "{args:?}: {stale_err}");
        assert!(
            stale_err.contains(&format!(
                "`.changeset/_schema.json` differs from what oakum {BINARY_VERSION} generates"
            )) && stale_err.contains("; run `oakum upgrade`"),
            "{args:?}: {stale_err}"
        );
        assert!(
            !current_err.contains("_schema.json"),
            "{args:?}: {current_err}"
        );
    }
}

/// The directive is what makes a missing file worth a word: the config asks
/// taplo for a schema that is not there.
#[test]
fn a_missing_schema_the_directive_names_is_a_note() {
    let root = schema_fixture("schema-missing-named", NAMES_SCHEMA, None);
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("`.changeset/_schema.json` is missing")
            && stderr.contains("; run `oakum upgrade`"),
        "{stderr}"
    );
}

/// With nothing pinning oakum, `upgrade` still runs, so the note names it
/// directly rather than asking for pins to be settled first.
#[test]
fn a_stale_schema_with_no_pin_names_upgrade_directly() {
    let root = schema_fixture("schema-stale-no-pin", NAMES_SCHEMA, Some("{}\n"));
    fs::remove_file(root.join(".github/workflows/release.yml")).expect("drop the pin");
    commit(&root, "drop the pin");
    let (_, _, stderr) = oakum_exit(&root, &["check"]);
    assert!(
        stderr.contains("differs from what oakum")
            && stderr.contains("; run `oakum upgrade`")
            && !stderr.contains("settle the install pins"),
        "{stderr}"
    );
}

/// With no directive naming it, a schema nobody committed is not missing: the
/// look found nothing to compare, and says nothing.
#[test]
fn an_absent_schema_the_config_does_not_name_hears_nothing() {
    for (label, directive) in [
        ("schema-no-directive", ""),
        (
            "schema-node-modules",
            "#:schema ../node_modules/@oakoss/oakum/schema.json\n",
        ),
    ] {
        let root = schema_fixture(label, directive, None);
        let (code, _, stderr) = oakum_exit(&root, &["check"]);
        assert_eq!(code, Some(0), "{label}: {stderr}");
        assert!(!stderr.contains("_schema.json"), "{label}: {stderr}");
        assert_eq!(schema_row(&root)["outcome"], "ok", "{label}");
    }
}

/// A schema behind a symlink that leaves the repository is one `check` did not
/// read, which must not read as one it found current.
#[cfg(unix)]
#[test]
fn a_schema_that_cannot_be_read_is_not_reported_as_current() {
    let current = schema_fixture(
        "schema-current-unread",
        NAMES_SCHEMA,
        Some(&oakum::config::schema_json()),
    );
    let root = schema_fixture("schema-escaping", NAMES_SCHEMA, None);
    let outside = sibling(&root, "outside-schema.json");
    fs::write(&outside, oakum::config::schema_json()).expect("outside schema");
    std::os::unix::fs::symlink(&outside, root.join(".changeset/_schema.json")).expect("symlink");

    let (code, _, stderr) = oakum_exit(&root, &["check", "--strict"]);
    assert_eq!(code, Some(0), "a note, not a refusal: {stderr}");
    assert!(
        stderr.contains("`.changeset/_schema.json` could not be read"),
        "{stderr}"
    );
    let unread = schema_row(&root);
    assert_eq!(unread["outcome"], "reported", "{unread}");
    assert_ne!(unread, schema_row(&current));
}

/// The schema is read where the config was: through a `.changeset` that is an
/// absolute symlink inside the repository, which the config loader follows.
#[cfg(unix)]
#[test]
fn a_schema_beside_a_symlinked_config_is_read_where_the_config_was() {
    let root = temp_git_repo("schema-symlinked-changeset");
    fs::create_dir_all(root.join("real-changeset")).expect("real dir");
    fs::write(root.join("real-changeset/_config.toml"), NAMES_SCHEMA).expect("config");
    fs::write(
        root.join("real-changeset/_schema.json"),
        oakum::config::schema_json(),
    )
    .expect("schema");
    std::os::unix::fs::symlink(root.join("real-changeset"), root.join(".changeset"))
        .expect("symlink");
    write_install_pin(&root, BINARY_VERSION);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);

    // The staging look refuses this layout on its own, so the exit code is not
    // this look's to assert.
    let (_, stdout, stderr) = oakum_exit(&root, &["check"]);
    assert!(
        stdout.contains("looking at management, tags, install pin, schema"),
        "the config loaded and the look ran: {stdout}"
    );
    assert!(!stderr.contains("_schema.json"), "{stderr}");
}

/// `upgrade` keeps the file whatever the directive says, so an existing one is
/// compared either way; the directive decides only whether a missing one is.
#[test]
fn an_existing_schema_is_compared_without_a_directive() {
    let root = schema_fixture("schema-stale-unnamed", "", Some("{}\n"));
    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("`.changeset/_schema.json` differs from what oakum"),
        "{stderr}"
    );
    assert_eq!(schema_row(&root)["outcome"], "reported");
}

/// Pins that agree on another version mean this binary's schema is not the
/// one the repository expects, and `oakum upgrade` from this binary refuses.
/// So the look says it did not compare, even over bytes this binary would
/// call current.
#[test]
fn a_pin_naming_another_version_leaves_the_schema_uncompared() {
    for (label, schema) in [
        ("schema-pin-other-stale", String::from("{}\n")),
        ("schema-pin-other-current", oakum::config::schema_json()),
    ] {
        let root = schema_fixture(label, NAMES_SCHEMA, Some(&schema));
        write_install_pin(&root, MISMATCHED_PIN);
        commit(&root, "pin another version");
        let (code, _, stderr) = oakum_exit(&root, &["check"]);
        assert_eq!(code, Some(0), "{label}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "`.changeset/_schema.json` was not compared: this binary is oakum {BINARY_VERSION} and the install pin is {MISMATCHED_PIN}"
            )),
            "{label}: {stderr}"
        );
        assert!(!stderr.contains("run `oakum upgrade`"), "{label}: {stderr}");
        assert_eq!(schema_row(&root)["outcome"], "reported", "{label}");
    }
}

/// Unsettled pins leave this binary as the only one to compare against, but
/// `oakum upgrade` refuses until they settle, so the remedy says so. The exit
/// code is the install-pin look's, the same with a current schema.
fn assert_unsettled_pins_compare_and_settle_first(label: &str, unsettle: fn(&Path)) {
    let current = schema_fixture(
        &format!("{label}-current"),
        NAMES_SCHEMA,
        Some(&oakum::config::schema_json()),
    );
    let stale = schema_fixture(&format!("{label}-stale"), NAMES_SCHEMA, Some("{}\n"));
    unsettle(&current);
    unsettle(&stale);
    let (current_code, _, current_err) = oakum_exit(&current, &["check"]);
    let (code, _, stderr) = oakum_exit(&stale, &["check"]);
    assert_eq!(code, current_code, "{label}: {stderr}");
    assert!(
        !current_err.contains("_schema.json"),
        "{label}: {current_err}"
    );
    assert!(
        stderr.contains("`.changeset/_schema.json` differs from what oakum"),
        "{label}: {stderr}"
    );
    assert!(
        stderr.contains("settle the install pins first"),
        "{label}: {stderr}"
    );
    assert!(
        !stderr.contains("; run `oakum upgrade`"),
        "{label}: {stderr}"
    );
    assert!(!stderr.contains("was not compared"), "{label}: {stderr}");
}

#[test]
fn disagreeing_pins_compare_and_name_settling_them_first() {
    assert_unsettled_pins_compare_and_settle_first("schema-pins-disagree", write_stale_pin);
}

#[cfg(unix)]
#[test]
fn an_unreadable_pin_compares_and_names_settling_it_first() {
    assert_unsettled_pins_compare_and_settle_first("schema-pin-dangling", |root| {
        std::os::unix::fs::symlink("no-such-mise.toml", root.join(".mise.toml"))
            .expect("dangling symlink");
    });
}

/// A FIFO opened for reading blocks until a writer appears; the look must say
/// it could not read one rather than wait.
#[cfg(unix)]
#[test]
fn a_fifo_schema_is_unread_rather_than_a_hang() {
    let root = schema_fixture("schema-fifo", NAMES_SCHEMA, None);
    let made = Command::new("mkfifo")
        .arg(root.join(".changeset/_schema.json"))
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo: {made}");

    let output = output_within(
        oakum(&root).arg("check"),
        std::time::Duration::from_secs(10),
    )
    .expect("check blocked on a FIFO schema");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("`.changeset/_schema.json` could not be read")
            && stderr.contains("not a regular file"),
        "{stderr}"
    );
}

/// A directory where the schema belongs resolves inside the repository and
/// fails only at the read, which is the half the symlink test never reaches.
#[test]
fn a_schema_that_is_a_directory_is_unread_not_current() {
    let root = schema_fixture("schema-directory", NAMES_SCHEMA, None);
    fs::create_dir(root.join(".changeset/_schema.json")).expect("directory");
    fs::write(root.join(".changeset/_schema.json/keep"), "").expect("keep");
    commit(&root, "schema is a directory");

    let (code, _, stderr) = oakum_exit(&root, &["check"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("`.changeset/_schema.json` could not be read"),
        "{stderr}"
    );
    assert_eq!(schema_row(&root)["outcome"], "reported");
}

#[test]
fn the_document_reports_a_stale_schema_and_passes_a_current_one() {
    let stale = schema_fixture("schema-json-stale", NAMES_SCHEMA, Some("{}\n"));
    let row = schema_row(&stale);
    assert_eq!(row["outcome"], "reported", "{row}");
    assert!(
        row["reports"][0]
            .as_str()
            .is_some_and(|line| line.contains("run `oakum upgrade`")),
        "{row}"
    );
    assert!(row["refusals"].is_null(), "{row}");

    let current = schema_fixture(
        "schema-json-current",
        NAMES_SCHEMA,
        Some(&oakum::config::schema_json()),
    );
    let row = schema_row(&current);
    assert_eq!(row["outcome"], "ok", "{row}");
    assert!(row["reports"].is_null(), "{row}");
}
