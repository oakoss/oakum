//! The `.changeset/` files oakum owns: `_schema.json`, `README.md`, and
//! `_config.toml` (ADR-0023).
//!
//! Three commands write them: `init` writes all three, `migrate` writes them
//! and puts back a schema or README that went missing, and `upgrade`
//! regenerates `_schema.json` and rewrites `_config.toml`'s `tool-version`.
//! One definition of "owned" here, so the probe, the writes, and the uninstall
//! line cannot disagree about what oakum may touch.

use std::io;
use std::path::Path;

use cap_std::fs::Dir;
use oakum::config;
use oakum::plan::Versioning;
use semver::Version;

use super::fs::{write_file_exclusive, write_file_via_rename};
use super::CliError;

const CONFIG_REL: &str = ".changeset/_config.toml";
const SCHEMA_REL: &str = ".changeset/_schema.json";
pub(super) const README_REL: &str = ".changeset/README.md";
const README: &str = include_str!("changeset-readme.md");

/// What one `_schema.json` write did. `init`, `migrate`, and `upgrade`
/// share the currency test (byte equality with the bundled schema) and each
/// says the outcome in its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SchemaOutcome {
    Created,
    Replaced,
    Unchanged,
}

/// How `_schema.json` stands against the bundled schema, by bytes. A file
/// that cannot be read counts as stale: the rename replaces it without
/// reading, and reports if it cannot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SchemaState {
    Absent,
    Current,
    Stale,
}

pub(super) fn schema_state(dir: &Dir, path: &Path) -> SchemaState {
    match dir.read(path) {
        Ok(existing) if existing == config::schema_json().as_bytes() => SchemaState::Current,
        Err(err) if err.kind() == io::ErrorKind::NotFound => SchemaState::Absent,
        Ok(_) | Err(_) => SchemaState::Stale,
    }
}

/// Write the bundled schema at `path` unless `state` says it already holds
/// it. Both are the caller's (`upgrade` resolves `path` through symlinks),
/// so the pending line, the write, and the report all rest on one look.
///
/// # Errors
///
/// A failed write.
pub(super) fn write_schema(
    dir: &Dir,
    path: &Path,
    state: SchemaState,
) -> Result<SchemaOutcome, Box<dyn std::error::Error>> {
    let outcome = match state {
        SchemaState::Current => return Ok(SchemaOutcome::Unchanged),
        SchemaState::Stale => SchemaOutcome::Replaced,
        SchemaState::Absent => SchemaOutcome::Created,
    };
    write_file_via_rename(dir, path, &config::schema_json())?;
    Ok(outcome)
}

/// Whether the README present in `.changeset/` is oakum's. A copy
/// byte-identical to the bundled one is (a run that stopped before
/// `_config.toml` leaves exactly that), so the uninstall line names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadmeState {
    Absent,
    Ours,
    Theirs,
}

/// # Errors
///
/// A README that is a directory or symlink, or one that cannot be read.
fn readme_state(dir: &Dir) -> Result<ReadmeState, Box<dyn std::error::Error>> {
    if !regular_file_exists(dir, README_REL)? {
        return Ok(ReadmeState::Absent);
    }
    Ok(if dir.read_to_string(README_REL)? == README {
        ReadmeState::Ours
    } else {
        ReadmeState::Theirs
    })
}

/// The owned files as they stand before a write, probed once so the
/// pending line, the writes, and the uninstall line rest on one look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OwnedPlan {
    pub(super) schema: SchemaState,
    pub(super) readme: ReadmeState,
}

impl OwnedPlan {
    /// Refuses a README or schema that is a directory or symlink, so that
    /// surfaces before anything is printed or written.
    ///
    /// # Errors
    ///
    /// A path that exists but is not a regular file, or cannot be read.
    pub(super) fn probe(dir: &Dir) -> Result<Self, Box<dyn std::error::Error>> {
        let schema = if regular_file_exists(dir, SCHEMA_REL)? {
            schema_state(dir, Path::new(SCHEMA_REL))
        } else {
            SchemaState::Absent
        };
        Ok(Self {
            schema,
            readme: readme_state(dir)?,
        })
    }

    /// Every file oakum owns once the write lands: what the uninstall line
    /// names. A README that was already oakum's counts; a user's does not.
    pub(super) fn owned_after_write(self) -> Vec<&'static str> {
        let mut written = vec![SCHEMA_REL];
        if self.readme != ReadmeState::Theirs {
            written.push(README_REL);
        }
        written.push(CONFIG_REL);
        written
    }
}

