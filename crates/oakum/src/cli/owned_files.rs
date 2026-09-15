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

use super::deliver_out;
use super::fs::{write_file_exclusive, write_file_via_rename};
use super::tag_shape::ReadableTemplate;
use super::CliError;

const CONFIG_REL: &str = ".changeset/_config.toml";
const SCHEMA_REL: &str = ".changeset/_schema.json";
pub(super) const README_REL: &str = ".changeset/README.md";
const README: &str = include_str!("changeset-readme.md");

/// The preferences `_config.toml` records. `init` resolves them from its
/// flags; `migrate` carries what the source tool set.
///
/// Both intent mechanisms false is a config the loader rejects; `init`'s
/// `refuse_both_intent_disabled` refuses it at the flag boundary, so the pair
/// arrives here pre-validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConfigSettings {
    pub(super) change_files: bool,
    pub(super) conventional_commits: bool,
    pub(super) versioning: Versioning,
    pub(super) private_packages: PrivatePackages,
    /// Set only when the repository's existing tags derive a shape that
    /// differs from the default `release` would otherwise apply.
    pub(super) tag_format: Option<ReadableTemplate>,
    /// Carried from a source tool's own setting when it differs from what
    /// `ci version-pr` writes anyway.
    pub(super) commit_message: Option<String>,
}

/// oakum's `private-packages` opt-in (ADR-0027) as the writer needs it:
/// [`oakum::config::PrivatePackages`] is what the parser produces and cannot
/// be built here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PrivatePackages {
    pub(super) version: bool,
    pub(super) tag: bool,
}

impl PrivatePackages {
    /// The axes this value opts in, so a report can name what one source file
    /// contributed rather than what the whole migration writes.
    pub(super) fn axis_names(self) -> Vec<&'static str> {
        [("version", self.version), ("tag", self.tag)]
            .into_iter()
            .filter_map(|(name, on)| on.then_some(name))
            .collect()
    }

    /// Whether the setting says anything an absent table does not.
    pub(super) const fn any(self) -> bool {
        self.version || self.tag
    }

    pub(super) const fn union(self, other: Self) -> Self {
        Self {
            version: self.version || other.version,
            tag: self.tag || other.tag,
        }
    }

    /// The line `_config.toml` carries. One value produces it, so the report
    /// and the file cannot describe the opt-in differently.
    pub(super) fn toml_line(self) -> String {
        format!(
            "private-packages = {{ version = {}, tag = {} }}",
            self.version, self.tag
        )
    }
}

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
    settings: ConfigSettings,
    records: &mut Records,
) -> Result<OwnedWrites, Box<dyn std::error::Error>> {
    let schema = write_schema(dir, Path::new(SCHEMA_REL), plan.schema)?;
    records.record(&format!(
        "{} {SCHEMA_REL}",
        match schema {
            SchemaOutcome::Created => "created",
            SchemaOutcome::Replaced => "replaced",
            SchemaOutcome::Unchanged => "unchanged",
        }
    ));
    if plan.readme == ReadmeState::Absent {
        write_file_exclusive(dir, Path::new(README_REL), README)?;
        records.record(&format!("created {README_REL}"));
    }
    write_file_exclusive(dir, Path::new(CONFIG_REL), &config_body(binary, settings))?;
    records.record(&format!("created {CONFIG_REL}"));
    Ok(OwnedWrites {
        written: plan.owned_after_write(),
    })
}

/// The lines a write command hands over, delivered as each one is ready so a
/// failure part-way through leaves an accurate record. A refused delivery is
/// held rather than returned: a reader who went away is no reason to leave
/// the directory half-written, so the remaining writes still happen and the
/// refusal becomes the verdict at [`Records::finish`], naming every line that
/// never arrived. A write that fails after a refusal is the verdict, and
/// [`Records::abandon`] carries the refusal into it.
#[derive(Default)]
pub(super) struct Records {
    refused: Option<(String, std::io::Error)>,
    undelivered: Vec<String>,
}

impl Records {
    /// A `created`/`wrote` line, or a summary line: the record of a write
    /// that already landed.
    pub(super) fn record(&mut self, line: &str) {
        self.deliver(&format!("the record `{line}`"), line);
    }

    /// `what` names the text in the verdict; `text` is what stdout receives.
    pub(super) fn deliver(&mut self, what: &str, text: &str) {
        if self.refused.is_some() {
            self.undelivered.push(what.to_owned());
            return;
        }
        if let Err(err) = deliver_out(text) {
            self.refused = Some((what.to_owned(), err));
        }
    }

    /// A write failed after these records: its error is the verdict, and the
    /// refusal is named inside it so the lines that landed are not lost.
    pub(super) fn abandon(
        &mut self,
        err: impl Into<Box<dyn std::error::Error>>,
    ) -> Box<dyn std::error::Error> {
        let err = err.into();
        let Err(refusal) = std::mem::take(self).finish() else {
            return err;
        };
        Box::new(CliError::new(format!(
            "{}; before that, {}",
            CliError::from_boxed(err).detail(),
            refusal.detail()
        )))
    }

