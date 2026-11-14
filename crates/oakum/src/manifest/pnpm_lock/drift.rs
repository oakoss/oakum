//! Compare the specifiers `pnpm-lock.yaml` records for each workspace project
//! and catalog entry with what the manifests and `pnpm-workspace.yaml` declare.
//! Only specifiers: settings, `dependenciesMeta`, `publishConfig.directory` and
//! patches are left to `pnpm install --frozen-lockfile`.
//!
//! The rules follow what pnpm 12.16.1 accepted and refused on measured
//! fixtures: an importer entry matches its manifest by exact specifier in the
//! section pnpm files it under, and a catalog entry the lockfile records
//! matches `pnpm-workspace.yaml`. A catalog entry no importer uses is not
//! recorded and not compared. Entries the lockfile's own `overrides`,
//! `ignoredOptionalDependencies` or package extensions account for are skipped
//! and counted rather than compared.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

use super::{read_model, Found, Model, Node, Text, IMPORTER_SECTIONS};
use crate::manifest::catalog_file::CatalogFile;
use crate::plan::bounds::Bounds;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PnpmLockDrift {
    pub drifted: Vec<PnpmLockDrifted>,
    /// Entries or files that could not be read, so their drift is unknown.
    pub unread: Vec<String>,
    /// Importer entries not compared because an override, an ignored optional
    /// dependency or a package extension accounts for them.
    pub skipped: usize,
}

/// One entry where the lockfile and its source disagree. `None` on one side
/// is an entry that side lacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PnpmLockDrifted {
    /// Slash-joined lockfile path, as `importers/<importer>/<section>/<key>`
    /// or `catalogs/<catalog>/<key>`.
    pub entry: String,
    /// The file the lockfile should agree with, relative to the lockfile's
    /// directory.
    pub source: String,
    pub recorded: Option<String>,
    pub declared: Option<String>,
}

impl core::fmt::Display for PnpmLockDrifted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self {
            entry,
            source,
            recorded,
            declared,
        } = self;
        match (recorded, declared) {
            (Some(recorded), Some(declared)) => write!(
                f,
                "`{entry}` records `{recorded}`, where `{source}` declares `{declared}`"
            ),
            (Some(recorded), None) => write!(
                f,
                "`{entry}` records `{recorded}`, which `{source}` does not declare"
            ),
            (None, Some(declared)) => write!(
                f,
                "`{source}` declares `{declared}`, which `{entry}` does not record"
            ),
            (None, None) => write!(f, "`{entry}` disagrees with `{source}`"),
        }
    }
}

/// One workspace project's manifest, read by the caller.
pub struct PnpmManifest {
    pub importer: String,
    /// `Ok(None)` when the directory has no `package.json`; `Err` says why it
    /// could not be read.
    pub text: Result<Option<String>, String>,
}

/// The manifest fields pnpm reads for an importer.
#[derive(Deserialize)]
struct Declared {
    #[serde(default)]
    dependencies: Option<BTreeMap<String, String>>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: Option<BTreeMap<String, String>>,
    #[serde(rename = "optionalDependencies", default)]
    optional_dependencies: Option<BTreeMap<String, String>>,
    #[serde(rename = "peerDependencies", default)]
    peer_dependencies: Option<BTreeMap<String, String>>,
}

type Sections = BTreeMap<&'static str, BTreeMap<String, String>>;

/// Compares `text`, the lockfile in `lock_dir` (repository-relative, empty for
/// the root), with `manifests`, one per workspace project, and the
/// `pnpm-workspace.yaml` text in `workspace`.
///
/// # Errors
///
/// Why the whole file is in a shape nobody measured, or one whose specifiers a
/// pnpmfile may have rewritten, so nothing was compared.
pub fn pnpm_lock_drift(
    text: &str,
    lock_dir: &str,
    manifests: &[PnpmManifest],
    workspace: Option<&str>,
) -> Result<PnpmLockDrift, String> {
    let (_, _, model) = read_model(text)?;
    if model.pnpmfile_checksum.is_some() {
        return Err(String::from(
            "the lockfile records a pnpmfile (`pnpmfileChecksum`), whose `readPackage` hook can rewrite any specifier",
        ));
    }
    let Some(importers) = &model.importers else {
        return Err(String::from(
            "the lockfile has no importers, which pnpm 9 and later write",
        ));
    };
    let (auto_install_peers, exclude_links) = settings(&model);
    let mut drift = PnpmLockDrift::default();
    let skips = Skips::of(&model, lock_dir);
    for manifest in manifests {
        let text = match &manifest.text {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(why) => {
                drift.unread.push(why.clone());
                continue;
            }
        };
        let source = if manifest.importer == "." {
            String::from("package.json")
        } else {
            format!("{}/package.json", manifest.importer)
        };
        let declared = match serde_json::from_str::<Declared>(text) {
            Ok(declared) => declared,
            Err(err) => {
                drift.unread.push(format!(
                    "`{source}` does not parse as a manifest pnpm reads ({err})"
                ));
                continue;
            }
        };
        let Ok(identity) = serde_json::from_str::<Identity>(text) else {
            drift.unread.push(format!(
                "`{source}` does not parse as a manifest with a name and version pnpm reads"
            ));
            continue;
        };
        let project = Project {
            importer: &manifest.importer,
            name: identity.name,
            version: identity.version,
        };
        let expected = expected_sections(declared, auto_install_peers);
        let recorded = match Found::of(importers.get(&manifest.importer)) {
            Found::Absent => Sections::new(),
            Found::Odd => {
                drift.unread.push(format!(
                    "`importers/{}` is not a mapping",
                    manifest.importer
                ));
                continue;
            }
            Found::Value(importer) => {
                recorded_sections(importer, &manifest.importer, &mut drift.unread)
            }
        };
        for section in IMPORTER_SECTIONS {
            let empty = BTreeMap::new();
            let recorded = recorded.get(section).unwrap_or(&empty);
            let expected = expected.get(section).unwrap_or(&empty);
            let keys: BTreeSet<&String> = recorded.keys().chain(expected.keys()).collect();
            for key in keys {
                let (recorded, declared) = (recorded.get(key), expected.get(key));
                if recorded == declared {
                    continue;
                }
                if skips.cover(&project, section, key, recorded, declared) {
                    drift.skipped += 1;
                    continue;
                }
                let entry = ["importers", &manifest.importer, section, key].join("/");
                if let (true, None, Some(declared)) = (exclude_links, recorded, declared) {
                    // Measured under excludeLinksFromLockfile: a `link:` entry is
                    // left out and installs; a `file:` or `catalog:` one is left
                    // out and pnpm refuses its own lockfile.
                    if declared.starts_with("link:") {
                        continue;
                    }
                    if declared.starts_with("file:") || declared.starts_with("catalog:") {
                        drift.unread.push(format!(
                            "`{entry}` is left out under excludeLinksFromLockfile, where pnpm refuses its own lockfile"
                        ));
                        continue;
                    }
                }
                drift.drifted.push(PnpmLockDrifted {
                    entry,
                    source: source.clone(),
                    recorded: recorded.cloned(),
                    declared: declared.cloned(),
                });
            }
        }
    }
    compare_catalogs(&model, workspace, &mut drift);
    drift.drifted.sort_by(|a, b| a.entry.cmp(&b.entry));
    Ok(drift)
}

