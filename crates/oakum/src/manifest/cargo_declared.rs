//! Cargo members' versions and external dependencies read from manifest text,
//! as `cargo metadata --no-deps` reports them, for a side of a comparison that
//! no cargo runs on (ADR-0039). Sources are cargo's ids from before `[patch]`.

use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use toml::{Table, Value};
use url::Url;

use crate::plan::{DependencyKind, ExternalDependency, Package, PackageId, Workspace};

const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// A member as one side's manifests declare it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declarations {
    pub version: Version,
    pub external_dependencies: Vec<ExternalDependency>,
    /// Declarations from a named registry whose index no config on that side
    /// names; their `source` is `None` and unread.
    pub source_unknown: Vec<ExternalDependency>,
}

/// Cargo's config on the side being read: each file's text, nearest the
/// workspace first, and the `CARGO_REGISTRIES_<NAME>_INDEX` variables, which
/// outrank every file.
#[derive(Clone, Debug, Default)]
pub struct CargoConfig {
    files: Vec<Table>,
    env: BTreeMap<String, String>,
}

impl CargoConfig {
    /// A text that is not TOML names no registry.
    #[must_use]
    pub fn new<'t>(
        files: impl IntoIterator<Item = &'t str>,
        env: BTreeMap<String, String>,
    ) -> Self {
        Self {
            files: files
                .into_iter()
                .filter_map(|text| toml::from_str(text).ok())
                .collect(),
            env,
        }
    }

    /// The variable cargo reads `name`'s index from.
    #[must_use]
    pub fn index_variable(name: &str) -> String {
        format!(
            "CARGO_REGISTRIES_{}_INDEX",
            name.to_uppercase().replace('-', "_")
        )
    }

    fn registry_source(&self, name: &str) -> Option<String> {
        let index = self
            .env
            .get(&Self::index_variable(name))
            .cloned()
            .or_else(|| {
                self.files.iter().find_map(|file| {
                    file.get("registries")?
                        .get(name)?
                        .get("index")?
                        .as_str()
                        .map(str::to_owned)
                })
            })?;
        Some(index_source(&index))
    }
}

/// A manifest on the side being read that is not TOML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotToml {
    /// Repository-relative.
    pub path: String,
    pub message: String,
}

/// Each Cargo member of `workspace` as one side declares it: `root` is the
/// workspace root's `Cargo.toml` there and `manifests` each member's, `None`
/// where absent. A member that was no member of this workspace there (its own
/// `[workspace]`, excluded, or matched by neither `members` nor a member's path
/// dependency, as cargo finds them) is left out, as one added since is; so is
/// one whose manifest cargo would refuse there, such as inheritance the root
/// cannot answer. A path entry naming a member's directory is a workspace graph
/// edge, which discovery keeps out of the external dependencies too.
///
/// # Errors
///
/// A manifest that is not TOML.
pub fn declared_members(
    workspace: &Workspace,
    root: Option<&str>,
    manifests: &[(&Package, Option<&str>)],
    config: &CargoConfig,
) -> Result<BTreeMap<PackageId, Declarations>, NotToml> {
    let dir = workspace.cargo_workspace_root().unwrap_or_default();
    let parse = |path: String, text: &str| {
        toml::from_str::<Table>(text).map_err(|err| NotToml {
            path,
            message: err.to_string(),
        })
    };
    let table = root
        .map(|text| parse(joined(dir, "Cargo.toml"), text))
        .transpose()?
        .unwrap_or_default();
    let mut parsed = Vec::new();
    let mut garbled = Vec::new();
    for (package, text) in manifests {
        if let Some(text) = text {
            match parse(package.manifest_path(), text) {
                Ok(table) => parsed.push((*package, table)),
                Err(err) => garbled.push((*package, err)),
            }
        }
    }
    let root = Root { table: &table, dir };
    let (members, unread) = root.members(&parsed);
    // cargo reads only the members its globs list and their path dependencies,
    // so a garbled manifest outside them is no error there.
    if let Some((_, err)) = garbled.into_iter().find(|(package, _)| {
        let dir = package.manifest_dir();
        members.contains(dir) || unread.contains(dir) || root.listed(dir)
    }) {
        return Err(err);
    }
    // A path is a graph edge only to a directory that held the same member at
    // the base; one renamed since is a crate outside the members there.
    let member_dirs: Vec<&str> = parsed
        .iter()
        .filter(|(package, manifest)| {
            members.contains(package.manifest_dir())
                && package_table(manifest)
                    .and_then(|table| table.get("name"))
                    .and_then(Value::as_str)
                    == Some(package.id().name.as_str())
        })
        .map(|(package, _)| package.manifest_dir())
        .collect();
    Ok(parsed
        .iter()
        .filter(|(package, _)| members.contains(package.manifest_dir()))
        .filter_map(|(package, manifest)| {
            let found = declared(&root, &member_dirs, package, manifest, config)?;
            Some((package.id().clone(), found))
        })
        .collect())
}