    pub(super) fn finish(self) -> Result<(), CliError> {
        let Some((mut what, err)) = self.refused else {
            return Ok(());
        };
        if !self.undelivered.is_empty() {
            what.push_str(" and, after it, ");
            what.push_str(&self.undelivered.join(", "));
        }
        Err(CliError::undelivered(what, &err))
    }
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

/// The `commit-message` line as the file receives it: a TOML basic string with
/// the two characters TOML gives meaning escaped. Shared with the report, so the
/// line `migrate` announces and the line it writes cannot differ — announcing
/// the decoded value printed `commit-message = "cut "the" packages"`, which was
/// neither.
///
/// A message reaching here has been refused for control characters upstream, so
/// no other escape applies.
pub(super) fn commit_message_line(message: &str) -> String {
    format!(
        "commit-message = \"{}\"",
        message.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// `private-packages` is written as an inline table, the shape ADR-0027
/// documents. A `[private-packages]` header would have to follow every scalar
/// or swallow the ones after it (`okm-404.12`); an inline table has no such
/// ordering to get wrong.
fn config_body(binary: &Version, settings: ConfigSettings) -> String {
    let ConfigSettings {
        change_files,
        conventional_commits,
        versioning,
        private_packages,
        tag_format,
        commit_message,
    } = settings;
    // `ReadableTemplate` can only hold one of the four shapes `tag_shape`
    // recognizes, none of which carries anything TOML would have to escape.
    let tag_format = tag_format.map_or_else(String::new, |template| {
        format!("tag-format = \"{}\"\n", template.as_str())
    });
    let private = if private_packages.any() {
        format!("{}\n", private_packages.toml_line())
    } else {
        String::new()
    };
    let commit_message = commit_message
        .as_deref()
        .map_or_else(String::new, |message| {
            format!("{}\n", commit_message_line(message))
        });
    format!(
        "#:schema ./_schema.json\n\
tool-version = \"{binary}\"\n\
change-files = {change_files}\n\
conventional-commits = {conventional_commits}\n\
versioning = \"{versioning}\"\n\
{tag_format}{private}{commit_message}"
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
mod records {
    use super::Records;

    fn refused(what: &str) -> Records {
        Records {
            refused: Some((
                what.to_owned(),
                std::io::Error::from(std::io::ErrorKind::BrokenPipe),
            )),
            undelivered: Vec::new(),
        }
    }

    #[test]
    fn a_lone_refusal_names_only_itself() {
        let verdict = refused("the record `created x`").finish().unwrap_err();
        assert_eq!(
            verdict.detail(),
            "the record `created x` could not be delivered to stdout: broken pipe"
        );
    }

    #[test]
    fn lines_after_a_refusal_are_named_in_order() {
        let mut records = refused("the record `created x`");
        records.deliver("the workflow", "workflow text");
        records.record("created y");
        let verdict = records.finish().unwrap_err();
        assert_eq!(
            verdict.detail(),
            "the record `created x` and, after it, the workflow, the record `created y` could not be delivered to stdout: broken pipe"
        );
    }

    #[test]
    fn a_write_error_after_a_refusal_names_both() {
        let mut records = refused("the record `wrote a`");
        records.record("created b");
        let err = records.abandon(std::io::Error::other("failed to create c"));
        assert_eq!(
            err.to_string(),
            "failed to create c; before that, the record `wrote a` and, after it, the record `created b` could not be delivered to stdout: broken pipe"
        );
    }

    #[test]
    fn a_write_error_with_nothing_refused_is_itself() {
        let err = Records::default().abandon(std::io::Error::other("failed to create c"));
        assert_eq!(err.to_string(), "failed to create c");
    }

    #[test]
    fn nothing_refused_is_ok() {
        let mut records = Records::default();
        records.record("created x");
        assert!(records.finish().is_ok());
    }
}

#[cfg(test)]
mod written_config {
    use super::{config_body, ConfigSettings, PrivatePackages};
    use oakum::plan::Versioning;
    use semver::Version;

    fn body(private_packages: PrivatePackages) -> String {
        config_body(
            &Version::new(0, 2, 0),
            ConfigSettings {
                change_files: true,
                conventional_commits: false,
                versioning: Versioning::Semver,
                private_packages,
                tag_format: None,
                commit_message: None,
            },
        )
    }

    #[test]
    fn an_opted_in_axis_round_trips_through_the_config_loader() {
        for private_packages in [
            PrivatePackages {
                version: true,
                tag: true,
            },
            PrivatePackages {
                version: true,
                tag: false,
            },
            PrivatePackages {
                version: false,
                tag: true,
            },
        ] {
            let written = body(private_packages);
            let parsed = oakum::config::parse(&written).expect("written config parses");
            assert_eq!(
                parsed.private_packages().version(),
                private_packages.version,
                "{written}"
            );
            assert_eq!(
                parsed.private_packages().tag(),
                private_packages.tag,
                "{written}"
            );
            assert_eq!(parsed.versioning(), Versioning::Semver, "{written}");
            assert!(parsed.change_files(), "{written}");
            assert!(!parsed.conventional_commits(), "{written}");
        }
    }

    #[test]
    fn the_default_writes_no_private_packages_key() {
        let written = body(PrivatePackages::default());
        assert!(!written.contains("private-packages"), "{written}");
        let parsed = oakum::config::parse(&written).expect("written config parses");
        assert!(!parsed.private_packages().version());
        assert!(!parsed.private_packages().tag());
    }

    /// An inline table cannot swallow the scalars around it; a
    /// `[private-packages]` header placed before them would (`okm-404.12`).
    #[test]
    fn the_carried_setting_is_an_inline_table_on_one_line() {
        let written = body(PrivatePackages {
            version: true,
            tag: true,
        });
        assert!(
            written.contains("\nprivate-packages = { version = true, tag = true }\n"),
            "{written}"
        );
        assert!(!written.contains("[private-packages]"), "{written}");
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