/// What the lockfile records pnpm did to its importers beyond the manifests.
/// The importer an entry belongs to and its manifest's package name.
struct Project<'a> {
    importer: &'a str,
    name: Option<String>,
    version: Option<String>,
}

/// A manifest's `name` and `version` as pnpm reads them: the last of a
/// duplicated key, and a value that is not a string as none (measured,
/// 12.16.1). Every other value is skipped unchecked, as `Declared` skips it,
/// and these two are taken raw, so a value `serde_json` refuses reads as none.
#[derive(Default)]
struct Identity {
    name: Option<String>,
    version: Option<String>,
}

impl<'de> Deserialize<'de> for Identity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Identity;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("a manifest object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Identity, A::Error> {
                let mut identity = Identity::default();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    let slot = match key.as_ref() {
                        "name" => &mut identity.name,
                        "version" => &mut identity.version,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                            continue;
                        }
                    };
                    let raw = map.next_value::<&'de serde_json::value::RawValue>()?;
                    *slot = serde_json::from_str::<StringOrNone>(raw.get())
                        .ok()
                        .and_then(|value| value.0);
                }
                Ok(identity)
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

/// A string, or `None` for any other value, skipped unchecked.
struct StringOrNone(Option<String>);

impl<'de> Deserialize<'de> for StringOrNone {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Value;
        impl<'de> Visitor<'de> for Value {
            type Value = StringOrNone;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("any value")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<StringOrNone, E> {
                Ok(StringOrNone(Some(value.to_owned())))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<StringOrNone, E> {
                Ok(StringOrNone(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<StringOrNone, E> {
                Ok(StringOrNone(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<StringOrNone, E> {
                Ok(StringOrNone(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<StringOrNone, E> {
                Ok(StringOrNone(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<StringOrNone, E> {
                Ok(StringOrNone(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StringOrNone, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(StringOrNone(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StringOrNone, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(StringOrNone(None))
            }
        }
        deserializer.deserialize_any(Value)
    }
}

/// An override selector's parent package, with the range its version must
/// satisfy for the override to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Parent<'a> {
    name: &'a str,
    range: Option<&'a str>,
}

impl Parent<'_> {
    /// Measured on pnpm 12.16.1: a range, even `*`, needs a version that
    /// satisfies it, so a versionless project or an unparsable range never
    /// matches; no range matches any version, or none. One leading `v` is
    /// read past; `=` and blanks are not.
    fn matches(&self, project: &Project<'_>) -> bool {
        project.name.as_deref() == Some(self.name)
            && self.range.is_none_or(|range| {
                let version = project
                    .version
                    .as_deref()
                    .map(|version| Version::parse(version.strip_prefix('v').unwrap_or(version)));
                match (version, Bounds::from_npm_text(range)) {
                    // node-semver refuses a component past 2^53 - 1 that Rust
                    // semver accepts.
                    (Some(Ok(version)), Ok(bounds))
                        if version.to_string().parse::<js_semver::Version>().is_ok() =>
                    {
                        bounds.matches(&version)
                    }
                    _ => false,
                }
            })
    }
}

struct Skips<'a> {
    /// `(parent, value)` by target package. What pnpm applied is what the
    /// lockfile records; pnpm 12.16.1 ignores a root `pnpm.overrides`.
    overrides: BTreeMap<&'a str, Vec<(Option<Parent<'a>>, &'a str)>>,
    ignored_optional: &'a [String],
    extended: bool,
    lock_dir: &'a str,
}

impl<'a> Skips<'a> {
    fn of(model: &'a Model, lock_dir: &'a str) -> Self {
        let mut overrides: BTreeMap<&str, Vec<(Option<Parent<'_>>, &str)>> = BTreeMap::new();
        for (key, value) in model.overrides.iter().flatten() {
            if let Some(value) = value.as_str() {
                let (parent, target) = override_selector(key);
                overrides.entry(target).or_default().push((parent, value));
            }
        }
        Self {
            overrides,
            ignored_optional: model
                .ignored_optional_dependencies
                .as_deref()
                .unwrap_or(&[]),
            extended: model.package_extensions_checksum.is_some(),
            lock_dir,
        }
    }

    /// Only an override pnpm applied counts: the recorded specifier is its
    /// value, not the declared one. A selector's parent or range can keep it
    /// from this importer, and the lockfile lists it all the same.
    fn cover(
        &self,
        project: &Project<'_>,
        section: &str,
        key: &str,
        recorded: Option<&String>,
        declared: Option<&String>,
    ) -> bool {
        let overridden = self.overrides.get(key).is_some_and(|values| {
            values.iter().any(|(parent, value)| match recorded {
                // `-` removes the dependency, so pnpm records nothing; nothing
                // else says it applied, so only a parent this project matches.
                None => *value == "-" && parent.is_none_or(|parent| parent.matches(project)),
                Some(recorded) => {
                    applied(value, self.lock_dir, project.importer).as_deref()
                        == Some(recorded.as_str())
                }
            })
        });
        let ignored_optional = section == "optionalDependencies"
            && recorded.is_none()
            && ignored(self.ignored_optional, key);
        // An extension adds entries the manifest lacks; declared ones still
        // compare.
        overridden || ignored_optional || (self.extended && declared.is_none())
    }
}

/// An override value as pnpm records it in `importer`: a `link:` or `file:`
/// path is written from the workspace root and recorded from the importer.
/// Both resolve through `lock_dir`, so an importer outside it still matches.
fn applied(value: &str, lock_dir: &str, importer: &str) -> Option<String> {
    let Some((protocol, path)) = ["link:", "file:"]
        .into_iter()
        .find_map(|protocol| Some((protocol, value.strip_prefix(protocol)?)))
    else {
        return Some(value.to_owned());
    };
    let normal = |under: &str, path: &str| -> Option<Vec<String>> {
        let mut parts: Vec<String> = Vec::new();
        for part in under.split('/').chain(path.split('/')) {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                part => parts.push(part.to_owned()),
            }
        }
        Some(parts)
    };
    let (target, from) = (normal(lock_dir, path)?, normal(lock_dir, importer)?);
    let shared = target.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut steps = vec![String::from(".."); from.len() - shared];
    steps.extend(target[shared..].iter().cloned());
    Some(format!("{protocol}{}", steps.join("/")))
}

/// Whether pnpm's `ignoredOptionalDependencies` patterns leave `name` out:
/// `*` matches any run of characters, `/` included, and the last pattern that
/// matches decides, `!` excluding. A list opening with `!` starts from
/// everything.
pub(crate) fn ignored(patterns: &[String], name: &str) -> bool {
    let mut ignored = patterns.first().is_some_and(|first| first.starts_with('!'));
    for pattern in patterns {
        match pattern.strip_prefix('!') {
            Some(negated) if wildcard(negated, name) => ignored = false,
            None if wildcard(pattern, name) => ignored = true,
            _ => {}
        }
    }
    ignored
}

fn wildcard(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => name.strip_prefix(head).is_some_and(|rest| {
            (0..=rest.len())
                .filter(|&at| rest.is_char_boundary(at))
                .any(|at| wildcard(tail, &rest[at..]))
        }),
    }
}

/// `(autoInstallPeers, excludeLinksFromLockfile)`, at pnpm's defaults where
/// the lockfile does not record a boolean.
fn settings(model: &Model) -> (bool, bool) {
    let flag = |node: Option<&Node<bool>>, default| match node {
        Some(Node::Value(value)) => *value,
        _ => default,
    };
    match &model.settings {
        Some(Node::Value(settings)) => (
            flag(settings.auto_install_peers.as_ref(), true),
            flag(settings.exclude_links_from_lockfile.as_ref(), false),
        ),
        _ => (true, false),
    }
}

#[cfg(test)]
fn override_target(selector: &str) -> &str {
    override_selector(selector).1
}

/// `(parent, target)` of an override selector, the target without its range
/// (`a@1>@s/b@^2` is a parent `a` at `1` and `@s/b`). pnpm splits parent from
/// child at the first `>` not preceded by a space, `|` or `@`, so `@s/a@>=1`
/// is one selector.
fn override_selector(selector: &str) -> (Option<Parent<'_>>, &str) {
    let bytes = selector.as_bytes();
    let split = (1..bytes.len())
        .find(|&at| bytes[at] == b'>' && !matches!(bytes[at - 1], b' ' | b'|' | b'@'));
    match split {
        Some(at) => {
            let (name, range) = package_and_range(&selector[..at]);
            (
                Some(Parent { name, range }),
                package_name(&selector[at + 1..]),
            )
        }
        None => (None, package_name(selector)),
    }
}

/// A package selector without its version range.
fn package_name(selector: &str) -> &str {
    package_and_range(selector).0
}

/// A package selector split into its name and the range after its `@`.
fn package_and_range(selector: &str) -> (&str, Option<&str>) {
    let from = usize::from(selector.starts_with('@'));
    match selector[from..].find('@') {
        Some(at) => (&selector[..from + at], Some(&selector[from + at + 1..])),
        None => (selector, None),
    }
}

/// Where pnpm files each declared key: `optionalDependencies` over
/// `dependencies` over `devDependencies`, and a peer no other section declares
/// under `dependencies` when pnpm installs peers.
fn expected_sections(declared: Declared, auto_install_peers: bool) -> Sections {
    let optional = declared.optional_dependencies.unwrap_or_default();
    let dependencies = declared.dependencies.unwrap_or_default();
    let dev: BTreeMap<String, String> = declared
        .dev_dependencies
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| !optional.contains_key(key) && !dependencies.contains_key(key))
        .collect();
    let mut ordinary: BTreeMap<String, String> = dependencies
        .into_iter()
        .filter(|(key, _)| !optional.contains_key(key))
        .collect();
    if auto_install_peers {
        for (key, range) in declared.peer_dependencies.unwrap_or_default() {
            if !optional.contains_key(&key)
                && !ordinary.contains_key(&key)
                && !dev.contains_key(&key)
            {
                ordinary.insert(key, range);
            }
        }
    }
    Sections::from([
        ("dependencies", ordinary),
        ("devDependencies", dev),
        ("optionalDependencies", optional),
    ])
}

fn recorded_sections(importer: &super::Importer, name: &str, unread: &mut Vec<String>) -> Sections {
    let mut sections = Sections::new();
    for section in IMPORTER_SECTIONS {
        let entries = match section {
            "dependencies" => &importer.dependencies,
            "devDependencies" => &importer.dev_dependencies,
            _ => &importer.optional_dependencies,
        };
        let entries = match Found::of(entries.as_ref()) {
            Found::Absent => continue,
            Found::Odd => {
                unread.push(format!("`importers/{name}/{section}` is not a mapping"));
                continue;
            }
            Found::Value(entries) => entries,
        };
        let mut specifiers = BTreeMap::new();
        for (key, entry) in entries {
            match specifier(entry) {
                Some(specifier) => {
                    specifiers.insert(key.clone(), specifier.to_owned());
                }
                None => unread.push(format!(
                    "`importers/{name}/{section}/{key}` has no specifier string"
                )),
            }
        }
        sections.insert(section, specifiers);
    }
    sections
}

fn specifier(entry: &Node<super::Entry>) -> Option<&str> {
    match entry {
        Node::Value(entry) => entry.specifier.as_ref().and_then(Text::as_str),
        Node::Other(_) => None,
    }
}

/// Each catalog entry the lockfile records against `pnpm-workspace.yaml`.
fn compare_catalogs(model: &Model, catalog: Option<&str>, drift: &mut PnpmLockDrift) {
    let Some(catalogs) = &model.catalogs else {
        return;
    };
    let file = match catalog.map(CatalogFile::parse) {
        None => None,
        Some(Ok(file)) => Some(file),
        Some(Err(_)) => {
            drift
                .unread
                .push(String::from("`pnpm-workspace.yaml` could not be parsed"));
            return;
        }
    };
    for (name, entries) in catalogs {
        let entries = match entries {
            Node::Value(entries) => entries,
            Node::Other(_) => {
                drift
                    .unread
                    .push(format!("`catalogs/{name}` is not a mapping"));
                continue;
            }
        };
        for (key, entry) in entries {
            let entry_path = format!("catalogs/{name}/{key}");
            let Some(recorded) = specifier(entry) else {
                drift
                    .unread
                    .push(format!("`{entry_path}` has no specifier string"));
                continue;
            };
            // pnpm records the default catalog as `default`, whichever of the
            // two spellings declares it.
            let declared = file.as_ref().and_then(|file| {
                if name == "default" {
                    file.string_at(&["catalog", key])
                        .or_else(|| file.string_at(&["catalogs", "default", key]))
                } else {
                    file.string_at(&["catalogs", name, key])
                }
            });
            if declared != Some(recorded) {
                drift.drifted.push(PnpmLockDrifted {
                    entry: entry_path,
                    source: String::from("pnpm-workspace.yaml"),
                    recorded: Some(recorded.to_owned()),
                    declared: declared.map(str::to_owned),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{override_target, Identity, PnpmLockDrift, PnpmManifest};

    /// `Identity` reads past every value `Declared` skips, however deep.
    #[test]
    fn identity_reads_past_what_a_full_parse_refuses() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let text = format!(
            r#"{{"name":"b","x":{deep},"version":1,"version":"2.0.0","y":1e400,"z":"\ud800","name":{{"n":[1]}},"name":"b"}}"#
        );
        let identity = serde_json::from_str::<Identity>(&text).expect("read");
        assert_eq!(identity.name.as_deref(), Some("b"));
        assert_eq!(identity.version.as_deref(), Some("2.0.0"));
        let other =
            serde_json::from_str::<Identity>(r#"{"name":["b"],"version":2}"#).expect("read");
        assert_eq!((other.name, other.version), (None, None));
    }

    fn pnpm_lock_drift(
        text: &str,
        manifests: &[PnpmManifest],
        workspace: Option<&str>,
    ) -> Result<PnpmLockDrift, String> {
        super::pnpm_lock_drift(text, "", manifests, workspace)
    }

    /// `pnpm install --lockfile-only --offline` under pnpm 12.16.1 for
    /// [`MANIFESTS`] and [`CATALOG`]; a frozen install accepts it.
    const LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

catalogs:
  default:
    a:
      specifier: workspace:^1.0.0
      version: workspace:^1.0.0
    al:
      specifier: npm:d@^1.0.0
      version: npm:d@^1.0.0
  tools:
    c:
      specifier: workspace:*
      version: workspace:*

importers:

  .:
    devDependencies:
      a:
        specifier: workspace:*
        version: link:packages/a

  packages/a: {}

  packages/b:
    dependencies:
      a:
        specifier: 'catalog:'
        version: link:../a
      aa:
        specifier: workspace:a@^1.0.0
        version: link:../a
      al:
        specifier: 'catalog:'
        version: link:../d
      cl:
        specifier: link:../c
        version: link:../c
      dd:
        specifier: file:../d
        version: link:../d
      na:
        specifier: npm:a@^1.0.0
        version: link:../a
    devDependencies:
      c:
        specifier: catalog:tools
        version: link:../c
      d:
        specifier: workspace:~1.0.0
        version: link:../d
    optionalDependencies:
      a2:
        specifier: workspace:a@*
        version: link:../a

  packages/c:
    dependencies:
      a:
        specifier: workspace:^1.0.0
        version: link:../a
      d:
        specifier: workspace:*
        version: link:../d

  packages/d: {}
";

    /// `b` declares a peer an ordinary section shadows; `c`'s peers are
    /// installed under `dependencies`.
    const MANIFESTS: [(&str, &str); 5] = [
        (
            ".",
            r#"{"name":"root","private":true,"devDependencies":{"a":"workspace:*"}}"#,
        ),
        ("packages/a", r#"{"name":"a","version":"1.0.0"}"#),
        (
            "packages/b",
            r#"{"name":"b","version":"1.0.0","dependencies":{"a":"catalog:","aa":"workspace:a@^1.0.0","cl":"link:../c","dd":"file:../d","na":"npm:a@^1.0.0","al":"catalog:"},"devDependencies":{"c":"catalog:tools","d":"workspace:~1.0.0"},"optionalDependencies":{"a2":"workspace:a@*"},"peerDependencies":{"a":"^1.0.0","c":"workspace:^"}}"#,
        ),
        (
            "packages/c",
            r#"{"name":"c","version":"2.0.0","peerDependencies":{"a":"workspace:^1.0.0","d":"workspace:*"}}"#,
        ),
        ("packages/d", r#"{"name":"d","version":"1.0.0"}"#),
    ];

    /// `unused` is a catalog no importer references, so pnpm records none of it.
    const CATALOG: &str = "packages:
  - packages/*
linkWorkspacePackages: true
catalog:
  al: npm:d@^1.0.0
  a: workspace:^1.0.0
catalogs:
  tools:
    c: workspace:*
  unused:
    a: workspace:*
";

    fn manifests(edit: &[(&str, Option<&str>)]) -> Vec<PnpmManifest> {
        let mut all: Vec<PnpmManifest> = MANIFESTS
            .iter()
            .map(|(importer, text)| PnpmManifest {
                importer: (*importer).to_owned(),
                text: Ok(Some((*text).to_owned())),
            })
            .collect();
        for (importer, text) in edit {
            let text = Ok(text.map(str::to_owned));
            match all
                .iter_mut()
                .find(|manifest| manifest.importer == *importer)
            {
                Some(manifest) => manifest.text = text,
                None => all.push(PnpmManifest {
                    importer: (*importer).to_owned(),
                    text,
                }),
            }
        }
        all
    }

    fn drift(lock: &str, edit: &[(&str, Option<&str>)], catalog: &str) -> PnpmLockDrift {
        pnpm_lock_drift(lock, &manifests(edit), Some(catalog)).expect("a measured lockfile")
    }

    const NONE: [&str; 0] = [];

    fn lines(drift: &PnpmLockDrift) -> Vec<String> {
        drift.drifted.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_lock_pnpm_wrote_has_no_drift() {
        assert_eq!(drift(LOCK, &[], CATALOG), PnpmLockDrift::default());
    }

    #[test]
    fn an_edited_range_is_named_with_both_values() {
        let b = MANIFESTS[2]
            .1
            .replace("workspace:a@^1.0.0", "workspace:a@^1.0.1");
        assert_eq!(
            lines(&drift(LOCK, &[("packages/b", Some(&b))], CATALOG)),
            ["`importers/packages/b/dependencies/aa` records `workspace:a@^1.0.0`, where `packages/b/package.json` declares `workspace:a@^1.0.1`"]
        );
    }

    /// Each refused a frozen install under pnpm 12.16.1.
    #[test]
    fn an_added_removed_or_moved_dependency_is_drift() {
        let added = MANIFESTS[3].1.replace(
            r#""peerDependencies""#,
            r#""dependencies":{"x":"workspace:*"},"peerDependencies""#,
        );
        assert_eq!(
            lines(&drift(LOCK, &[("packages/c", Some(&added))], CATALOG)),
            ["`packages/c/package.json` declares `workspace:*`, which `importers/packages/c/dependencies/x` does not record"]
        );
        let moved = MANIFESTS[0].1.replace("devDependencies", "dependencies");
        assert_eq!(
            lines(&drift(LOCK, &[(".", Some(&moved))], CATALOG)),
            [
                "`package.json` declares `workspace:*`, which `importers/./dependencies/a` does not record",
                "`importers/./devDependencies/a` records `workspace:*`, which `package.json` does not declare",
            ]
        );
    }

    /// An auto-installed peer is recorded under `dependencies`; one an ordinary
    /// section shadows is not, and without `autoInstallPeers` none is.
    #[test]
    fn a_peer_counts_only_where_pnpm_installs_it() {
        let shadowed = MANIFESTS[2].1.replace(r#""a":"^1.0.0""#, r#""a":"^2.0.0""#);
        assert_eq!(
            lines(&drift(LOCK, &[("packages/b", Some(&shadowed))], CATALOG)),
            NONE
        );
        let installed = MANIFESTS[3]
            .1
            .replace(r#""d":"workspace:*""#, r#""d":"workspace:^""#);
        assert_eq!(
            lines(&drift(LOCK, &[("packages/c", Some(&installed))], CATALOG)),
            ["`importers/packages/c/dependencies/d` records `workspace:*`, where `packages/c/package.json` declares `workspace:^`"]
        );
        let without = LOCK
            .replace("autoInstallPeers: true", "autoInstallPeers: false")
            .replace(
                "  packages/c:
    dependencies:
      a:
        specifier: workspace:^1.0.0
        version: link:../a
      d:
        specifier: workspace:*
        version: link:../d
",
                "  packages/c: {}
",
            );
        assert_eq!(
            lines(&drift(
                &without,
                &[("packages/c", Some(&installed))],
                CATALOG
            )),
            NONE
        );
    }

    /// `optionalDependencies` over `dependencies` over `devDependencies`: a
    /// shadowed declaration is not recorded, so editing it is not drift.
    #[test]
    fn one_key_in_two_sections_is_recorded_once() {
        let dup = r#"{"name":"c","version":"2.0.0","dependencies":{"d":"workspace:^1.0.0","a":"workspace:*"},"devDependencies":{"d":"workspace:~1.0.0","a":"workspace:*"},"optionalDependencies":{"a":"workspace:^"}}"#;
        let lock = LOCK.replace(
            "  packages/c:
    dependencies:
      a:
        specifier: workspace:^1.0.0
        version: link:../a
      d:
        specifier: workspace:*
        version: link:../d
",
            "  packages/c:
    dependencies:
      d:
        specifier: workspace:^1.0.0
        version: link:../d
    optionalDependencies:
      a:
        specifier: workspace:^
        version: link:../a
",
        );
        assert_eq!(
            drift(&lock, &[("packages/c", Some(dup))], CATALOG),
            PnpmLockDrift::default()
        );
    }

    #[test]
    fn a_catalog_entry_the_lock_records_must_match_the_workspace_file() {
        let changed = CATALOG.replace("  a: workspace:^1.0.0", "  a: workspace:^1.0.1");
        assert_eq!(
            lines(&drift(LOCK, &[], &changed)),
            ["`catalogs/default/a` records `workspace:^1.0.0`, where `pnpm-workspace.yaml` declares `workspace:^1.0.1`"]
        );
        let removed = CATALOG.replace("  tools:\n    c: workspace:*\n", "");
        assert_eq!(
            lines(&drift(LOCK, &[], &removed)),
            ["`catalogs/tools/c` records `workspace:*`, which `pnpm-workspace.yaml` does not declare"]
        );
        // A frozen install accepts an edit to a catalog no importer uses.
        let unused = CATALOG.replace("    a: workspace:*", "    a: workspace:^1");
        assert_eq!(lines(&drift(LOCK, &[], &unused)), NONE);
        let nested = CATALOG
            .replace("catalog:\n", "catalogs:\n  default:\n")
            .replace(
                "  al: npm:d@^1.0.0\n  a: workspace:^1.0.0\ncatalogs:\n",
                "    al: npm:d@^1.0.0\n    a: workspace:^1.0.0\n",
            );
        assert_eq!(lines(&drift(LOCK, &[], &nested)), NONE, "{nested}");
    }

    /// Only the projects passed are compared: a frozen install accepts an
    /// importer left behind by a removed member, edited or not. A new member
    /// with dependencies and no importer is refused; one with none is not.
    #[test]
    fn importers_follow_the_members_pnpm_requires() {
        let c = MANIFESTS[3]
            .1
            .replace(r#""d":"workspace:*""#, r#""d":"workspace:^""#);
        let without_c: Vec<PnpmManifest> = manifests(&[("packages/c", Some(&c))])
            .into_iter()
            .filter(|manifest| manifest.importer != "packages/c")
            .collect();
        assert_eq!(
            pnpm_lock_drift(LOCK, &without_c, Some(CATALOG)).expect("measured"),
            PnpmLockDrift::default()
        );
        assert_eq!(lines(&drift(LOCK, &[("packages/d", None)], CATALOG)), NONE);
        assert_eq!(
            lines(&drift(
                LOCK,
                &[("packages/e", Some(r#"{"name":"e"}"#))],
                CATALOG
            )),
            NONE
        );
        assert_eq!(
            lines(&drift(
                LOCK,
                &[(
                    "packages/e",
                    Some(r#"{"name":"e","dependencies":{"a":"workspace:*"}}"#)
                )],
                CATALOG
            )),
            ["`packages/e/package.json` declares `workspace:*`, which `importers/packages/e/dependencies/a` does not record"]
        );
    }

    #[test]
    fn an_unreadable_manifest_or_entry_is_unread_not_clean() {
        let found = drift(
            &LOCK.replace("        specifier: workspace:~1.0.0\n", ""),
            &[("packages/a", Some(r#"{"dependencies":{"x":1}}"#))],
            CATALOG,
        );
        assert_eq!(
            found.unread,
            [
                "`packages/a/package.json` does not parse as a manifest pnpm reads (invalid type: integer `1`, expected a string at line 1 column 22)",
                "`importers/packages/b/devDependencies/d` has no specifier string",
            ]
        );
    }

    /// A file in a shape the comparison was not measured under compares
    /// nothing, so it cannot read as clean.
    #[test]
    fn an_unmeasured_file_is_refused_whole() {
        let refused = |lock: &str| {
            pnpm_lock_drift(lock, &manifests(&[]), Some(CATALOG)).expect_err("unmeasured")
        };
        assert_eq!(
            refused(&LOCK.replace("'9.0'", "'9.1'")),
            "lockfileVersion 9.1 is not a format oakum has measured"
        );
        assert!(
            refused(&LOCK[..LOCK.find("importers:").expect("importers")]).contains("no importers")
        );
        assert!(refused(&LOCK.replace(
            "\ncatalogs:",
            "\npnpmfileChecksum: sha256-PVj+MyFqYFzSD0ZbemT5/CG520ew469gbogcyN23Pwk=\n\ncatalogs:"
        ))
        .contains("pnpmfile"));
    }

    /// Settings are out of scope: a missing block reads at pnpm's defaults.
    #[test]
    fn settings_are_not_a_refusal() {
        let bare = LOCK.replace(
            "settings:\n  autoInstallPeers: true\n  excludeLinksFromLockfile: false\n",
            "",
        );
        assert_eq!(drift(&bare, &[], CATALOG), PnpmLockDrift::default());
    }

    /// Under `excludeLinksFromLockfile: true`, pnpm 12.16.1 records a
    /// `workspace:` entry, leaves out a `link:` one and installs, and leaves out
    /// a `file:` or `catalog:` one and refuses its own lockfile.
    #[test]
    fn excluded_links_compare_where_pnpm_installs() {
        let lock = LOCK.replace(
            "excludeLinksFromLockfile: false",
            "excludeLinksFromLockfile: true",
        );
        let only =
            |deps: &str| format!(r#"{{"name":"e","version":"1.0.0","dependencies":{{{deps}}}}}"#);
        let e = |deps: &str| drift(&lock, &[("packages/e", Some(&only(deps)))], CATALOG);
        assert_eq!(e(r#""a":"link:../a""#), PnpmLockDrift::default());
        assert_eq!(
            lines(&e(r#""a":"workspace:^""#)),
            ["`packages/e/package.json` declares `workspace:^`, which `importers/packages/e/dependencies/a` does not record"]
        );
        for spec in ["file:../a", "catalog:"] {
            let found = e(&format!(r#""a":"{spec}""#));
            assert_eq!(lines(&found), NONE, "{spec}");
            assert_eq!(
                found.unread,
                ["`importers/packages/e/dependencies/a` is left out under excludeLinksFromLockfile, where pnpm refuses its own lockfile"],
                "{spec}"
            );
        }
    }

    /// pnpm 12.16.1 records `workspace:*` for `b`'s `workspace:^1.0.0` under
    /// the override `a: workspace:*` or `b>a: workspace:*`, and installs it.
    #[test]
    fn an_overridden_dependency_is_skipped_and_counted() {
        let lock = LOCK.replace(
            "        specifier: workspace:a@^1.0.0\n",
            "        specifier: workspace:*\n",
        );
        assert_eq!(lines(&drift(&lock, &[], CATALOG)).len(), 1);
        for key in ["aa", "'b>aa@^1'"] {
            let applied = lock.replace(
                "\nimporters:",
                &format!("\noverrides:\n  {key}: workspace:*\n\nimporters:"),
            );
            let found = drift(&applied, &[], CATALOG);
            assert_eq!(found.drifted, [], "{key}");
            assert_eq!(found.skipped, 1, "{key}");
        }
    }

    /// Only the overrides the lockfile records were applied: pnpm 12.16.1
    /// ignores `pnpm.overrides` in the root manifest, and one only in
    /// `pnpm-workspace.yaml` has not been installed.
    #[test]
    fn an_override_the_lockfile_does_not_record_skips_nothing() {
        let lock = LOCK.replace(
            "        specifier: workspace:a@^1.0.0\n",
            "        specifier: workspace:*\n",
        );
        let root = MANIFESTS[0].1.replace(
            r#""private":true"#,
            r#""private":true,"pnpm":{"overrides":{"aa":"workspace:*"}}"#,
        );
        let workspace = format!("{CATALOG}overrides:\n  aa: workspace:*\n");
        let found = drift(&lock, &[(".", Some(&root))], &workspace);
        assert_eq!(found.drifted.len(), 1);
        assert_eq!(found.skipped, 0);
    }

    /// pnpm's selector delimiter is a `>` not preceded by a space, `|` or `@`,
    /// so a range's `>=` stays in the range.
    #[test]
    fn an_override_selector_targets_its_child_package() {
        assert_eq!(override_target("a"), "a");
        assert_eq!(override_target("a@^1"), "a");
        assert_eq!(override_target("@s/a"), "@s/a");
        assert_eq!(override_target("@s/a@1"), "@s/a");
        assert_eq!(override_target("p@1>@s/b@^2"), "@s/b");
        assert_eq!(override_target("p>b"), "b");
        assert_eq!(override_target("@probe/a@>=1.0.0"), "@probe/a");
        assert_eq!(override_target("b>@probe/a"), "@probe/a");
        assert_eq!(override_target("@probe/b@1>@probe/a@>=1"), "@probe/a");
    }

    fn probe(b: &str) -> Vec<PnpmManifest> {
        [
            (".", r#"{"name":"root","private":true}"#.to_owned()),
            (
                "packages/a",
                r#"{"name":"@probe/a","version":"1.0.0"}"#.to_owned(),
            ),
            (
                "packages/c",
                r#"{"name":"@probe/c","version":"1.0.0"}"#.to_owned(),
            ),
            (
                "packages/b",
                format!(r#"{{"name":"@probe/b","version":"1.0.0"{b}}}"#),
            ),
        ]
        .into_iter()
        .map(|(importer, text)| PnpmManifest {
            importer: importer.to_owned(),
            text: Ok(Some(text)),
        })
        .collect()
    }

    /// pnpm 12.16.1 with `ignoredOptionalDependencies: ['@probe/c']` in
    /// `pnpm-workspace.yaml`; a frozen install accepts it.
    const IGNORED_OPTIONAL_LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .: {}

  packages/a: {}

  packages/b:
    dependencies:
      '@probe/a':
        specifier: workspace:^
        version: link:../a

  packages/c: {}

ignoredOptionalDependencies:
  - '@probe/c'
";

    #[test]
    fn an_ignored_optional_dependency_is_skipped_and_counted() {
        let b = r#","dependencies":{"@probe/a":"workspace:^"},"optionalDependencies":{"@probe/c":"workspace:^"}"#;
        let found = pnpm_lock_drift(IGNORED_OPTIONAL_LOCK, &probe(b), None).expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
    }

    /// pnpm 12.16.1 with `packageExtensions` adding `@probe/c` to `@probe/b`;
    /// a frozen install accepts it.
    const EXTENDED_LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

packageExtensionsChecksum: sha256-EAof3GAL14nskg1efmFLRG2YY5eBr9hAHuYs+VwMYzo=

importers:

  .: {}

  packages/a: {}

  packages/b:
    dependencies:
      '@probe/a':
        specifier: workspace:^
        version: link:../a
      '@probe/c':
        specifier: workspace:^
        version: link:../c

  packages/c: {}
";

    /// pnpm 12.16.1 output for `@probe/b` depending on `@probe/a` under each
    /// override, verbatim: `'@probe/c>@probe/a'` and `'@probe/a@>=5'` were
    /// listed but not applied to `b`; `link:./packages/a` was.
    fn override_lock(key: &str, value: &str, specifier: &str) -> String {
        format!(
            "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

overrides:
  {key}: {value}

importers:

  .: {{}}

  packages/a: {{}}

  packages/b:
    dependencies:
      '@probe/a':
        specifier: {specifier}
        version: link:../a

  packages/c: {{}}
"
        )
    }

    /// An override pnpm listed but did not apply to this importer leaves the
    /// specifier as declared, so an edit since is drift, not a skip.
    #[test]
    fn an_override_pnpm_did_not_apply_skips_nothing() {
        for key in ["'@probe/c>@probe/a'", "'@probe/a@>=5'"] {
            let lock = override_lock(key, "workspace:*", "workspace:^");
            let clean = pnpm_lock_drift(
                &lock,
                &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
                None,
            )
            .expect("measured");
            assert_eq!(clean, PnpmLockDrift::default(), "{key}");
            let edited = pnpm_lock_drift(
                &lock,
                &probe(r#","dependencies":{"@probe/a":"workspace:~"}"#),
                None,
            )
            .expect("measured");
            assert_eq!(edited.drifted.len(), 1, "{key}");
            assert_eq!(edited.skipped, 0, "{key}");
        }
    }

    /// A `link:` override is written relative to the workspace and recorded
    /// relative to the importer.
    #[test]
    fn an_applied_link_override_is_skipped() {
        let lock = override_lock("'@probe/a'", "link:./packages/a", "link:../a");
        let found = pnpm_lock_drift(
            &lock,
            &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
    }

    /// pnpm 12.16.1 records `file:./packages/a` as `file:../a` in `b`, and a
    /// frozen install accepts it.
    #[test]
    fn an_applied_file_override_is_skipped() {
        let lock = override_lock("'@probe/a'", "file:./packages/a", "file:../a");
        let found = pnpm_lock_drift(
            &lock,
            &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
    }

    /// pnpm 12.16.1 output for `'@probe/b>@probe/a': '-'`, which removes the
    /// dependency; a frozen install accepts it.
    const REMOVED_LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

overrides:
  '@probe/b>@probe/a': '-'

importers:

  .: {}

  packages/a: {}

  packages/b: {}

  packages/c: {}
";

    #[test]
    fn a_dependency_an_override_removes_is_skipped() {
        let found = pnpm_lock_drift(
            REMOVED_LOCK,
            &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
    }

    /// pnpm 12.16.1 writes [`REMOVED_LOCK`] with this key in place for a `b`
    /// without the dependency; adding it without an install fails a frozen
    /// install, since the override removes it only under `@probe/c`.
    #[test]
    fn a_removal_under_another_parent_covers_nothing() {
        let lock = REMOVED_LOCK.replace("'@probe/b>@probe/a'", "'@probe/c>@probe/a'");
        let found = pnpm_lock_drift(
            &lock,
            &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted.len(), 1);
        assert_eq!(found.skipped, 0);
        let unparented = REMOVED_LOCK.replace("'@probe/b>@probe/a'", "'@probe/a'");
        let found = pnpm_lock_drift(
            &unparented,
            &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
    }

    /// A parent range limits a removal to the parent versions it admits: each
    /// case is whether pnpm 12.16.1 applied the override to `b` at that
    /// version.
    #[test]
    fn a_removal_under_a_parent_range_covers_only_a_version_it_admits() {
        const B: &str = "@probe/b";
        // The parent's name, and its `version` as JSON.
        for (key, name, version, applied) in [
            ("'@probe/b@2>@probe/a'", B, Some(r#""2.0.0""#), true),
            ("'@probe/b@^2>@probe/a'", B, Some(r#""2.3.0""#), true),
            (
                "'@probe/b@>=2.0.0-rc.0>@probe/a'",
                B,
                Some(r#""2.0.0-rc.1""#),
                true,
            ),
            ("'@probe/b@2>@probe/a'", B, Some(r#""v2.0.0""#), true),
            ("'@probe/b>@probe/a'", B, None, true),
            ("'@probe/b>@probe/a'", B, Some("2"), true),
            ("'probe-b@2>@probe/a'", "probe-b", Some(r#""2.0.0""#), true),
            ("'@probe/b@2>@probe/a'", B, Some(r#""1.0.0""#), false),
            ("'@probe/b@2>@probe/a'", B, Some(r#""2.0.0-rc.1""#), false),
            ("'@probe/b@2>@probe/a'", B, None, false),
            ("'@probe/b@2>@probe/a'", B, Some("2"), false),
            ("'@probe/b@*>@probe/a'", B, None, false),
            ("'@probe/b@>@probe/a'", B, Some(r#""2.0.0""#), false),
            ("'@probe/b@nonsense>@probe/a'", B, Some(r#""2.0.0""#), false),
            ("'probe-b@2>@probe/a'", "probe-b", Some(r#""1.0.0""#), false),
            ("'@probe/c@1>@probe/a'", B, Some(r#""1.0.0""#), false),
            (
                "'@probe/b@2>@probe/a'",
                B,
                Some(r#""1.0.0","version":"2.0.0""#),
                true,
            ),
            (
                "'@probe/b@*>@probe/a'",
                B,
                Some(r#""9007199254740992.0.0""#),
                false,
            ),
            // Values a full JSON parse refuses and `Declared` skips.
            (
                "'@probe/b@2>@probe/a'",
                B,
                Some(r#""2.0.0","x":1e400,"y":"\ud800""#),
                true,
            ),
            ("'@probe/b>@probe/a'", B, Some("1e400"), true),
            ("'@probe/b@2>@probe/a'", B, Some(r#""\ud800""#), false),
            (
                "'@probe/b@2>@probe/a'",
                B,
                Some(r#"1e400,"version":"2.0.0""#),
                true,
            ),
        ] {
            let lock = REMOVED_LOCK.replace("'@probe/b>@probe/a'", key);
            let version =
                version.map_or(String::new(), |version| format!(r#","version":{version}"#));
            let manifests: Vec<PnpmManifest> = [
                (".", String::from(r#"{"name":"root","private":true}"#)),
                ("packages/a", String::from(r#"{"name":"@probe/a","version":"1.0.0"}"#)),
                ("packages/c", String::from(r#"{"name":"@probe/c","version":"1.0.0"}"#)),
                (
                    "packages/b",
                    format!(
                        r#"{{"name":"{name}"{version},"dependencies":{{"@probe/a":"workspace:^"}}}}"#
                    ),
                ),
            ]
            .into_iter()
            .map(|(importer, text)| PnpmManifest {
                importer: importer.to_owned(),
                text: Ok(Some(text)),
            })
            .collect();
            let found = pnpm_lock_drift(&lock, &manifests, None).expect("measured");
            let case = format!("{key} at {version:?}");
            assert_eq!(found.drifted.is_empty(), applied, "{case}: {found:?}");
            assert_eq!(found.skipped, usize::from(applied), "{case}");
        }
    }

    /// Paths are relative to the workspace root in the override and to the
    /// importer in the record, which may sit outside the lockfile's directory.
    #[test]
    fn a_path_override_is_read_from_the_importer() {
        assert_eq!(
            super::applied("link:./packages/a", "", "packages/b").as_deref(),
            Some("link:../a")
        );
        assert_eq!(
            super::applied("file:./packages/a", "", ".").as_deref(),
            Some("file:packages/a")
        );
        assert_eq!(
            super::applied("link:./packages/a", "js", "../tools/e").as_deref(),
            Some("link:../../js/packages/a")
        );
        assert_eq!(
            super::applied("workspace:*", "js", "../tools/e").as_deref(),
            Some("workspace:*")
        );
    }

    /// pnpm 12.16.1 with `ignoredOptionalDependencies: ['@probe/*']`.
    #[test]
    fn an_ignored_optional_pattern_matches_like_pnpm() {
        let lock = IGNORED_OPTIONAL_LOCK
            .replace(
                "  packages/b:\n    dependencies:\n      '@probe/a':\n        specifier: workspace:^\n        version: link:../a\n",
                "  packages/b: {}\n",
            )
            .replace("  - '@probe/c'", "  - '@probe/*'");
        let found = pnpm_lock_drift(
            &lock,
            &probe(r#","optionalDependencies":{"@probe/c":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
    }

    /// `*` spans `/`; a `!` pattern excludes, a later pattern re-includes, and
    /// a list opening with `!` starts from everything. Each case is what pnpm
    /// 12.16.1 left out of the lockfile.
    #[test]
    fn ignored_patterns_follow_pnpm_order() {
        let ignores = |patterns: &[&str], name: &str| {
            let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
            super::ignored(&patterns, name)
        };
        assert!(ignores(&["*"], "@probe/c"));
        assert!(ignores(&["*/c"], "@probe/c"));
        assert!(ignores(&["@probe/c*"], "@probe/c"));
        assert!(!ignores(&["@probe/c*"], "@probe/d"));
        assert!(!ignores(&["@probe/*", "!@probe/d"], "@probe/d"));
        assert!(ignores(&["@probe/*", "!@probe/d"], "@probe/c"));
        assert!(ignores(&["!@probe/d"], "@probe/c"));
        assert!(!ignores(&["!@probe/d"], "@probe/d"));
        assert!(ignores(&["!@probe/d", "@probe/*"], "@probe/d"));
        assert!(ignores(&["@probe/*", "!@probe/*", "@probe/c"], "@probe/c"));
        assert!(!ignores(&["@probe/*", "!@probe/*", "@probe/c"], "@probe/d"));
    }

    /// An extension adds an entry the manifest lacks; what the manifest
    /// declares is still compared.
    #[test]
    fn a_package_extension_entry_is_skipped_but_declarations_compare() {
        let found = pnpm_lock_drift(
            EXTENDED_LOCK,
            &probe(r#","dependencies":{"@probe/a":"workspace:^"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(found.drifted, []);
        assert_eq!(found.skipped, 1);
        let edited = pnpm_lock_drift(
            EXTENDED_LOCK,
            &probe(r#","dependencies":{"@probe/a":"workspace:*"}"#),
            None,
        )
        .expect("measured");
        assert_eq!(
            lines(&edited),
            ["`importers/packages/b/dependencies/@probe/a` records `workspace:^`, where `packages/b/package.json` declares `workspace:*`"]
        );
    }
}
