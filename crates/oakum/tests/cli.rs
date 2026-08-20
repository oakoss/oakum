// Spawning the binary is the point of an integration test, so this file opts
// out of the denylist in clippy.toml. It is not an ADR-0002 boundary marker;
// that trigger scopes to attributes under src/.
#![allow(clippy::disallowed_methods)]

use std::process::Command;

// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests; it resolves to the
// path of the built binary.
#[test]
fn binary_runs_and_identifies_itself() {
    let output = Command::new(env!("CARGO_BIN_EXE_oakum"))
        .output()
        .expect("binary should be runnable");

    assert!(output.status.success(), "exited with {}", output.status);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "oakum");
}
