//! `oakum upgrade`: gated on the install pin like the writers, except that a
//! repository with no pin may still upgrade (ADR-0038). Validates against the
//! old schema, runs migrations, writes the new `tool-version`, regenerates
//! `_schema.json`, and reports what changed.
//! Owns exactly those two files (ADR-0023), and writes nothing when
//! validation or migration fails — a half-migrated config is worse than a
//! stale one.

use semver::Version;

use super::config::{contain_template_sources, read_config_source, resolve_sibling_write_target};
use super::fs::write_file_via_rename;
use super::install_pin::{self, PinPolicy};
use super::owned_files::{schema_state, write_schema, Records, SchemaOutcome};
use super::repository;
use super::CliError;
use super::{say_err, say_out};

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let Some(source) = read_config_source(&repo)? else {
        return Err(Box::new(CliError::new(
            "nothing to upgrade: `.changeset/_config.toml` does not exist (`oakum init` creates it)",
        )));
    };
    // A missing pin lets upgrade run, but the run must not read as checked against one.
    if let Some(owed) = install_pin::gate(&repo, PinPolicy::Upgrade)? {
        say_err(&format!("unverified: {}", owed.detail()));
    }

    let old = oakum::config::parse(source.text()).map_err(|err| {
        CliError::new(format!(
            "`.changeset/_config.toml` failed validation, so nothing was written: {err}"
        ))
    })?;
    contain_template_sources(&repo, &old).map_err(|err| {
        CliError::new(format!(
            "`.changeset/_config.toml` failed validation, so nothing was written: {err}"
        ))
    })?;
    let old_version = old
        .tool_version()
        .cloned()
        .ok_or_else(|| CliError::new("`.changeset/_config.toml` carries no `tool-version`"))?;
    let binary = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|err| CliError::new(format!("this binary reports a non-semver version: {err}")))?;

    // Migration registry: none exist yet. When a release changes the config
    // grammar, its migration rewrites the raw text under its own rules —
    // necessarily before the strict parse can accept an old grammar, so the
    // validation above moves with it. A failed migration leaves both files
    // untouched.
    let migrated_text = source.text().to_owned();

    let new_config = if old_version == binary {
        None
    } else {
        Some(
            oakum::config::set_tool_version(&migrated_text, &binary).map_err(|err| {
                CliError::new(format!(
                    "failed to rewrite `tool-version`, so nothing was written: {err}"
                ))
            })?,
        )
    };

    let schema_path = resolve_sibling_write_target(&repo, source.changeset_path(), "_schema.json")?;
    // Schema first, so an interrupted run never leaves a new `tool-version`
    // beside a stale schema; a rerun finishes the config.
    let schema = write_schema(
        repo.dir(),
        &schema_path,
        schema_state(repo.dir(), &schema_path),
    )?;
    if new_config.is_none() && schema == SchemaOutcome::Unchanged {
        say_out(&format!(
            "already at {binary}; `.changeset/_config.toml` and `.changeset/_schema.json` are current"
        ));
        return Ok(());
    }
    if let Some(body) = &new_config {
        write_file_via_rename(repo.dir(), source.config_path(), body).map_err(|err| {
            CliError::new(match schema {
                SchemaOutcome::Unchanged => err.to_string(),
                SchemaOutcome::Created | SchemaOutcome::Replaced => {
                    format!("{err}; `.changeset/_schema.json` was regenerated first")
                }
            })
        })?;
    }

    let mut records = Records::default();
    records.record(&match (&new_config, old_version.cmp(&binary)) {
        (None, _) => format!("tool-version: {binary} (unchanged)"),
        (Some(_), std::cmp::Ordering::Greater) => format!(
            "tool-version: {old_version} -> {binary} (downgrade: this binary is older than the config)"
        ),
        (Some(_), _) => format!("tool-version: {old_version} -> {binary}"),
    });
    records.record("migrations: none required");
    records.record(&format!(
        "schema: .changeset/_schema.json {}",
        match schema {
            SchemaOutcome::Unchanged => "unchanged",
            SchemaOutcome::Created | SchemaOutcome::Replaced => "regenerated",
        }
    ));
    records.finish()?;
    Ok(())
}