struct Root<'a> {
    table: &'a Table,
    dir: &'a str,
}

impl Root<'_> {
    fn workspace(&self, key: &str) -> Option<&Value> {
        self.table.get("workspace")?.as_table()?.get(key)
    }

    fn entries(&self, key: &str) -> Vec<String> {
        self.workspace(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|entry| joined("", entry))
            .collect()
    }

    /// Cargo matches `exclude` as a path prefix, overridden by a `members`
    /// entry that is one read literally.
    fn excluded(&self, under_root: &str) -> bool {
        let under = |entries: Vec<String>| entries.iter().any(|entry| within(under_root, entry));
        under(self.entries("exclude")) && !under(self.entries("members"))
    }

    /// The directories of the packages in `parsed` this root's workspace holds:
    /// the root package, those a `members` glob matches, and the path
    /// dependencies of either under the root, transitively, short of `exclude`.
    fn members(&self, parsed: &[(&Package, Table)]) -> (BTreeSet<String>, BTreeSet<String>) {
        let tables: BTreeMap<&str, &Table> = parsed
            .iter()
            .map(|(package, table)| (package.manifest_dir(), table))
            .collect();
        let root_package = package_table(self.table).is_some();
        let mut queue: Vec<&str> = Vec::new();
        if root_package {
            queue.push(self.dir);
        }
        for &dir in tables.keys() {
            if self.listed(dir) {
                queue.push(dir);
            }
        }
        let mut members = BTreeSet::new();
        let mut unread = BTreeSet::new();
        while let Some(dir) = queue.pop() {
            if !members.insert(dir.to_owned()) {
                continue;
            }
            let table = if dir == self.dir && root_package {
                Some(self.table)
            } else {
                tables.get(dir).copied()
            };
            for path in table
                .map(|table| self.path_dependencies(table, dir))
                .unwrap_or_default()
            {
                if under(self.dir, &path).is_none_or(|rel| self.excluded(rel)) {
                    continue;
                }
                match tables.get_key_value(path.as_str()) {
                    Some((&found, _)) => queue.push(found),
                    None => {
                        unread.insert(path);
                    }
                }
            }
        }
        (members, unread)
    }

    /// Whether a `members` glob names `dir`, short of `exclude`.
    fn listed(&self, dir: &str) -> bool {
        self.table.get("workspace").is_some_and(Value::is_table)
            && under(self.dir, dir).is_some_and(|rel| {
                !self.excluded(rel)
                    && self
                        .entries("members")
                        .iter()
                        .any(|glob| glob_matches(glob, rel))
            })
    }

    /// Every directory `manifest`, in `dir`, names by `path`, inherited entries
    /// included; repository-relative.
    fn path_dependencies(&self, manifest: &Table, dir: &str) -> Vec<String> {
        let inherited = self.workspace("dependencies").and_then(Value::as_table);
        sections(manifest)
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .filter_map(|(key, entry)| {
                let (entry, from) = if workspace_true(entry) {
                    (inherited?.get(key)?, self.dir)
                } else {
                    (entry, dir)
                };
                Some(joined(from, entry.get("path")?.as_str()?))
            })
            .collect()
    }
}

/// `dir` relative to `root`, when it lies under it.
fn under<'d>(root: &str, dir: &'d str) -> Option<&'d str> {
    if root.is_empty() {
        Some(dir)
    } else if dir == root {
        Some("")
    } else {
        dir.strip_prefix(root)?.strip_prefix('/')
    }
}