pub(super) fn write_owned_files(
    dir: &Dir,
    plan: OwnedPlan,
    binary: &Version,
    change_files: bool,
    conventional_commits: bool,
    versioning: Versioning,
) -> Result<OwnedWrites, Box<dyn std::error::Error>> {
    // Each line prints as its write lands, so a failure part-way through
    // leaves an accurate record of what changed.
    let schema = write_schema(dir, Path::new(SCHEMA_REL), plan.schema)?;
    println!(
        "{} {SCHEMA_REL}",
        match schema {
            SchemaOutcome::Created => "created",
            SchemaOutcome::Replaced => "replaced",
            SchemaOutcome::Unchanged => "unchanged",
        }
    );
    if plan.readme == ReadmeState::Absent {
        write_file_exclusive(dir, Path::new(README_REL), README)?;
        println!("created {README_REL}");
    }
    write_file_exclusive(
        dir,
        Path::new(CONFIG_REL),
        &config_body(binary, change_files, conventional_commits, versioning),
    )?;
    println!("created {CONFIG_REL}");
    Ok(OwnedWrites {
        written: plan.owned_after_write(),
    })
}

/// Every file oakum now owns after [`write_owned_files`], including a
/// `_schema.json` it replaced or found current: what the uninstall line
/// must name.
pub(super) struct OwnedWrites {
    pub(super) written: Vec<&'static str>,
}

/// An owned file `migrate` can put back after it went missing. `_config.toml`
/// carries user settings and is never replaced, so it is not one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RestorableFile {
    Schema,
    Readme,
}

impl RestorableFile {
    const ALL: [Self; 2] = [Self::Schema, Self::Readme];

    pub(super) const fn rel(self) -> &'static str {
        match self {
            Self::Schema => SCHEMA_REL,
            Self::Readme => README_REL,
        }
    }

    fn body(self) -> String {
        match self {
            Self::Schema => config::schema_json(),
            Self::Readme => String::from(README),
        }
    }
}

/// The owned files a migrated repository no longer has.
pub(super) fn missing_owned_files(
    dir: &Dir,
) -> Result<Vec<RestorableFile>, Box<dyn std::error::Error>> {
    let mut missing = Vec::new();
    for file in RestorableFile::ALL {
        if !regular_file_exists(dir, file.rel())? {
            missing.push(file);
        }
    }
    Ok(missing)
}

/// Exclusive, so a file that appeared since the look is refused, not replaced.
pub(super) fn restore_owned_file(dir: &Dir, file: RestorableFile) -> io::Result<()> {
    write_file_exclusive(dir, Path::new(file.rel()), &file.body())
}

fn config_body(
    binary: &Version,
    change_files: bool,
    conventional_commits: bool,
    versioning: Versioning,
) -> String {
    format!(
        "#:schema ./_schema.json\n\
tool-version = \"{binary}\"\n\
change-files = {change_files}\n\
conventional-commits = {conventional_commits}\n\
versioning = \"{versioning}\"\n"
    )
}

fn regular_file_exists(dir: &Dir, path: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match dir.symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(meta) if meta.file_type().is_symlink() => Err(Box::new(CliError::new(format!(
            "`{path}` is a symlink; replace it with a regular file or remove it so oakum can write its own"
        )))),
        Ok(_) => Err(Box::new(CliError::new(format!(
            "`{path}` exists and is not a regular file"
        )))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(Box::new(CliError::new(format!(
            "failed to inspect `{path}`: {err}"
        )))),
    }
}

#[cfg(test)]
mod schema_seam {
    use super::{schema_state, write_schema, SchemaOutcome, SCHEMA_REL};
    use crate::test_fixture::Fixture;
    use cap_std::fs::Dir;
    use std::path::Path;

    fn scratch(label: &str) -> Fixture {
        let root = Fixture::new("schema-seam", label);
        std::fs::create_dir_all(root.join(".changeset")).expect("scratch");
        root
    }

    #[test]
    fn one_write_says_created_replaced_or_unchanged() {
        let root = scratch("outcomes");
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).expect("dir");
        let path = Path::new(SCHEMA_REL);
        assert_eq!(
            write_schema(&dir, path, schema_state(&dir, path)).expect("first"),
            SchemaOutcome::Created
        );
        assert_eq!(
            write_schema(&dir, path, schema_state(&dir, path)).expect("again"),
            SchemaOutcome::Unchanged
        );
        std::fs::write(root.join(SCHEMA_REL), "{\"stale\": true}\n").expect("stale");
        assert_eq!(
            write_schema(&dir, path, schema_state(&dir, path)).expect("stale"),
            SchemaOutcome::Replaced
        );
        assert_eq!(
            std::fs::read_to_string(root.join(SCHEMA_REL)).expect("read"),
            oakum::config::schema_json()
        );
    }
}
