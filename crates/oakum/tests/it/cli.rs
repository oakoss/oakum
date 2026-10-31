use std::process::Command;

// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests; it resolves to the
// path of the built binary.
fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_oakum"))
        .args(args)
        .arg("--help")
        .output()
        .expect("binary should be runnable");
    assert!(output.status.success(), "{args:?}: {}", output.status);
    String::from_utf8(output.stdout).expect("utf-8 help")
}

fn from_line(help: &str) -> &str {
    help.lines()
        .find(|line| line.trim_start().starts_with("--from <REF>"))
        .and_then(|line| line.split_once("<REF>"))
        .map_or("", |(_, text)| text.trim())
}

/// A user with only the binary cannot look up an ADR, and a default stated in
/// one place cannot disagree with another.
#[test]
fn help_names_no_adr_and_states_one_from_default() {
    let commands: [&[&str]; 12] = [
        &[],
        &["init"],
        &["add"],
        &["generate"],
        &["status"],
        &["check"],
        &["version"],
        &["release"],
        &["ci", "version-pr"],
        &["ci", "pr-status"],
        &["migrate"],
        &["upgrade"],
    ];
    for args in commands {
        let text = help(args);
        assert!(!text.contains("ADR-"), "{args:?}: {text}");
    }
    let planned = from_line(&help(&["status"])).to_owned();
    assert!(
        planned.contains("each package oakum tags starts at its highest reachable tag"),
        "{planned}"
    );
    for args in [&["check"][..], &["ci", "pr-status"]] {
        assert_eq!(from_line(&help(args)), planned, "{args:?}");
    }
    // `release` reads a base only for its coverage look.
    assert_eq!(
        from_line(&help(&["release"])),
        from_line(&help(&["generate"]))
    );
    // These read a base only for commit-derived intent.
    let intent = from_line(&help(&["version"])).to_owned();
    assert!(intent.contains("unused with bump files"), "{intent}");
    assert_eq!(from_line(&help(&["ci", "version-pr"])), intent);
    assert!(from_line(&help(&["generate"])).contains("merge-base with `origin/main`"));
    let check = help(&["check"]);
    assert!(check.contains("coverage and notes looks"), "{check}");
}

#[test]
fn bare_oakum_prints_help_in_workflow_order_and_exits_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_oakum"))
        .output()
        .expect("binary should be runnable");

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let help = String::from_utf8_lossy(&output.stderr);
    let listed: Vec<&str> = help
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        listed,
        [
            "init", "add", "generate", "status", "check", "version", "release", "ci", "migrate",
            "upgrade", "help"
        ],
        "{help}"
    );
    assert!(help.contains("Typical flow: `oakum init`"), "{help}");
    assert!(
        help.contains("https://github.com/oakoss/oakum/tree/main/docs/guide"),
        "{help}"
    );
}
