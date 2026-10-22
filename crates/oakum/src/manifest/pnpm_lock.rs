//! Rewrite the `pnpm-lock.yaml` entries a workspace range rewrite invalidates.
//!
//! pnpm records each importer's manifest specifier, and each catalog entry's
//! specifier, beside what it resolved to. A frozen install refuses a lockfile
//! whose specifier no longer matches the manifest (ADR-0023). A linked member
//! resolves to `link:<path>` in an importer and to its own specifier in a
//! catalog, so rewriting those fields is the whole fix and matches what
//! `pnpm install` writes. An entry resolved from the registry would need a
//! fresh resolution, so it is left for `pnpm install` and reported, as is an
//! entry that had already drifted from the manifest before this run, and any
//! entry this run invalidated but could not read.
//!
//! Decisions read the lockfile as data; the span walker only writes. A
//! lockfile in a shape nobody measured is left whole and named.
//!
//! Returns the new text; filesystem writes belong to `version`.

use std::collections::BTreeMap;

use saphyr_parser::{Event, Parser, Span as YamlSpan, SpannedEventReceiver};
use serde::de::IgnoredAny;
use serde::Deserialize;

use super::catalog::find_yaml_scalar;
use super::json::{json_string, JsonEditError};
use crate::discover::CatalogFile;

/// Lockfile formats whose importer and catalog shapes were measured: pnpm 9
/// through 12 write `9.0`.
const MEASURED_FORMATS: [&str; 1] = ["9.0"];

/// The sections pnpm records an importer's dependencies under.
const IMPORTER_SECTIONS: [&str; 3] = ["dependencies", "devDependencies", "optionalDependencies"];

const PEER_SECTION: &str = "peerDependencies";

/// One member manifest this run rewrote, keyed by its pnpm importer (`.` for
/// the lockfile's own directory).
#[derive(Clone, Copy, Debug)]
pub struct PnpmImporterChange<'a> {
    pub importer: &'a str,
    pub original: &'a str,
    pub next: &'a str,
    /// `(section, key)` for each edge on a bumped package.
    pub edges: &'a [(&'a str, &'a str)],
}

/// A `catalog:` reference to a bumped package, with the importer that makes it.
#[derive(Clone, Copy, Debug)]
pub struct PnpmCatalogRef<'a> {
    /// `None` is the default catalog.
    pub catalog: Option<&'a str>,
    pub key: &'a str,
    pub importer: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct PnpmCatalogChange<'a> {
    /// `pnpm-workspace.yaml` before and after this run; `None` when it could
    /// not be read.
    pub files: Option<(&'a str, &'a str)>,
    pub refs: &'a [PnpmCatalogRef<'a>],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PnpmLockRetarget {
    pub text: String,
    /// Entries the run invalidated but could not rewrite.
    pub left: Vec<PnpmLockLeft>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PnpmLockLeft {
    /// Slash-joined lockfile path to the entry, or `pnpm-lock.yaml` for the
    /// whole file.
    pub entry: String,
    pub reason: PnpmLockLeftReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PnpmLockLeftReason {
    /// The entry, or the manifest or catalog value it records, is missing or
    /// not a single-line string.
    Unreadable,
    Drifted {
        specifier: String,
        expected: String,
    },
    Resolved {
        version: String,
    },
    /// The lockfile has no entry where the manifest says pnpm records one.
    Missing,
    /// A catalog entry no importer records through the catalog, so nothing
    /// says whether it links the workspace member.
    Unconfirmed,
    NotRewritable,
    /// The whole file: a format or document layout nobody measured.
    UnknownFormat {
        detail: String,
    },
}

impl core::fmt::Display for PnpmLockLeft {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entry = &self.entry;
        match &self.reason {
            PnpmLockLeftReason::Drifted {
                specifier,
                expected,
            } => write!(
                f,
                "`{entry}` specifier is `{specifier}`, not the `{expected}` the manifest had"
            ),
            PnpmLockLeftReason::Resolved { version } => write!(
                f,
                "`{entry}` resolves to {version} from the registry, not to the workspace member"
            ),
            PnpmLockLeftReason::Unreadable => {
                write!(f, "`{entry}` could not be read as a single-line string")
            }
            PnpmLockLeftReason::Missing => {
                write!(f, "`{entry}` is missing, though the manifest declares it")
            }
            PnpmLockLeftReason::Unconfirmed => write!(
                f,
                "`{entry}` has no importer recording it through the catalog, so nothing shows it links the workspace member"
            ),
            PnpmLockLeftReason::NotRewritable => {
                write!(
                    f,
                    "`{entry}` cannot hold the new range in its current quoting"
                )
            }
            PnpmLockLeftReason::UnknownFormat { detail } => {
                write!(f, "the whole file: {detail}")
            }
        }
    }
}

/// The parts of the lockfile decisions read. A node in an unexpected shape
/// deserializes as `Other` rather than failing the whole file.
#[derive(Deserialize)]
struct Model {
    #[serde(rename = "lockfileVersion", default)]
    lockfile_version: Option<Text>,
    #[serde(default)]
    importers: Option<BTreeMap<String, Node<Importer>>>,
    #[serde(default)]
    catalogs: Option<BTreeMap<String, Node<Entries>>>,
}

#[derive(Deserialize)]
struct Importer {
    #[serde(default)]
    dependencies: Option<Node<Entries>>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: Option<Node<Entries>>,
    #[serde(rename = "optionalDependencies", default)]
    optional_dependencies: Option<Node<Entries>>,
}

type Entries = BTreeMap<String, Node<Entry>>;

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    specifier: Option<Text>,
    #[serde(default)]
    version: Option<Text>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Node<T> {
    Value(T),
    Other(IgnoredAny),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Text {
    Str(String),
    Other(IgnoredAny),
}

impl Text {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            Self::Other(_) => None,
        }
    }
}

/// A lookup that keeps "absent" apart from "present in a shape nobody reads",
/// which must be reported rather than taken for absence.
enum Found<'a, T> {
    Absent,
    Odd,
    Value(&'a T),
}

impl<'a, T> Found<'a, T> {
    fn of(node: Option<&'a Node<T>>) -> Self {
        match node {
            None => Self::Absent,
            Some(Node::Other(_)) => Self::Odd,
            Some(Node::Value(value)) => Self::Value(value),
        }
    }
}

impl Model {
    fn importer(&self, importer: &str) -> Found<'_, Importer> {
        Found::of(self.importers.as_ref().and_then(|all| all.get(importer)))
    }

    fn catalog(&self, table: &str, key: &str) -> Found<'_, Entry> {
        match Found::of(self.catalogs.as_ref().and_then(|all| all.get(table))) {
            Found::Absent => Found::Absent,
            Found::Odd => Found::Odd,
            Found::Value(entries) => Found::of(entries.get(key)),
        }
    }
}