fn within(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `[package]`, or `[project]`, which cargo reads as one before edition 2024.
fn package_table(manifest: &Table) -> Option<&Table> {
    manifest
        .get("package")
        .or_else(|| manifest.get("project"))
        .and_then(Value::as_table)
}

/// `member` as `manifest` declares it; `None` when it is no package of the
/// member's name, a workspace of its own, or an entry cargo would refuse.
fn declared(
    root: &Root<'_>,
    member_dirs: &[&str],
    member: &Package,
    manifest: &Table,
    config: &CargoConfig,
) -> Option<Declarations> {
    let package = package_table(manifest)?;
    if package.get("name").and_then(Value::as_str) != Some(member.id().name.as_str()) {
        return None;
    }
    let own_root = manifest.contains_key("workspace") && member.manifest_dir() != root.dir;
    let elsewhere = package.get("workspace").is_some_and(|path| {
        path.as_str()
            .map(|path| joined(member.manifest_dir(), path))
            != Some(root.dir.to_owned())
    });
    if own_root || elsewhere {
        return None;
    }
    let version = match package.get("version") {
        None => Version::new(0, 0, 0),
        Some(value) if workspace_true(value) => root
            .workspace("package")?
            .get("version")?
            .as_str()?
            .parse()
            .ok()?,
        Some(value) => value.as_str()?.parse().ok()?,
    };
    let mut declarations = Declarations {
        version,
        external_dependencies: Vec::new(),
        source_unknown: Vec::new(),
    };
    let inherited = root.workspace("dependencies").and_then(Value::as_table);
    for (kind, entries) in sections(manifest) {
        for (key, entry) in entries {
            let (entry, dir, is_inherited) = if workspace_true(entry) {
                (inherited?.get(key)?, root.dir, true)
            } else {
                (entry, member.manifest_dir(), false)
            };
            let (dependency, source) = dependency(key, kind, entry, is_inherited, config)?;
            match source {
                Source::Path(path) if member_dirs.contains(&joined(dir, path).as_str()) => {}
                Source::Unknown => declarations.source_unknown.push(dependency),
                Source::Path(_) | Source::Id => declarations.external_dependencies.push(dependency),
            }
        }
    }
    Some(declarations)
}

fn workspace_true(value: &Value) -> bool {
    value
        .as_table()
        .and_then(|table| table.get("workspace"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Every dependency table, `[target.<cfg>]` ones included. Cargo still reads
/// `dev_dependencies` and `build_dependencies` where the hyphenated table is
/// absent.
fn sections(manifest: &Table) -> Vec<(DependencyKind, &Table)> {
    let targets = manifest
        .get("target")
        .and_then(Value::as_table)
        .into_iter()
        .flat_map(|targets| targets.values().filter_map(Value::as_table));
    std::iter::once(manifest)
        .chain(targets)
        .flat_map(|table| {
            [
                (DependencyKind::Normal, "dependencies"),
                (DependencyKind::Development, "dev-dependencies"),
                (DependencyKind::Build, "build-dependencies"),
            ]
            .into_iter()
            .filter_map(move |(kind, section)| {
                table
                    .get(section)
                    .or_else(|| table.get(&section.replace('-', "_")))
                    .and_then(Value::as_table)
                    .map(|entries| (kind, entries))
            })
        })
        .collect()
}

/// Where a declaration's source came from: a path relative to the manifest
/// that wrote it, a source id on the dependency, or a registry no config names.
enum Source<'e> {
    Path(&'e str),
    Id,
    Unknown,
}

/// The dependency `key` declares; `None` for an entry cargo would refuse.
fn dependency<'e>(
    key: &str,
    kind: DependencyKind,
    entry: &'e Value,
    inherited: bool,
    config: &CargoConfig,
) -> Option<(ExternalDependency, Source<'e>)> {
    let dependency = |name: &str, source: Option<String>, req: Option<&str>| ExternalDependency {
        name: name.to_owned(),
        declared_as: key.to_owned(),
        kind,
        source,
        // Unparsable counts as any version, as discovery reads it.
        req: req.map_or(VersionReq::STAR, |req| {
            VersionReq::parse(req).unwrap_or(VersionReq::STAR)
        }),
        inherited,
    };
    if let Some(req) = entry.as_str() {
        return Some((
            dependency(key, Some(CRATES_IO.to_owned()), Some(req)),
            Source::Id,
        ));
    }
    let table = entry.as_table()?;
    let text = |field: &str| table.get(field).and_then(Value::as_str);
    let name = text("package").unwrap_or(key);
    let (source, from) = if let Some(path) = text("path") {
        (None, Source::Path(path))
    } else if let Some(url) = text("git") {
        let reference = ["branch", "tag", "rev"]
            .into_iter()
            .find_map(|field| text(field).map(|value| format!("?{field}={value}")))
            .unwrap_or_default();
        (
            Some(format!("git+{}{reference}", normalized(url))),
            Source::Id,
        )
    } else if let Some(index) = text("registry-index") {
        (Some(index_source(index)), Source::Id)
    } else if let Some(registry) = text("registry").filter(|name| *name != "crates-io") {
        match config.registry_source(registry) {
            Some(source) => (Some(source), Source::Id),
            None => (None, Source::Unknown),
        }
    } else {
        (Some(CRATES_IO.to_owned()), Source::Id)
    };
    Some((dependency(name, source, text("version")), from))
}

/// Cargo's id for a registry index URL: `sparse+` ones keep their prefix.
fn index_source(index: &str) -> String {
    if index.starts_with("sparse+") {
        normalized(index)
    } else {
        format!("registry+{}", normalized(index))
    }
}

/// The URL as cargo parses and prints it; one that does not parse is kept as
/// written, since cargo refuses it and no lock row holds it.
fn normalized(url: &str) -> String {
    Url::parse(url).map_or_else(|_| url.to_owned(), String::from)
}

/// `path` under the repository-relative `dir`, `.` and `..` resolved.
fn joined(dir: &str, path: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// A `members` glob against a path under the root, one component at a time as
/// cargo expands it: `*`, `?` and `[…]` within a component, `**` across any.
fn glob_matches(glob: &str, path: &str) -> bool {
    let split = |text: &str| -> Vec<Vec<char>> {
        text.split('/')
            .filter(|part| !part.is_empty())
            .map(|part| part.chars().collect())
            .collect()
    };
    components_match(&split(glob), &split(path))
}

fn components_match(glob: &[Vec<char>], path: &[Vec<char>]) -> bool {
    match glob.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first.as_slice() == ['*', '*'] => {
            (0..=path.len()).any(|skip| components_match(rest, &path[skip..]))
        }
        Some((first, rest)) => path.split_first().is_some_and(|(head, tail)| {
            component_matches(first, head) && components_match(rest, tail)
        }),
    }
}

fn component_matches(glob: &[char], name: &[char]) -> bool {
    match glob.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| component_matches(rest, &name[skip..])),
        Some(('?', rest)) => name
            .split_first()
            .is_some_and(|(_, tail)| component_matches(rest, tail)),
        Some(('[', rest)) => match class(rest) {
            Some((accepts, after)) => name
                .split_first()
                .is_some_and(|(c, tail)| accepts(*c) && component_matches(after, tail)),
            None => name
                .split_first()
                .is_some_and(|(head, tail)| *head == '[' && component_matches(rest, tail)),
        },
        Some((c, rest)) => name
            .split_first()
            .is_some_and(|(head, tail)| head == c && component_matches(rest, tail)),
    }
}

/// The `[…]` class opening `rest` (after its `[`), `!` negating, `a-z` ranges,
/// a leading `]` literal, and what follows its closing `]`.
fn class(rest: &[char]) -> Option<(impl Fn(char) -> bool + '_, &[char])> {
    let (negated, body) = match rest.split_first() {
        Some(('!', body)) => (true, body),
        _ => (false, rest),
    };
    let close = body
        .iter()
        .skip(1)
        .position(|c| *c == ']')
        .map(|at| at + 1)?;
    let set = &body[..close];
    let accepts = move |c: char| {
        let mut hit = false;
        let mut at = 0;
        while at < set.len() {
            if at + 2 < set.len() && set[at + 1] == '-' {
                hit |= (set[at]..=set[at + 2]).contains(&c);
                at += 3;
            } else {
                hit |= set[at] == c;
                at += 1;
            }
        }
        hit != negated
    };
    Some((accepts, &body[close + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Ecosystem, ResolvesDependenciesAt};

    fn package(name: &str, dir: &str) -> Package {
        Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            Version::new(9, 9, 9),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )
        .with_manifest_dir(dir)
    }

    fn workspace_of(dirs: &[(&str, &str)]) -> Workspace {
        Workspace::new(dirs.iter().map(|(name, dir)| package(name, dir)))
            .expect("workspace")
            .with_cargo_workspace_root("")
    }

    const ROOT: &str = r#"[workspace]
members = ["crates/*"]

[workspace.package]
version = "1.2.3"

[workspace.dependencies]
core = { path = "crates/core" }
rand09 = { package = "rand", version = "0.9" }
vendored = { path = "vendor/vendored", version = "0.5" }
"#;

    const MEMBERS: &str = "[workspace]\nmembers = [\"crates/*\"]\n";

    /// Each package's manifest as given, the root's `root`; the members found.
    fn declared_in(
        packages: &[(&str, &str, &str)],
        root: &str,
        config: &CargoConfig,
    ) -> Result<BTreeMap<PackageId, Declarations>, NotToml> {
        let dirs: Vec<(&str, &str)> = packages
            .iter()
            .map(|(name, dir, _)| (*name, *dir))
            .collect();
        let workspace = workspace_of(&dirs);
        let manifests: Vec<(&Package, Option<&str>)> = workspace
            .packages()
            .map(|package| {
                let text = packages
                    .iter()
                    .find(|(name, _, _)| *name == package.id().name)
                    .map(|(_, _, text)| *text);
                (package, text)
            })
            .collect();
        declared_members(&workspace, Some(root), &manifests, config)
    }

    fn declared_by(manifest: &str, root: &str) -> Result<Option<Declarations>, NotToml> {
        declared_with(manifest, root, &CargoConfig::default())
    }

    fn declared_with(
        manifest: &str,
        root: &str,
        config: &CargoConfig,
    ) -> Result<Option<Declarations>, NotToml> {
        let mut found = declared_in(
            &[
                ("app", "crates/app", manifest),
                ("core", "crates/core", "[package]\nname = \"core\"\n"),
            ],
            root,
            config,
        )?;
        Ok(found.remove(&PackageId::new(Ecosystem::Cargo, "app")))
    }

    fn dep(
        name: &str,
        declared_as: &str,
        kind: DependencyKind,
        source: Option<&str>,
        req: &str,
        inherited: bool,
    ) -> ExternalDependency {
        ExternalDependency {
            name: name.to_owned(),
            declared_as: declared_as.to_owned(),
            kind,
            source: source.map(str::to_owned),
            req: VersionReq::parse(req).expect("req"),
            inherited,
        }
    }

    #[test]
    fn reads_what_cargo_metadata_reports() {
        let manifest = r#"[package]
name = "app"
version.workspace = true

[dependencies]
core = { path = "../core" }
helper = { path = "../../vendor/helper" }
log = "0.4"
rand = { version = "0.8", features = ["small_rng"] }
plain = { git = "https://example.invalid/plain.git" }
pinned = { git = "https://example.invalid/pinned.git", rev = "abc123", version = "0.1" }
mirrored = { version = "1", registry-index = "https://example.invalid/index" }
sparse = { version = "1", registry-index = "sparse+https://example.invalid/index/" }
private = { version = "1", registry = "corp" }

[dev-dependencies]
rand09.workspace = true

[build_dependencies]
cc = "1"

[target.'cfg(unix)'.dependencies]
nix-alias = { package = "nix", git = "https://example.invalid/nix.git", branch = "next" }

[target.'cfg(windows)'.build-dependencies]
winres = { git = "https://example.invalid/winres.git", tag = "v2" }
vendored = { workspace = true, optional = true }
"#;
        let crates_io = Some(CRATES_IO);
        let declared = declared_by(manifest, ROOT).expect("parses").expect("app");
        assert_eq!(declared.version, Version::new(1, 2, 3));
        let (normal, dev, build) = (
            DependencyKind::Normal,
            DependencyKind::Development,
            DependencyKind::Build,
        );
        assert_eq!(
            declared.external_dependencies,
            [
                dep("helper", "helper", normal, None, "*", false),
                dep("log", "log", normal, crates_io, "0.4", false),
                dep(
                    "mirrored",
                    "mirrored",
                    normal,
                    Some("registry+https://example.invalid/index"),
                    "1",
                    false
                ),
                dep(
                    "pinned",
                    "pinned",
                    normal,
                    Some("git+https://example.invalid/pinned.git?rev=abc123"),
                    "0.1",
                    false
                ),
                dep(
                    "plain",
                    "plain",
                    normal,
                    Some("git+https://example.invalid/plain.git"),
                    "*",
                    false
                ),
                dep("rand", "rand", normal, crates_io, "0.8", false),
                dep(
                    "sparse",
                    "sparse",
                    normal,
                    Some("sparse+https://example.invalid/index/"),
                    "1",
                    false
                ),
                dep("rand", "rand09", dev, crates_io, "0.9", true),
                dep("cc", "cc", build, crates_io, "1", false),
                dep(
                    "nix",
                    "nix-alias",
                    normal,
                    Some("git+https://example.invalid/nix.git?branch=next"),
                    "*",
                    false
                ),
                dep("vendored", "vendored", build, None, "0.5", true),
                dep(
                    "winres",
                    "winres",
                    build,
                    Some("git+https://example.invalid/winres.git?tag=v2"),
                    "*",
                    false
                ),
            ]
        );
        assert_eq!(
            declared.source_unknown,
            [dep("private", "private", normal, None, "1", false)]
        );
    }

    #[test]
    fn a_manifest_that_is_not_the_member_declares_nothing() {
        for manifest in [
            "[workspace]\nmembers = []\n",
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\n",
        ] {
            assert_eq!(declared_by(manifest, MEMBERS), Ok(None), "{manifest}");
        }
        let unversioned = declared_by("[package]\nname = \"app\"\n", MEMBERS)
            .expect("parses")
            .expect("app");
        assert_eq!(unversioned.version, Version::new(0, 0, 0));
    }

    #[test]
    fn what_cargo_would_refuse_declares_nothing() {
        for manifest in [
            "[package]\nname = \"app\"\nversion = \"one\"\n",
            "[package]\nname = \"app\"\nversion.workspace = true\n",
            "[package]\nname = \"app\"\n[dependencies]\nmissing.workspace = true\n",
            "[package]\nname = \"app\"\n[dependencies]\nodd = 1\n",
        ] {
            assert_eq!(declared_by(manifest, MEMBERS), Ok(None), "{manifest}");
        }
        let unversioned_root = format!("{MEMBERS}\n[workspace.package]\nedition = \"2021\"\n");
        assert_eq!(
            declared_by(
                "[package]\nname = \"app\"\nversion.workspace = true\n",
                &unversioned_root
            ),
            Ok(None)
        );
    }

    #[test]
    fn only_text_that_is_not_toml_is_an_error() {
        let err = declared_by("[package\nname = ", MEMBERS).expect_err("garbled");
        assert_eq!(err.path, "crates/app/Cargo.toml");
        let err = declared_in(
            &[("app", "crates/app", "[package]\nname = \"app\"\n")],
            "[workspace",
            &CargoConfig::default(),
        )
        .expect_err("garbled root");
        assert_eq!(err.path, "Cargo.toml");
    }

    #[test]
    fn a_project_table_is_the_package() {
        let declared = declared_by("[project]\nname = \"app\"\nversion = \"0.4.0\"\n", MEMBERS)
            .expect("parses")
            .expect("app");
        assert_eq!(declared.version, Version::new(0, 4, 0));
    }

    /// Each pair is what `cargo metadata --no-deps` (cargo 1.99.1) reported
    /// for the spelling on the left.
    #[test]
    fn a_source_is_spelled_as_cargo_normalizes_it() {
        let manifest = r#"[package]
name = "app"

[dependencies]
a = { git = "file:///tmp/x/sub/../dev" }
b = { git = "https://GitHub.com/Foo/Bar.git/" }
c = { git = "https://example.com:443/a//b/./c/", branch = "main" }
d = { git = "ssh://git@Example.com/x/y.git", rev = "ABC" }
e = { git = "https://example.com/a b%7e", tag = "v 1" }
f = { version = "1", registry-index = "https://Example.com/idx" }
g = { version = "1", registry-index = "sparse+HTTPS://Example.com:443/a/../idx" }
h = { git = "https://example.com/x?q=1", branch = "m" }
i = { git = "https://example.com" }
"#;
        let declared = declared_by(manifest, MEMBERS)
            .expect("parses")
            .expect("app");
        let sources: Vec<(&str, Option<&str>)> = declared
            .external_dependencies
            .iter()
            .map(|dep| (dep.name.as_str(), dep.source.as_deref()))
            .collect();
        assert_eq!(
            sources,
            [
                ("a", Some("git+file:///tmp/x/dev")),
                ("b", Some("git+https://github.com/Foo/Bar.git/")),
                ("c", Some("git+https://example.com/a//b/c/?branch=main")),
                ("d", Some("git+ssh://git@Example.com/x/y.git?rev=ABC")),
                ("e", Some("git+https://example.com/a%20b%7e?tag=v 1")),
                ("f", Some("registry+https://example.com/idx")),
                ("g", Some("sparse+https://Example.com:443/idx")),
                ("h", Some("git+https://example.com/x?q=1?branch=m")),
                ("i", Some("git+https://example.com/")),
            ]
        );
    }

    /// As `cargo metadata --no-deps` (cargo 1.99.1) resolved each name: the
    /// variable over every file, the nearer file over the farther.
    #[test]
    fn a_named_registry_resolves_from_the_config() {
        let manifest = "[package]\nname = \"app\"\n\n[dependencies]\n\
                        corp = { version = \"1\", registry = \"corp\" }\n\
                        near = { version = \"1\", registry = \"my-near\" }\n\
                        far = { version = \"1\", registry = \"far\" }\n\
                        home = { version = \"1\", registry = \"crates-io\" }\n\
                        lost = { version = \"1\", registry = \"lost\" }\n";
        let config = CargoConfig::new(
            [
                "[registries.my-near]\nindex = \"sparse+https://Near.invalid/i/\"\n\
                 [registries.far]\nindex = \"https://near.invalid/far\"\n",
                "not toml [",
                "[registries.far]\nindex = \"https://far.invalid/far\"\n\
                 [registries.corp]\nindex = \"https://file.invalid/corp\"\n",
            ],
            BTreeMap::from([(
                CargoConfig::index_variable("corp"),
                String::from("https://Env.invalid/a/../corp"),
            )]),
        );
        assert_eq!(
            CargoConfig::index_variable("my-near"),
            "CARGO_REGISTRIES_MY_NEAR_INDEX"
        );
        let declared = declared_with(manifest, MEMBERS, &config)
            .expect("parses")
            .expect("app");
        let sources: Vec<(&str, Option<&str>)> = declared
            .external_dependencies
            .iter()
            .map(|dep| (dep.name.as_str(), dep.source.as_deref()))
            .collect();
        assert_eq!(
            sources,
            [
                ("corp", Some("registry+https://env.invalid/corp")),
                ("far", Some("registry+https://near.invalid/far")),
                ("home", Some(CRATES_IO)),
                ("near", Some("sparse+https://Near.invalid/i/")),
            ]
        );
        let unknown: Vec<&str> = declared
            .source_unknown
            .iter()
            .map(|dep| dep.name.as_str())
            .collect();
        assert_eq!(unknown, ["lost"]);
    }

    fn members_of(root: &str, packages: &[(&str, &str, &str)]) -> Vec<String> {
        declared_in(packages, root, &CargoConfig::default())
            .expect("parses")
            .into_keys()
            .map(|id| id.name)
            .collect()
    }

    fn plain(name: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")
    }

    #[test]
    fn a_tool_with_its_own_workspace_is_no_member() {
        let xt = "[package]\nname = \"xt\"\nversion = \"0.3.0\"\n\n[workspace]\n";
        let app = plain("app");
        let root = "[workspace]\nmembers = [\"app\", \"tools/xt\"]\n";
        assert_eq!(
            members_of(root, &[("app", "app", &app), ("xt", "tools/xt", xt)]),
            ["app"]
        );
        let pointed = "[package]\nname = \"xt\"\nworkspace = \"..\"\n";
        assert_eq!(
            members_of(root, &[("app", "app", &app), ("xt", "tools/xt", pointed)]),
            ["app"]
        );
    }

    /// Each case's members are what `cargo metadata --no-deps` (cargo 1.99.1)
    /// listed for that root, with `a` path-depending on `b` and `b` on `c`.
    #[test]
    fn members_are_found_as_cargo_finds_them() {
        let (b, xt, yy, hid) = (plain("b"), plain("xt"), plain("yy"), plain("hid"));
        let a = format!(
            "{}\n[dependencies]\nb = {{ path = \"../../libs/b\" }}\n",
            plain("a")
        );
        let b_on_c = format!("{b}\n[dependencies]\nc = {{ path = \"../c\" }}\n");
        let c = plain("c");
        let packages = [
            ("a", "crates/a", a.as_str()),
            ("b", "libs/b", b_on_c.as_str()),
            ("c", "libs/c", c.as_str()),
            ("hid", "crates/.hid", hid.as_str()),
            ("xt", "tools/xt", xt.as_str()),
            ("yy", "tools/yy", yy.as_str()),
        ];
        for (members, expected) in [
            ("members = [\"crates/*\"]", &["a", "b", "c", "hid"][..]),
            (
                "members = [\"crates/*\", \"tools/*\"]\nexclude = [\"tools\"]",
                &["a", "b", "c", "hid"],
            ),
            (
                "members = [\"crates/*\", \"tools/*\"]\nexclude = [\"tools/x*\"]",
                &["a", "b", "c", "hid", "xt", "yy"],
            ),
            (
                "members = [\"crates/*\", \"tools/xt\"]\nexclude = [\"tools\"]",
                &["a", "b", "c", "hid", "xt"],
            ),
            (
                "members = [\"./crates/a/\", \"tools/xt\"]",
                &["a", "b", "c", "xt"],
            ),
            ("members = [\"tools/??\"]", &["xt", "yy"]),
            ("members = [\"tools/[x]t\"]", &["xt"]),
            ("members = [\"tools/[!x]?\"]", &["yy"]),
            ("members = [\"**/xt\"]", &["xt"]),
            (
                "members = [\"crates/a\"]\nexclude = [\"libs/c\"]",
                &["a", "b"],
            ),
        ] {
            let root = format!("[workspace]\n{members}\n");
            assert_eq!(members_of(&root, &packages), expected, "{members}");
        }
    }

    #[test]
    fn a_root_package_and_its_path_dependencies_are_members() {
        let root = "[package]\nname = \"r\"\nversion = \"0.1.0\"\n\n\
                    [dev-dependencies]\nxt = { path = \"tools/xt\" }\n\n[workspace]\n";
        let xt = plain("xt");
        let yy = plain("yy");
        assert_eq!(
            members_of(
                root,
                &[
                    ("r", "", root),
                    ("xt", "tools/xt", &xt),
                    ("yy", "tools/yy", &yy)
                ]
            ),
            ["r", "xt"]
        );
        let lone = plain("r");
        assert_eq!(
            members_of(&lone, &[("r", "", &lone), ("xt", "tools/xt", &xt)]),
            ["r"]
        );
    }

    /// cargo never read a manifest the base's members did not list, so its text
    /// there does not matter.
    #[test]
    fn a_garbled_manifest_outside_the_base_members_declares_nothing() {
        let declared = declared_in(
            &[
                (
                    "app",
                    "crates/app",
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
                ),
                ("xt", "tools/xt", "[package\nname = "),
            ],
            MEMBERS,
            &CargoConfig::default(),
        )
        .expect("an unlisted manifest is not read");
        assert!(declared.contains_key(&PackageId::new(Ecosystem::Cargo, "app")));
        assert!(!declared.contains_key(&PackageId::new(Ecosystem::Cargo, "xt")));
        let err = declared_in(
            &[("xt", "crates/xt", "[package\nname = ")],
            MEMBERS,
            &CargoConfig::default(),
        )
        .expect_err("a listed manifest");
        assert_eq!(err.path, "crates/xt/Cargo.toml");
        let err = declared_in(
            &[
                (
                    "a",
                    "crates/a",
                    "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n\
                     [dependencies]\nb = { path = \"../../libs/b\" }\n",
                ),
                ("b", "libs/b", "[package\nname = "),
            ],
            MEMBERS,
            &CargoConfig::default(),
        )
        .expect_err("a path member cargo loads");
        assert_eq!(err.path, "libs/b/Cargo.toml");
    }

    /// `crates/leaf` holds today's `newleaf`, but `oldleaf` at the base: a path
    /// to it there is to a crate outside the members, not a graph edge.
    #[test]
    fn a_path_to_a_directory_whose_member_was_renamed_stays_declared() {
        let declared = declared_in(
            &[
                (
                    "helper",
                    "crates/helper",
                    "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n\n\
                     [dependencies]\noldleaf = { path = \"../leaf\" }\n",
                ),
                (
                    "newleaf",
                    "crates/leaf",
                    "[package]\nname = \"oldleaf\"\nversion = \"0.1.0\"\n",
                ),
            ],
            MEMBERS,
            &CargoConfig::default(),
        )
        .expect("parses");
        let helper = &declared[&PackageId::new(Ecosystem::Cargo, "helper")];
        assert!(
            helper
                .external_dependencies
                .iter()
                .any(|dep| dep.name == "oldleaf" && dep.source.is_none()),
            "{:?}",
            helper.external_dependencies
        );
    }

    #[test]
    fn a_path_resolves_against_the_directory_that_wrote_it() {
        assert_eq!(joined("crates/app", "../core"), "crates/core");
        assert_eq!(joined("crates/app", "./src/../x/"), "crates/app/x");
        assert_eq!(joined("", "../outside"), "../outside");
        assert_eq!(joined("a", "../../b"), "../b");
    }
}
