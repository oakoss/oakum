//! `oakum upgrade`: the one command exempt from the version gate (ADR-0007).
//! Validates against the old schema, runs migrations, writes the new
//! `tool-version`, regenerates `_schema.json`, and reports what changed.
//! Owns exactly those two files (ADR-0023), and writes nothing when
//! validation or migration fails — a half-migrated config is worse than a
//! stale one.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cap_std::fs::{Dir, OpenOptions};
use semver::Version;

use super::config::{read_config_source, resolve_sibling_write_target};
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
        .ok()
        .is_some_and(|existing| existing == schema_body);

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
        write_via_rename(repo.dir(), &schema_path, &schema_body)?;
    }
    if let Some(body) = &new_config {
        write_via_rename(repo.dir(), source.config_path(), body)?;
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

/// Temp-file-plus-rename beside the resolved target: same directory means
/// same filesystem, so the rename cannot hit EXDEV when an internal symlink
/// resolves onto another mount. The staging file is created with
/// `create_new`, which never follows a pre-existing entry — a committed
/// symlink at the staging path cannot redirect the write onto a file
/// upgrade does not own.
fn write_via_rename(
    dir: &Dir,
    target: &Path,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::new("upgrade write target has no file name"))?;
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    // Invocation-unique staging: on a name collision, pick another name
    // rather than removing the entry — upgrade never deletes or writes
    // through a path it did not create in this invocation, so concurrent
    // runs (pid namespaces included) and committed look-alike entries are
    // both safe. Crashed runs can orphan a staging dotfile; sweeping those
    // would reintroduce the race.
    let mut attempt: u32 = 0;
    let (tmp, mut staged) = loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let candidate = parent.join(format!(
            ".{file_name}.oakum-upgrade.{}.{nanos}.{attempt}",
            std::process::id()
        ));
        match dir.open_with(&candidate, OpenOptions::new().create_new(true).write(true)) {
            Ok(file) => break (candidate, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(err) => {
                return Err(Box::new(CliError::new(format!(
                    "failed to stage `{file_name}`: {err}"
                ))));
            }
        }
    };
    staged.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!("failed to stage `{file_name}`: {err}"))
    })?;
    drop(staged);
    dir.rename(&tmp, dir, target).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!("failed to replace `{file_name}`: {err}"))
    })?;
    Ok(())
}