impl Importer {
    fn entry(&self, section: &str, key: &str) -> Found<'_, Entry> {
        let entries = match section {
            "dependencies" => self.dependencies.as_ref(),
            "devDependencies" => self.dev_dependencies.as_ref(),
            _ => self.optional_dependencies.as_ref(),
        };
        match Found::of(entries) {
            Found::Absent => Found::Absent,
            Found::Odd => Found::Odd,
            Found::Value(entries) => Found::of(entries.get(key)),
        }
    }
}

/// An entry's `(specifier, version)`, or `None` when either is not a string.
fn fields(entry: &Entry) -> Option<(&str, &str)> {
    Some((
        entry.specifier.as_ref()?.as_str()?,
        entry.version.as_ref()?.as_str()?,
    ))
}

/// Whether the document uses YAML anchors or aliases, or `None` when it does
/// not parse. pnpm never writes them, and a write through an alias would land
/// on whatever entry owns the anchor.
fn uses_anchors(text: &str) -> Option<bool> {
    struct Scan(bool);
    impl<'input> SpannedEventReceiver<'input> for Scan {
        fn on_event(&mut self, event: Event<'input>, _span: YamlSpan) {
            match event {
                Event::Alias(_) => self.0 = true,
                Event::Scalar(_, _, anchor, _)
                | Event::MappingStart(anchor, _)
                | Event::SequenceStart(anchor, _)
                    if anchor > 0 =>
                {
                    self.0 = true;
                }
                _ => {}
            }
        }
    }
    let mut scan = Scan(false);
    Parser::new_from_str(text).load(&mut scan, false).ok()?;
    Some(scan.0)
}

