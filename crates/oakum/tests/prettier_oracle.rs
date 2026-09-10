//! Real Prettier over the files oakum generates that a repository's
//! formatter will see: `_schema.json` and the bundled `.changeset/README.md`.
//! ADR-0031 promises they survive unchanged; this is the gate, the same way
//! `changeset_foreign_parsers` gates ADR-0005 with the real `@changesets/parse`.
#![allow(clippy::disallowed_methods)]

mod support;

use std::io::Write;
use std::process::Stdio;

use support::changeset_foreign::js_runtime_dir;

const README: &str = include_str!("../src/cli/changeset-readme.md");

/// `text` as Prettier 3 prints it for a file named `name`, with defaults.
fn prettier(name: &str, text: &str) -> String {
    let runtime = js_runtime_dir();
    let script = runtime.join("node_modules/prettier/bin/prettier.cjs");
    assert!(
        script.is_file(),
        "{} missing; the fixture install is incomplete",
        script.display()
    );
    let mut child = support::command_on_path("node")
        .arg(&script)
        .args(["--stdin-filepath", name])
        .current_dir(&runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn node prettier");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("prettier");
    assert!(
        output.status.success(),
        "prettier on {name} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("prettier prints UTF-8")
}

#[test]
fn prettier_leaves_the_generated_schema_unchanged() {
    let schema = oakum::config::schema_json();
    assert_eq!(prettier("_schema.json", &schema), schema);
}

#[test]
fn prettier_leaves_the_bundled_changeset_readme_unchanged() {
    assert_eq!(prettier("README.md", README), README);
}
