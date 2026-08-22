//! Append to `GITHUB_OUTPUT` when GitHub Actions has set it (ADR-0013).

use std::env;
use std::fs::OpenOptions;
use std::io::Write;

use super::CliError;

const DELIMITER: &str = "OAKUM_JSON";

/// No-op when `GITHUB_OUTPUT` is unset (local runs).
pub(super) fn write_json(json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = env::var_os("GITHUB_OUTPUT") else {
        return Ok(());
    };
    if json.contains(DELIMITER) {
        return Err(Box::new(CliError::new(
            "release-state JSON contains the GITHUB_OUTPUT delimiter",
        )));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    write!(file, "json<<{DELIMITER}\n{json}\n{DELIMITER}\n")?;
    Ok(())
}