/// pnpm's environment document: one `.` importer holding only its own
/// `configDependencies` and `packageManagerDependencies`.
fn is_environment(text: &str) -> bool {
    #[derive(Deserialize)]
    struct Environment {
        #[expect(
            clippy::zero_sized_map_values,
            reason = "only the field names are read; the values are skipped"
        )]
        importers: BTreeMap<String, BTreeMap<String, IgnoredAny>>,
    }
    let Ok(environment) = serde_saphyr::from_str::<Environment>(text) else {
        return false;
    };
    let mut importers = environment.importers.iter();
    match (importers.next(), importers.next()) {
        (Some((key, fields)), None) => {
            key == "."
                && !fields.is_empty()
                && fields.keys().all(|field| {
                    field == "configDependencies" || field == "packageManagerDependencies"
                })
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum Resolution {
    /// An importer: the member is linked as `link:<path>`.
    Link,
    /// A catalog entry every consumer links; its version repeats its specifier.
    Catalog,
}

#[must_use]
pub fn retarget_pnpm_lock(
    text: &str,
    importers: &[PnpmImporterChange<'_>],
    catalog: Option<PnpmCatalogChange<'_>>,
) -> PnpmLockRetarget {
    let unchanged = |left| PnpmLockRetarget {
        text: text.to_owned(),
        left,
    };
    if !anything_moved(importers, catalog) {
        return unchanged(Vec::new());
    }
    let whole = |detail: String| {
        vec![PnpmLockLeft {
            entry: String::from("pnpm-lock.yaml"),
            reason: PnpmLockLeftReason::UnknownFormat { detail },
        }]
    };
    let (prefix, body) = match main_document(text) {
        Ok(split) => split,
        Err(detail) => return unchanged(whole(detail)),
    };
    match uses_anchors(body) {
        Some(false) => {}
        Some(true) => {
            return unchanged(whole(String::from(
                "the lockfile uses YAML anchors or aliases, which pnpm does not write",
            )))
        }
        None => return unchanged(whole(String::from("the lockfile could not be parsed"))),
    }
    let Ok(model) = serde_saphyr::from_str::<Model>(body) else {
        return unchanged(whole(String::from("the lockfile could not be parsed")));
    };
    match &model.lockfile_version {
        Some(Text::Str(format)) if MEASURED_FORMATS.contains(&format.as_str()) => {}
        Some(Text::Str(format)) => {
            return unchanged(whole(format!(
                "lockfileVersion {format} is not a format oakum has measured"
            )))
        }
        Some(Text::Other(_)) => {
            return unchanged(whole(String::from(
                "lockfileVersion is not a quoted string, as pnpm 9 and later write it",
            )))
        }
        None => return unchanged(whole(String::from("the lockfile has no lockfileVersion"))),
    }
    let mut out = body.to_owned();
    let mut left = Vec::new();
    for change in importers {
        retarget_importer(&mut out, &model, change, &mut left);
    }
    if let Some(catalog) = catalog {
        retarget_catalogs(&mut out, &model, catalog, &mut left);
    }
    PnpmLockRetarget {
        text: format!("{prefix}{out}"),
        left,
    }
}

/// Whether this run moved any manifest or catalog value, before the lockfile
/// is read at all: an unmeasured format with nothing to rewrite is not news.
fn anything_moved(
    importers: &[PnpmImporterChange<'_>],
    catalog: Option<PnpmCatalogChange<'_>>,
) -> bool {
    let edge_moved = importers.iter().any(|change| {
        change.edges.iter().any(|(section, key)| {
            json_string(change.original, &[section, key]).ok()
                != json_string(change.next, &[section, key]).ok()
        })
    });
    let catalog_moved = catalog.is_some_and(|catalog| {
        !catalog.refs.is_empty()
            && catalog
                .files
                .is_none_or(|(original, next)| original != next)
    });
    edge_moved || catalog_moved
}

/// Splits off the document pnpm keeps its lockfile in. With `packageManager`
/// or `configDependencies` set, pnpm writes an environment document first and
/// the lockfile second, each opened by a bare `---` line.
fn main_document(text: &str) -> Result<(&str, &str), String> {
    let mut markers = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare == "---" || bare.starts_with("--- ") {
            markers.push((offset, offset + line.len()));
        }
        offset += line.len();
    }
    let Some(&(_, body_start)) = markers.last() else {
        return Ok(("", text));
    };
    let leading = text[..markers[0].0].trim();
    let documents = markers.len() + usize::from(!leading.is_empty());
    let environment = &text[..markers.last().map_or(0, |marker| marker.0)];
    if documents > 2 || (documents == 2 && !is_environment(environment)) {
        return Err(format!(
            "the lockfile holds {documents} YAML documents in a layout oakum has not measured"
        ));
    }
    Ok((&text[..body_start], &text[body_start..]))
}

/// The manifest's `(from, to)` for `key` in `section`: `Ok(None)` when neither
/// side declares it, `Err` when one side cannot be read.
fn declared(
    change: &PnpmImporterChange<'_>,
    section: &str,
    key: &str,
) -> Result<Option<(String, String)>, ()> {
    let read = |text| match json_string(text, &[section, key]) {
        Ok(value) => Ok(Some(value)),
        Err(JsonEditError::Missing { .. }) => Ok(None),
        Err(_) => Err(()),
    };
    match (read(change.original)?, read(change.next)?) {
        (Some(from), Some(to)) => Ok(Some((from, to))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn retarget_importer(
    out: &mut String,
    model: &Model,
    change: &PnpmImporterChange<'_>,
    left: &mut Vec<PnpmLockLeft>,
) {
    let mut keys: Vec<&str> = Vec::new();
    for &(_, key) in change.edges {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let importer = model.importer(change.importer);
    for key in keys {
        let leave = |section: &str, reason| PnpmLockLeft {
            entry: ["importers", change.importer, section, key].join("/"),
            reason,
        };
        // Every section the manifest declares the key in, not only the edges
        // this run rewrote: pnpm records whichever declaration wins.
        let mut declarations = Vec::new();
        let mut unread = None;
        for section in IMPORTER_SECTIONS.into_iter().chain([PEER_SECTION]) {
            match declared(change, section, key) {
                Ok(Some((from, to))) => declarations.push((section, from, to)),
                Ok(None) => {}
                Err(()) => unread = unread.or(Some(section)),
            }
        }
        if let Some(section) = unread {
            left.push(leave(section, PnpmLockLeftReason::Unreadable));
            continue;
        }
        let Some((first_moved, _, _)) = declarations.iter().find(|(_, from, to)| from != to) else {
            continue;
        };
        let importer = match &importer {
            Found::Value(importer) => Some(*importer),
            Found::Absent => None,
            Found::Odd => {
                left.push(leave(first_moved, PnpmLockLeftReason::Unreadable));
                continue;
            }
        };
        let mut recorded = Vec::new();
        let mut odd = false;
        for section in IMPORTER_SECTIONS {
            match importer.map_or(Found::Absent, |importer| importer.entry(section, key)) {
                Found::Value(entry) => recorded.push((section, entry)),
                Found::Odd => {
                    odd = true;
                    left.push(leave(section, PnpmLockLeftReason::Unreadable));
                }
                Found::Absent => {}
            }
        }
        if recorded.is_empty() {
            // A peer pnpm did not auto-install has no importer entry to stale.
            let only_peers_moved = declarations
                .iter()
                .all(|(section, from, to)| *section == PEER_SECTION || from == to);
            if !odd && !only_peers_moved {
                left.push(leave(first_moved, PnpmLockLeftReason::Missing));
            }
            continue;
        }
        let ordinary = declarations
            .iter()
            .any(|(section, _, _)| IMPORTER_SECTIONS.contains(section));
        for (section, entry) in recorded {
            // A peer is auto-installed under `dependencies` only when no
            // ordinary section declares the key.
            let declaration = declarations
                .iter()
                .find(|(declared, _, _)| *declared == section)
                .or_else(|| {
                    (section == "dependencies" && !ordinary)
                        .then(|| {
                            declarations
                                .iter()
                                .find(|(declared, _, _)| *declared == PEER_SECTION)
                        })
                        .flatten()
                });
            let Some((_, from, to)) = declaration else {
                left.push(leave(section, PnpmLockLeftReason::Unreadable));
                continue;
            };
            if from == to {
                continue;
            }
            let path = ["importers", change.importer, section, key];
            if let Err(reason) = retarget_entry(out, &path, entry, from, to, Resolution::Link) {
                left.push(leave(section, reason));
            }
        }
    }
}

/// What a consumer's importer entry says about a catalog entry.
enum Evidence {
    Linked,
    Resolved(String),
    Unreadable,
}

fn retarget_catalogs(
    out: &mut String,
    model: &Model,
    change: PnpmCatalogChange<'_>,
    left: &mut Vec<PnpmLockLeft>,
) {
    // pnpm reads `catalog:default` as the default catalog.
    let mut groups: Vec<(Option<&str>, &str, Vec<&str>)> = Vec::new();
    for reference in change.refs {
        let catalog = reference.catalog.filter(|name| *name != "default");
        match groups
            .iter_mut()
            .find(|(c, key, _)| *c == catalog && *key == reference.key)
        {
            Some((_, _, importers)) => importers.push(reference.importer),
            None => groups.push((catalog, reference.key, vec![reference.importer])),
        }
    }
    let files = change.files.and_then(|(original, next)| {
        Some((
            CatalogFile::parse(original).ok()?,
            CatalogFile::parse(next).ok()?,
        ))
    });
    for (catalog, key, importers) in groups {
        let table = catalog.unwrap_or("default");
        let path = ["catalogs", table, key];
        let leave = |reason| PnpmLockLeft {
            entry: path.join("/"),
            reason,
        };
        let Some((original, next)) = &files else {
            left.push(leave(PnpmLockLeftReason::Unreadable));
            continue;
        };
        let pin = |file: &CatalogFile| -> Option<String> {
            match catalog {
                Some(name) => file.string_at(&["catalogs", name, key]),
                None => file
                    .string_at(&["catalog", key])
                    .or_else(|| file.string_at(&["catalogs", "default", key])),
            }
            .map(str::to_owned)
        };
        let (Some(from), Some(to)) = (pin(original), pin(next)) else {
            left.push(leave(PnpmLockLeftReason::Unreadable));
            continue;
        };
        if from == to {
            continue;
        }
        let evidence = catalog_evidence(model, catalog, key, &importers);
        let entry = match model.catalog(table, key) {
            Found::Value(entry) => entry,
            Found::Odd => {
                left.push(leave(PnpmLockLeftReason::Unreadable));
                continue;
            }
            // pnpm writes no catalog entry when no importer records the key
            // through the catalog; one that does means the entry went missing.
            Found::Absent => {
                if !evidence.is_empty() {
                    left.push(leave(PnpmLockLeftReason::Missing));
                }
                continue;
            }
        };
        if evidence
            .iter()
            .any(|seen| matches!(seen, Evidence::Unreadable))
        {
            left.push(leave(PnpmLockLeftReason::Unreadable));
            continue;
        }
        if evidence.is_empty() {
            left.push(leave(PnpmLockLeftReason::Unconfirmed));
            continue;
        }
        if let Some(version) = evidence.iter().find_map(|seen| match seen {
            Evidence::Resolved(version) => Some(version),
            _ => None,
        }) {
            left.push(leave(PnpmLockLeftReason::Resolved {
                version: version.clone(),
            }));
            continue;
        }
        if let Err(reason) = retarget_entry(out, &path, entry, &from, &to, Resolution::Catalog) {
            left.push(leave(reason));
        }
    }
}

/// One verdict per importer entry that takes `key` through this catalog. A
/// referenced importer that is absent or unreadable, or an entry whose
/// specifier cannot be read, might be one, so it counts as unreadable
/// evidence rather than none.
fn catalog_evidence(
    model: &Model,
    catalog: Option<&str>,
    key: &str,
    importers: &[&str],
) -> Vec<Evidence> {
    let through_catalog = |specifier: &str| match catalog {
        Some(name) => specifier.strip_prefix("catalog:") == Some(name),
        None => specifier == "catalog:" || specifier == "catalog:default",
    };
    let mut evidence = Vec::new();
    for importer in importers {
        let Found::Value(importer) = model.importer(importer) else {
            evidence.push(Evidence::Unreadable);
            continue;
        };
        for section in IMPORTER_SECTIONS {
            let entry = match importer.entry(section, key) {
                Found::Value(entry) => entry,
                Found::Odd => {
                    evidence.push(Evidence::Unreadable);
                    continue;
                }
                Found::Absent => continue,
            };
            match entry.specifier.as_ref().and_then(Text::as_str) {
                None => evidence.push(Evidence::Unreadable),
                Some(specifier) if through_catalog(specifier) => {
                    match entry.version.as_ref().and_then(Text::as_str) {
                        Some(version) if version.starts_with("link:") => {
                            evidence.push(Evidence::Linked);
                        }
                        Some(version) => evidence.push(Evidence::Resolved(version.to_owned())),
                        None => evidence.push(Evidence::Unreadable),
                    }
                }
                Some(_) => {}
            }
        }
    }
    evidence
}

/// Leaves `text` untouched on `Err`.
fn retarget_entry(
    text: &mut String,
    path: &[&str],
    entry: &Entry,
    from: &str,
    to: &str,
    resolution: Resolution,
) -> Result<(), PnpmLockLeftReason> {
    let (specifier, version) = fields(entry).ok_or(PnpmLockLeftReason::Unreadable)?;
    if specifier == to {
        return Ok(());
    }
    if specifier != from {
        return Err(PnpmLockLeftReason::Drifted {
            specifier: specifier.to_owned(),
            expected: from.to_owned(),
        });
    }
    let linked = match resolution {
        Resolution::Link => version.starts_with("link:"),
        Resolution::Catalog => version == from,
    };
    if !linked {
        return Err(PnpmLockLeftReason::Resolved {
            version: version.to_owned(),
        });
    }
    let mut next = replace(text, &field_path(path, "specifier"), to)?;
    if matches!(resolution, Resolution::Catalog) {
        next = replace(&next, &field_path(path, "version"), to)?;
    }
    *text = next;
    Ok(())
}

fn field_path<'a>(entry: &[&'a str], field: &'a str) -> Vec<&'a str> {
    let mut path = entry.to_vec();
    path.push(field);
    path
}

/// Writes inside the existing quotes, then reads the value back: a value the
/// old style cannot carry is refused rather than written as something else.
fn replace(text: &str, path: &[&str], value: &str) -> Result<String, PnpmLockLeftReason> {
    let (span, _) =
        find_yaml_scalar(text, path, true).map_err(|_| PnpmLockLeftReason::NotRewritable)?;
    let mut out = String::with_capacity(text.len() + value.len());
    out.push_str(&text[..span.start]);
    out.push_str(value);
    out.push_str(&text[span.end..]);
    match find_yaml_scalar(&out, path, true) {
        Ok((_, decoded)) if decoded == value => Ok(out),
        _ => Err(PnpmLockLeftReason::NotRewritable),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        retarget_pnpm_lock, PnpmCatalogChange, PnpmCatalogRef, PnpmImporterChange, PnpmLockLeft,
        PnpmLockLeftReason,
    };

    /// `pnpm install` output under pnpm 12.11.2 for a workspace where `b`
    /// depends on `a` as `workspace:^1.0.0` and `c` takes it from the catalog.
    const LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

catalogs:
  default:
    a:
      specifier: workspace:^1.0.0
      version: workspace:^1.0.0

importers:

  .: {}

  packages/a: {}

  packages/b:
    dependencies:
      a:
        specifier: workspace:^1.0.0
        version: link:../a

  packages/c:
    dependencies:
      a:
        specifier: 'catalog:'
        version: link:../a
";

    /// The shape of the environment document pnpm 12.11.2 writes first when
    /// the root manifest declares `packageManager`, trimmed to one package and
    /// with its pnpm pin swapped for `a`, so a rewrite that reached it shows.
    const ENVIRONMENT: &str = "---
lockfileVersion: '9.0'

importers:

  .:
    configDependencies: {}
    packageManagerDependencies:
      a:
        specifier: workspace:^1.0.0
        version: workspace:^1.0.0

packages:

  '@pnpm/exe.darwin-arm64@12.11.2':
    resolution: {integrity: sha512-n9M4On66y8FDxUAKgMO1FXt0qihyG4e+56p3/F0uo8Bx9KplW8BwKVnXigzCJ8aNM6sVVPkOE6M9XXbeVIxnZA==}
    cpu: [arm64]
    os: [darwin]

snapshots:

  '@pnpm/exe.darwin-arm64@12.11.2':
    optional: true

---
";

    const B_BEFORE: &str = "{\"name\":\"b\",\"dependencies\":{\"a\":\"workspace:^1.0.0\"}}";
    const B_AFTER: &str = "{\"name\":\"b\",\"dependencies\":{\"a\":\"workspace:^2.0.0\"}}";
    const YAML_BEFORE: &str = "packages:\n  - packages/*\ncatalog:\n  a: workspace:^1.0.0\n";
    const YAML_AFTER: &str = "packages:\n  - packages/*\ncatalog:\n  a: workspace:^2.0.0\n";
    const C_REF: [PnpmCatalogRef<'static>; 1] = [PnpmCatalogRef {
        catalog: None,
        key: "a",
        importer: "packages/c",
    }];

    fn b_change<'a>(original: &'a str, next: &'a str) -> PnpmImporterChange<'a> {
        PnpmImporterChange {
            importer: "packages/b",
            original,
            next,
            edges: &[("dependencies", "a")],
        }
    }

    fn change<'a>(
        importer: &'a str,
        original: &'a str,
        next: &'a str,
        edges: &'a [(&'a str, &'a str)],
    ) -> PnpmImporterChange<'a> {
        PnpmImporterChange {
            importer,
            original,
            next,
            edges,
        }
    }

    fn catalog_change() -> PnpmCatalogChange<'static> {
        PnpmCatalogChange {
            files: Some((YAML_BEFORE, YAML_AFTER)),
            refs: &C_REF,
        }
    }

    fn exact_pin(refs: &[PnpmCatalogRef<'_>], lock: &str) -> super::PnpmLockRetarget {
        retarget_pnpm_lock(
            lock,
            &[],
            Some(PnpmCatalogChange {
                files: Some(("catalog:\n  a: 1.0.0\n", "catalog:\n  a: 2.0.0\n")),
                refs,
            }),
        )
    }

    fn left(entry: &str, reason: PnpmLockLeftReason) -> PnpmLockLeft {
        PnpmLockLeft {
            entry: entry.into(),
            reason,
        }
    }

    /// Byte-identical to what pnpm 12.11.2 wrote after the same bump.
    #[test]
    fn rewrites_importer_and_catalog_like_pnpm_does() {
        let out = retarget_pnpm_lock(LOCK, &[b_change(B_BEFORE, B_AFTER)], Some(catalog_change()));
        assert_eq!(out.left, []);
        assert_eq!(out.text, LOCK.replace("^1.0.0", "^2.0.0"));
    }

    #[test]
    fn an_unchanged_manifest_leaves_the_lock_alone() {
        let out = retarget_pnpm_lock(LOCK, &[b_change(B_BEFORE, B_BEFORE)], None);
        assert_eq!(out.text, LOCK);
        assert_eq!(out.left, []);
    }

    #[test]
    fn a_second_run_is_a_no_op() {
        let once = retarget_pnpm_lock(LOCK, &[b_change(B_BEFORE, B_AFTER)], Some(catalog_change()));
        let twice = retarget_pnpm_lock(
            &once.text,
            &[b_change(B_BEFORE, B_AFTER)],
            Some(catalog_change()),
        );
        assert_eq!(twice.text, once.text);
        assert_eq!(twice.left, []);
    }

    /// Only the lockfile document moves; the environment document's own
    /// `importers` is never read or written.
    #[test]
    fn the_environment_document_is_left_alone() {
        let lock = format!("{ENVIRONMENT}{LOCK}");
        let out = retarget_pnpm_lock(
            &lock,
            &[b_change(B_BEFORE, B_AFTER)],
            Some(catalog_change()),
        );
        assert_eq!(out.left, []);
        assert_eq!(
            out.text,
            format!("{ENVIRONMENT}{}", LOCK.replace("^1.0.0", "^2.0.0"))
        );
    }

    #[test]
    fn an_unmeasured_layout_or_format_is_named_whole() {
        let three = format!("---\nlockfileVersion: '9.0'\n{ENVIRONMENT}{LOCK}");
        let v6 = LOCK.replace("lockfileVersion: '9.0'", "lockfileVersion: '6.0'");
        let unversioned = LOCK.replace("lockfileVersion: '9.0'\n", "");
        for lock in [three, v6, unversioned, String::from("importers: [")] {
            let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
            assert_eq!(out.text, lock);
            assert!(
                matches!(
                    out.left.as_slice(),
                    [PnpmLockLeft {
                        reason: PnpmLockLeftReason::UnknownFormat { .. },
                        ..
                    }]
                ),
                "{lock}: {:?}",
                out.left
            );
        }
    }

    #[test]
    fn an_unmeasured_format_with_nothing_moved_is_not_named() {
        let v6 = LOCK.replace("lockfileVersion: '9.0'", "lockfileVersion: '6.0'");
        let out = retarget_pnpm_lock(&v6, &[b_change(B_BEFORE, B_BEFORE)], None);
        assert_eq!(out.left, []);
    }

    #[test]
    fn a_drifted_specifier_is_left_and_named() {
        let drifted = LOCK.replacen(
            "specifier: workspace:^1.0.0\n        version: link",
            "specifier: workspace:^1.1.0\n        version: link",
            1,
        );
        let out = retarget_pnpm_lock(&drifted, &[b_change(B_BEFORE, B_AFTER)], None);
        assert_eq!(out.text, drifted);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/dependencies/a",
                PnpmLockLeftReason::Drifted {
                    specifier: "workspace:^1.1.0".into(),
                    expected: "workspace:^1.0.0".into(),
                },
            )]
        );
    }

    #[test]
    fn a_registry_resolution_is_left_and_named() {
        let lock = "lockfileVersion: '9.0'\n\nimporters:\n\n  packages/b:\n    dependencies:\n      a:\n        specifier: ^1.0.0\n        version: 1.0.2\n";
        let before = "{\"dependencies\":{\"a\":\"^1.0.0\"}}";
        let after = "{\"dependencies\":{\"a\":\"^2.0.0\"}}";
        let out = retarget_pnpm_lock(lock, &[b_change(before, after)], None);
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/dependencies/a",
                PnpmLockLeftReason::Resolved {
                    version: "1.0.2".into()
                },
            )]
        );
    }

    #[test]
    fn a_registry_resolved_catalog_entry_is_left() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: ^1.0.0\n      version: 1.0.2\n\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: 1.0.2\n";
        let out = retarget_pnpm_lock(
            lock,
            &[],
            Some(PnpmCatalogChange {
                files: Some(("catalog:\n  a: ^1.0.0\n", "catalog:\n  a: ^2.0.0\n")),
                refs: &C_REF,
            }),
        );
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "catalogs/default/a",
                PnpmLockLeftReason::Resolved {
                    version: "1.0.2".into()
                },
            )]
        );
    }

    /// An exact pin repeats its specifier as its version whether or not it is
    /// linked, so only a consumer's `link:` makes it safe to rewrite.
    #[test]
    fn an_exact_catalog_pin_resolved_from_the_registry_is_left() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.0\n\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: 1.0.0\n";
        let out = exact_pin(&C_REF, lock);
        assert_eq!(out.text, lock);
        assert_eq!(out.left.len(), 1, "{:?}", out.left);

        let linked = lock.replace(
            "'catalog:'\n        version: 1.0.0",
            "'catalog:'\n        version: link:../a",
        );
        let out = exact_pin(&C_REF, &linked);
        assert_eq!(out.left, []);
        assert!(
            out.text.contains("specifier: 2.0.0\n      version: 2.0.0"),
            "{}",
            out.text
        );
    }

    /// Every consumer must link: one registry resolution among them is enough
    /// to leave the entry.
    #[test]
    fn one_unlinked_catalog_consumer_leaves_the_entry() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.0\n\nimporters:\n\n  packages/b:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: link:../a\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: 1.0.0\n";
        let refs = [
            PnpmCatalogRef {
                importer: "packages/b",
                ..C_REF[0]
            },
            C_REF[0],
        ];
        let out = exact_pin(&refs, lock);
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "catalogs/default/a",
                PnpmLockLeftReason::Resolved {
                    version: "1.0.0".into()
                }
            )]
        );
    }

    /// A link from an importer that does not take the key through the
    /// catalog says nothing about how the catalog entry resolved.
    #[test]
    fn only_catalog_consumers_count_as_link_evidence() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.0\n\nimporters:\n\n  packages/b:\n    dependencies:\n      a:\n        specifier: workspace:^1.0.0\n        version: link:../a\n";
        let refs = [PnpmCatalogRef {
            importer: "packages/b",
            ..C_REF[0]
        }];
        let out = exact_pin(&refs, lock);
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left("catalogs/default/a", PnpmLockLeftReason::Unconfirmed)]
        );
    }

    #[test]
    fn a_catalog_default_consumer_counts() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.0\n\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: catalog:default\n        version: link:../a\n";
        let out = exact_pin(&C_REF, lock);
        assert_eq!(out.left, []);
        assert!(out.text.contains("specifier: 2.0.0\n      version: 2.0.0"));
    }

    /// An unreadable consumer might be an unlinked one, so it is not dropped
    /// from the evidence.
    #[test]
    fn an_unreadable_catalog_consumer_leaves_the_entry() {
        let base = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.0\n\nimporters:\n\n  packages/b:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: link:../a\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: SPEC\n        version: VERSION\n";
        let refs = [
            PnpmCatalogRef {
                importer: "packages/b",
                ..C_REF[0]
            },
            C_REF[0],
        ];
        for (specifier, version) in [("['catalog:']", "1.0.0"), ("'catalog:'", "[link:../a]")] {
            let lock = base.replace("SPEC", specifier).replace("VERSION", version);
            let out = exact_pin(&refs, &lock);
            assert_eq!(out.text, lock, "{specifier} {version}");
            assert_eq!(
                out.left,
                [left("catalogs/default/a", PnpmLockLeftReason::Unreadable)],
                "{specifier} {version}"
            );
        }
    }

    #[test]
    fn an_odd_catalog_table_is_named() {
        let consumer = "\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: link:../a\n";
        for importers in [consumer, ""] {
            let lock = format!("lockfileVersion: '9.0'\n\ncatalogs:\n  default: [1]\n{importers}");
            let out = retarget_pnpm_lock(
                &lock,
                &[],
                Some(PnpmCatalogChange {
                    files: Some(("catalog:\n  a: ^1.0.0\n", "catalog:\n  a: ^2.0.0\n")),
                    refs: &C_REF,
                }),
            );
            assert_eq!(out.text, lock, "{importers:?}");
            assert_eq!(
                out.left,
                [left("catalogs/default/a", PnpmLockLeftReason::Unreadable)],
                "{importers:?}"
            );
        }
    }

    /// pnpm writes no catalog entry when its only consumer records the key
    /// from another section, and a frozen install accepts that lockfile.
    #[test]
    fn a_catalog_entry_pnpm_never_wrote_is_not_named() {
        let lock = "lockfileVersion: '9.0'\n\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: workspace:^1.0.0\n        version: link:../a\n";
        let out = retarget_pnpm_lock(
            lock,
            &[],
            Some(PnpmCatalogChange {
                files: Some(("catalog:\n  a: ^1.0.0\n", "catalog:\n  a: ^2.0.0\n")),
                refs: &C_REF,
            }),
        );
        assert_eq!(out.text, lock);
        assert_eq!(out.left, []);
    }

    #[test]
    fn a_named_catalog_and_a_quoted_key_are_found() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  tools:\n    '@s/a':\n      specifier: ^1.0.0\n      version: ^1.0.0\n\nimporters:\n\n  packages/c:\n    dependencies:\n      '@s/a':\n        specifier: catalog:tools\n        version: link:../a\n";
        let out = retarget_pnpm_lock(
            lock,
            &[],
            Some(PnpmCatalogChange {
                files: Some((
                    "catalogs:\n  tools:\n    '@s/a': ^1.0.0\n",
                    "catalogs:\n  tools:\n    '@s/a': ^2.0.0\n",
                )),
                refs: &[PnpmCatalogRef {
                    catalog: Some("tools"),
                    key: "@s/a",
                    importer: "packages/c",
                }],
            }),
        );
        assert_eq!(out.left, []);
        assert_eq!(out.text, lock.replace("^1.0.0", "^2.0.0"));
    }

    #[test]
    fn catalog_default_is_the_default_catalog() {
        let refs = [
            PnpmCatalogRef {
                catalog: Some("default"),
                ..C_REF[0]
            },
            C_REF[0],
        ];
        let out = retarget_pnpm_lock(
            LOCK,
            &[],
            Some(PnpmCatalogChange {
                files: Some((YAML_BEFORE, YAML_AFTER)),
                refs: &refs,
            }),
        );
        assert_eq!(out.left, []);
        assert!(out
            .text
            .contains("specifier: workspace:^2.0.0\n      version: workspace:^2.0.0"));
    }

    #[test]
    fn a_catalog_that_cannot_be_read_names_every_entry() {
        for files in [None, Some(("catalog: [", YAML_AFTER))] {
            let out = retarget_pnpm_lock(
                LOCK,
                &[],
                Some(PnpmCatalogChange {
                    files,
                    refs: &C_REF,
                }),
            );
            assert_eq!(out.text, LOCK);
            assert_eq!(
                out.left,
                [left("catalogs/default/a", PnpmLockLeftReason::Unreadable)]
            );
        }
    }

    #[test]
    fn a_catalog_pin_that_cannot_be_read_is_named() {
        let out = retarget_pnpm_lock(
            LOCK,
            &[],
            Some(PnpmCatalogChange {
                files: Some((
                    YAML_BEFORE,
                    "packages:\n  - packages/*\ncatalog:\n  b: ^1.0.0\n",
                )),
                refs: &C_REF,
            }),
        );
        assert_eq!(out.text, LOCK);
        assert_eq!(
            out.left,
            [left("catalogs/default/a", PnpmLockLeftReason::Unreadable)]
        );
    }

    /// pnpm records an auto-installed peer under the importer's
    /// `dependencies`, with the peer range as its specifier.
    #[test]
    fn an_auto_installed_peer_is_retargeted() {
        let before = "{\"peerDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"peerDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/b",
                before,
                after,
                &[("peerDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.left, []);
        assert!(out
            .text
            .contains("specifier: workspace:^2.0.0\n        version: link:../a"));
    }

    #[test]
    fn a_peer_with_no_entry_is_not_named() {
        let before =
            "{\"peerDependencies\":{\"a\":\"^1.0.0\"},\"devDependencies\":{\"a\":\"^1.0.0\"}}";
        let after =
            "{\"peerDependencies\":{\"a\":\"^2.0.0\"},\"devDependencies\":{\"a\":\"^1.0.0\"}}";
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/a",
                before,
                after,
                &[("peerDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, LOCK);
        assert_eq!(out.left, []);
    }

    /// The `dependencies` entry declares an unbumped member under the same
    /// key, so it is not an auto-installed peer.
    #[test]
    fn a_peer_does_not_stand_in_for_an_ordinary_declaration() {
        let before = "{\"dependencies\":{\"a\":\"workspace:^1.0.0\"},\"peerDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"dependencies\":{\"a\":\"workspace:^1.0.0\"},\"peerDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/b",
                before,
                after,
                &[("peerDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, LOCK);
        assert_eq!(out.left, []);
    }

    /// With the key also in `devDependencies`, pnpm records that declaration,
    /// so a `dependencies` entry is not the peer's.
    #[test]
    fn a_peer_does_not_stand_in_beside_another_declaration() {
        let before = "{\"devDependencies\":{\"a\":\"workspace:*\"},\"peerDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"devDependencies\":{\"a\":\"workspace:*\"},\"peerDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/b",
                before,
                after,
                &[("peerDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, LOCK);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/dependencies/a",
                PnpmLockLeftReason::Unreadable
            )]
        );
    }

    #[test]
    fn a_peer_stands_in_only_under_dependencies() {
        let lock = LOCK.replace(
            "  packages/b:\n    dependencies:",
            "  packages/b:\n    devDependencies:",
        );
        let before = "{\"peerDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"peerDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            &lock,
            &[change(
                "packages/b",
                before,
                after,
                &[("peerDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/devDependencies/a",
                PnpmLockLeftReason::Unreadable
            )]
        );
    }

    #[test]
    fn a_recorded_section_the_manifest_does_not_declare_is_named() {
        let lock = LOCK.replace(
            "  packages/b:\n    dependencies:",
            "  packages/b:\n    devDependencies:",
        );
        let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/devDependencies/a",
                PnpmLockLeftReason::Unreadable
            )]
        );
    }

    /// A key in `dependencies` and `devDependencies` is recorded once.
    #[test]
    fn a_key_in_two_sections_is_one_entry() {
        let before = "{\"dependencies\":{\"a\":\"workspace:^1.0.0\"},\"devDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"dependencies\":{\"a\":\"workspace:^2.0.0\"},\"devDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/b",
                before,
                after,
                &[("dependencies", "a"), ("devDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.left, []);
        assert!(out
            .text
            .contains("specifier: workspace:^2.0.0\n        version: link:../a"));
    }

    /// A null section is no section: the entry pnpm did record is still found.
    #[test]
    fn a_null_section_does_not_hide_the_recorded_one() {
        let lock = LOCK.replace(
            "  packages/b:\n    dependencies:",
            "  packages/b:\n    dependencies: null\n    optionalDependencies:",
        );
        let before = "{\"optionalDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"optionalDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            &lock,
            &[change(
                "packages/b",
                before,
                after,
                &[("optionalDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.left, []);
        assert_eq!(
            out.text,
            lock.replacen(
                "workspace:^1.0.0\n        version: link",
                "workspace:^2.0.0\n        version: link",
                1
            )
        );
    }

    /// A peer range moved but pnpm recorded the unchanged dev range, so this
    /// run invalidated nothing there, drift that predates it included.
    #[test]
    fn an_unmoved_recorded_declaration_is_not_named() {
        let before = "{\"peerDependencies\":{\"a\":\"workspace:^1.0.0\"},\"devDependencies\":{\"a\":\"workspace:*\"}}";
        let after = "{\"peerDependencies\":{\"a\":\"workspace:^2.0.0\"},\"devDependencies\":{\"a\":\"workspace:*\"}}";
        for recorded in ["workspace:*", "workspace:^0.1.0"] {
            let lock = LOCK.replace(
                "  packages/b:\n    dependencies:\n      a:\n        specifier: workspace:^1.0.0",
                &format!(
                    "  packages/b:\n    devDependencies:\n      a:\n        specifier: {recorded}"
                ),
            );
            let out = retarget_pnpm_lock(
                &lock,
                &[change(
                    "packages/b",
                    before,
                    after,
                    &[("peerDependencies", "a")],
                )],
                None,
            );
            assert_eq!(out.text, lock, "{recorded}");
            assert_eq!(out.left, [], "{recorded}");
        }
    }

    /// A drifted entry is compared with its own section's ranges, never
    /// matched against another section's move.
    #[test]
    fn a_drifted_entry_is_not_matched_across_sections() {
        let lock = LOCK.replace(
            "specifier: workspace:^1.0.0\n        version: link",
            "specifier: workspace:>=1.0.0\n        version: link",
        );
        let before = "{\"dependencies\":{\"a\":\"workspace:^1.0.0\"},\"devDependencies\":{\"a\":\"workspace:>=1.0.0\"}}";
        let after = "{\"dependencies\":{\"a\":\"workspace:^2.0.0\"},\"devDependencies\":{\"a\":\"workspace:>=2.0.0\"}}";
        let out = retarget_pnpm_lock(
            &lock,
            &[change(
                "packages/b",
                before,
                after,
                &[("dependencies", "a"), ("devDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/dependencies/a",
                PnpmLockLeftReason::Drifted {
                    specifier: "workspace:>=1.0.0".into(),
                    expected: "workspace:^1.0.0".into(),
                },
            )]
        );
    }

    #[test]
    fn an_unreadable_peer_entry_is_named() {
        let lock = LOCK.replace(
            "specifier: workspace:^1.0.0\n        version: link",
            "specifier: [workspace:^1.0.0]\n        version: link",
        );
        let before = "{\"peerDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"peerDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            &lock,
            &[change(
                "packages/b",
                before,
                after,
                &[("peerDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, lock);
        assert_eq!(out.left[0].reason, PnpmLockLeftReason::Unreadable);
    }

    #[test]
    fn a_missing_importer_entry_is_named() {
        let mut moved = b_change(B_BEFORE, B_AFTER);
        moved.importer = "packages/z";
        let out = retarget_pnpm_lock(LOCK, &[moved], None);
        assert_eq!(out.text, LOCK);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/z/dependencies/a",
                PnpmLockLeftReason::Missing
            )]
        );
    }

    /// An importer or section pnpm wrote in a shape nobody reads is reported,
    /// never taken for an absent entry.
    #[test]
    fn an_odd_importer_or_section_is_named() {
        for odd in [
            "  packages/b: 7\n",
            "  packages/b:\n    dependencies: [x]\n",
        ] {
            let lock = LOCK.replace(
                "  packages/b:\n    dependencies:\n      a:\n        specifier: workspace:^1.0.0\n        version: link:../a\n",
                odd,
            );
            let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
            assert_eq!(out.text, lock, "{odd}");
            assert_eq!(
                out.left,
                [left(
                    "importers/packages/b/dependencies/a",
                    PnpmLockLeftReason::Unreadable
                )],
                "{odd}"
            );
        }
    }

    #[test]
    fn a_catalog_consumer_that_is_absent_or_odd_leaves_the_entry() {
        let base = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.0\n\nimporters:\n\n  packages/b:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: link:../a\nCONSUMER";
        let refs = [
            PnpmCatalogRef {
                importer: "packages/b",
                ..C_REF[0]
            },
            C_REF[0],
        ];
        for consumer in [
            "",
            "\n  packages/c: [x]\n",
            "\n  packages/c:\n    dependencies: [x]\n",
        ] {
            let lock = base.replace("CONSUMER", consumer);
            let out = exact_pin(&refs, &lock);
            assert_eq!(out.text, lock, "{consumer:?}");
            assert_eq!(
                out.left,
                [left("catalogs/default/a", PnpmLockLeftReason::Unreadable)],
                "{consumer:?}"
            );
        }
    }

    /// Every consumer links, but the catalog entry's own version is not the
    /// pin it records, so rewriting it would invent a resolution.
    #[test]
    fn a_catalog_entry_whose_version_is_not_its_pin_is_left() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: 1.0.0\n      version: 1.0.3\n\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: link:../a\n";
        let out = exact_pin(&C_REF, lock);
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "catalogs/default/a",
                PnpmLockLeftReason::Resolved {
                    version: "1.0.3".into()
                }
            )]
        );
    }

    #[test]
    fn a_catalog_consumer_resolution_names_the_consumers_version() {
        let lock = "lockfileVersion: '9.0'\n\ncatalogs:\n  default:\n    a:\n      specifier: ^1.0.0\n      version: ^1.0.0\n\nimporters:\n\n  packages/c:\n    dependencies:\n      a:\n        specifier: 'catalog:'\n        version: 1.0.2\n";
        let out = retarget_pnpm_lock(
            lock,
            &[],
            Some(PnpmCatalogChange {
                files: Some(("catalog:\n  a: ^1.0.0\n", "catalog:\n  a: ^2.0.0\n")),
                refs: &C_REF,
            }),
        );
        assert_eq!(
            out.left,
            [left(
                "catalogs/default/a",
                PnpmLockLeftReason::Resolved {
                    version: "1.0.2".into()
                }
            )]
        );
    }

    /// A write through an alias would land on the entry that owns the anchor.
    #[test]
    fn a_lockfile_with_anchors_is_named_whole() {
        let lock = LOCK.replace(
            "specifier: workspace:^1.0.0\n        version: link:../a",
            "specifier: &s workspace:^1.0.0\n        version: link:../a",
        );
        let lock = format!("{lock}\n  packages/z:\n    dependencies:\n      a:\n        specifier: *s\n        version: 1.0.0\n");
        let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
        assert_eq!(out.text, lock);
        assert!(
            matches!(
                out.left.as_slice(),
                [PnpmLockLeft {
                    reason: PnpmLockLeftReason::UnknownFormat { .. },
                    ..
                }]
            ),
            "{:?}",
            out.left
        );
    }

    /// Only pnpm's own environment document may precede the lockfile.
    #[test]
    fn a_first_document_that_is_not_the_environment_is_named_whole() {
        for first in [
            "---\nlockfileVersion: '9.0'\nfoo: bar\n---\n",
            "lockfileVersion: '9.0'\n---\n",
            "---\n# configDependencies\nlockfileVersion: '9.0'\n---\n",
            "---\nimporters:\n  .:\n    configDependencies: [\n---\n",
            "---\nimporters:\n  .:\n    packageManagerDependencies: {}\n  other:\n    configDependencies: {}\n---\n",
        ] {
            let lock = format!("{first}{LOCK}");
            let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
            assert_eq!(out.text, lock, "{first}");
            assert!(
                matches!(
                    out.left.as_slice(),
                    [PnpmLockLeft {
                        reason: PnpmLockLeftReason::UnknownFormat { .. },
                        ..
                    }]
                ),
                "{first}: {:?}",
                out.left
            );
        }
    }

    /// saphyr-parser 0.2 rejects a raw NUL, which is a parse failure, not an
    /// anchor.
    #[test]
    fn a_lockfile_the_parser_rejects_is_named_unparsable() {
        let lock = LOCK.replace("packages/a: {}", "packages/a: {}\0");
        let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
        assert_eq!(out.text, lock);
        assert_eq!(
            out.left,
            [left(
                "pnpm-lock.yaml",
                PnpmLockLeftReason::UnknownFormat {
                    detail: "the lockfile could not be parsed".into()
                }
            )]
        );
    }

    #[test]
    fn a_numeric_lockfile_version_is_named_as_such() {
        let lock = LOCK.replace("lockfileVersion: '9.0'", "lockfileVersion: 9.0");
        let out = retarget_pnpm_lock(&lock, &[b_change(B_BEFORE, B_AFTER)], None);
        assert_eq!(
            out.left,
            [left(
                "pnpm-lock.yaml",
                PnpmLockLeftReason::UnknownFormat {
                    detail: "lockfileVersion is not a quoted string, as pnpm 9 and later write it"
                        .into()
                }
            )]
        );
    }

    /// A key the manifest drops on one side was not rewritten by this run,
    /// so nothing pairs the lock entry with a range.
    #[test]
    fn a_declaration_on_one_side_only_is_named() {
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/b",
                B_BEFORE,
                "{\"name\":\"b\"}",
                &[("dependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, LOCK);
        assert_eq!(
            out.left,
            [left(
                "importers/packages/b/dependencies/a",
                PnpmLockLeftReason::Unreadable
            )]
        );
    }

    /// pnpm records a key once, but a hand-edited lock that records it twice
    /// gets every recorded entry retargeted.
    #[test]
    fn every_recorded_section_is_retargeted() {
        let lock = LOCK.replace(
            "  packages/b:\n    dependencies:\n      a:\n        specifier: workspace:^1.0.0\n        version: link:../a\n",
            "  packages/b:\n    dependencies:\n      a:\n        specifier: workspace:^1.0.0\n        version: link:../a\n    optionalDependencies:\n      a:\n        specifier: workspace:^1.0.0\n        version: link:../a\n",
        );
        let before = "{\"dependencies\":{\"a\":\"workspace:^1.0.0\"},\"optionalDependencies\":{\"a\":\"workspace:^1.0.0\"}}";
        let after = "{\"dependencies\":{\"a\":\"workspace:^2.0.0\"},\"optionalDependencies\":{\"a\":\"workspace:^2.0.0\"}}";
        let out = retarget_pnpm_lock(
            &lock,
            &[change(
                "packages/b",
                before,
                after,
                &[("dependencies", "a"), ("optionalDependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.left, []);
        assert_eq!(
            out.text,
            lock.replace(
                "specifier: workspace:^1.0.0\n        version: link",
                "specifier: workspace:^2.0.0\n        version: link"
            )
        );
    }

    #[test]
    fn a_manifest_value_that_cannot_be_read_is_named() {
        let out = retarget_pnpm_lock(
            LOCK,
            &[change(
                "packages/b",
                B_BEFORE,
                "{\"dependencies\":{\"a\":2}}",
                &[("dependencies", "a")],
            )],
            None,
        );
        assert_eq!(out.text, LOCK);
        assert_eq!(out.left[0].reason, PnpmLockLeftReason::Unreadable);
    }

    /// A plain `*` reads back as an alias, so the write is refused.
    #[test]
    fn a_value_the_quoting_cannot_carry_is_not_written() {
        let after = "{\"dependencies\":{\"a\":\"*\"}}";
        let out = retarget_pnpm_lock(LOCK, &[b_change(B_BEFORE, after)], None);
        assert_eq!(out.text, LOCK);
        assert_eq!(out.left[0].reason, PnpmLockLeftReason::NotRewritable);
    }
}
