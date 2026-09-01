//! `oakum upgrade`: the one command exempt from the version gate (ADR-0007).
//! Validates against the old schema, runs migrations, writes the new
//! `tool-version`, regenerates `_schema.json`, and reports what changed.
//! Owns exactly those two files (ADR-0023), and writes nothing when
//! validation or migration fails — a half-migrated config is worse than a
//! stale one.

use semver::Version;

use super::config::{
    contain_template_sources, read_config_source, resolve_sibling_write_target,
    write_file_via_rename,
};
use super::repository;
use super::CliError;

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let Some(source) = read_config_source(&repo)? else {
        return Err(Box::new(CliError::new(
            "nothing to upgrade: `.changeset/_config.toml` does not exist (`oakum init` creates it)",
        )));
    };

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

    let schema_body = oakum::config::schema_json();
    let schema_path = resolve_sibling_write_target(&repo, source.changeset_path(), "_schema.json")?;
    let schema_current = repo
        .dir()
        .read_to_string(&schema_path)
        .is_ok_and(|existing| existing == schema_body);

    if new_config.is_none() && schema_current {
        println!(
            "already at {binary}; `.changeset/_config.toml` and `.changeset/_schema.json` are current"
        );
        return Ok(());
    }

    // Schema first: if the process dies between the two renames, the stale
    // `tool-version` keeps the gate refusing until upgrade re-runs. The
    // reverse order would let commands run against a stale schema.
    if !schema_current {
        write_file_via_rename(repo.dir(), &schema_path, &schema_body)?;
    }
    if let Some(body) = &new_config {
        write_file_via_rename(repo.dir(), source.config_path(), body)?;
    }

    match (&new_config, old_version.cmp(&binary)) {
        (None, _) => println!("tool-version: {binary} (unchanged)"),
        (Some(_), std::cmp::Ordering::Greater) => {
            println!("tool-version: {old_version} -> {binary} (downgrade: this binary is older than the config)");
        }
        (Some(_), _) => println!("tool-version: {old_version} -> {binary}"),
    }
    println!("migrations: none required");
    println!(
        "schema: .changeset/_schema.json {}",
        if schema_current {
            "unchanged"
        } else {
            "regenerated"
        }
    );
    Ok(())
}
