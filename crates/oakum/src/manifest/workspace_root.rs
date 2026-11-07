//! Which Cargo members a change to the workspace's root `Cargo.toml` or
//! `Cargo.lock` reaches (ADR-0039), from both sides' text. Reading the sides is
//! the caller's.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use semver::{Version, VersionReq};
use toml::{Table, Value};

use crate::plan::{Ecosystem, ExternalDependency, Package, PackageId, Workspace};

/// One side of the comparison; `None` is a file absent on that side.
#[derive(Clone, Copy, Debug, Default)]
pub struct RootFiles<'a> {
    pub manifest: Option<&'a str>,
    pub lock: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RootFile {
    Lock,
    Manifest,
}

impl RootFile {
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Lock => "Cargo.lock",
            Self::Manifest => "Cargo.toml",
        }
    }
}

/// What in a root file changed: `[workspace.dependencies] rusqlite` in the
/// manifest, or the dependency a member declares in the lock.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootReason {
    pub file: RootFile,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootChanges {
    pub members: BTreeMap<PackageId, BTreeSet<RootReason>>,
    /// Workspace-wide settings, which every managed Cargo package ships.
    pub everywhere: BTreeSet<RootReason>,
}

impl RootChanges {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty() && self.everywhere.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootFileError {
    pub file: RootFile,
    pub message: String,
}

impl fmt::Display for RootFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file.file_name(), self.message)
    }
}

impl std::error::Error for RootFileError {}

/// Top-level keys of a root package's own manifest, which reach that package
/// by its path rather than through this module.
const PACKAGE_OWN: [&str; 13] = [
    "package",
    "lib",
    "bin",
    "example",
    "test",
    "bench",
    "dependencies",
    "dev-dependencies",
    "dev_dependencies",
    "build-dependencies",
    "build_dependencies",
    "target",
    "features",
];

/// # Errors
///
/// [`RootFileError`] when either side of a file that differs is not TOML, or
/// is a lock whose rows cannot be read.
pub fn root_changes(
    workspace: &Workspace,
    base: RootFiles<'_>,
    head: RootFiles<'_>,
) -> Result<RootChanges, RootFileError> {
    let mut changes = RootChanges::default();
    if base.manifest != head.manifest {
        manifest_changes(
            workspace,
            &parse(RootFile::Manifest, base.manifest)?,
            &parse(RootFile::Manifest, head.manifest)?,
            &mut changes,
        );
    }
    if base.lock != head.lock {
        lock_changes(
            workspace,
            &Lock::parse(base.lock)?,
            &Lock::parse(head.lock)?,
            &mut changes,
        );
    }
    Ok(changes)
}

fn parse(file: RootFile, text: Option<&str>) -> Result<Table, RootFileError> {
    text.map_or_else(
        || Ok(Table::new()),
        |text| {
            toml::from_str(text).map_err(|err| RootFileError {
                file,
                message: err.to_string(),
            })
        },
    )
}

fn cargo_members(workspace: &Workspace) -> impl Iterator<Item = &crate::plan::Package> {
    workspace
        .packages()
        .filter(|package| package.id().ecosystem == Ecosystem::Cargo)
}

fn reach(changes: &mut RootChanges, id: &PackageId, file: RootFile, name: String) {
    changes
        .members
        .entry(id.clone())
        .or_default()
        .insert(RootReason { file, name });
}

/// Keys whose value differs between the two tables, either side missing.
fn changed_keys<'a>(base: &'a Table, head: &'a Table) -> BTreeSet<&'a str> {
    base.keys()
        .chain(head.keys())
        .map(String::as_str)
        .filter(|key| base.get(*key) != head.get(*key))
        .collect()
}

fn sub_table(table: &Table, key: &str) -> Table {
    table
        .get(key)
        .and_then(Value::as_table)
        .cloned()
        .unwrap_or_default()
}

fn is_table(base: &Table, head: &Table, key: &str) -> bool {
    [base, head]
        .iter()
        .any(|side| side.get(key).is_some_and(Value::is_table))
}

fn manifest_changes(workspace: &Workspace, base: &Table, head: &Table, changes: &mut RootChanges) {
    for key in changed_keys(base, head) {
        if key == "workspace" {
            workspace_changes(workspace, base, head, changes);
        } else if !PACKAGE_OWN.contains(&key) {
            let name = if is_table(base, head, key) {
                format!("[{key}]")
            } else {
                key.to_owned()
            };
            changes.everywhere.insert(RootReason {
                file: RootFile::Manifest,
                name,
            });
        }
    }
}

fn workspace_changes(workspace: &Workspace, base: &Table, head: &Table, changes: &mut RootChanges) {
    let (base, head) = (sub_table(base, "workspace"), sub_table(head, "workspace"));
    for key in changed_keys(&base, &head) {
        match key {
            "metadata" => {}
            "dependencies" => {
                let (base, head) = (sub_table(&base, key), sub_table(&head, key));
                for declared in changed_keys(&base, &head) {
                    if only_a_path_version_moved(&base, &head, declared) {
                        continue;
                    }
                    for package in cargo_members(workspace) {
                        let inherits = package.inherited_path_dependencies().contains(declared)
                            || package.external_dependencies().iter().any(|dep| {
                                dep.inherited
                                    && dep.kind.is_runtime()
                                    && dep.declared_as == declared
                            });
                        if inherits {
                            let name = format!("[workspace.dependencies] {declared}");
                            reach(changes, package.id(), RootFile::Manifest, name);
                        }
                    }
                }
            }
            "package" => {
                let (base, head) = (sub_table(&base, key), sub_table(&head, key));
                // `version` is what `version` itself writes for an inheriting
                // member; a hand-set one is the `hand_set` report's.
                for field in changed_keys(&base, &head)
                    .into_iter()
                    .filter(|field| *field != "version")
                {
                    for package in cargo_members(workspace) {
                        if package.inherited_fields().contains(field) {
                            let name = format!("[workspace.package] {field}");
                            reach(changes, package.id(), RootFile::Manifest, name);
                        }
                    }
                }
            }
            _ => {
                let name = if is_table(&base, &head, key) {
                    format!("[workspace.{key}]")
                } else {
                    format!("[workspace] {key}")
                };
                changes.everywhere.insert(RootReason {
                    file: RootFile::Manifest,
                    name,
                });
            }
        }
    }
}

/// A path entry's `version` is what `version` writes when the crate it names
/// is bumped; the crate's own change reaches through the workspace graph.
fn only_a_path_version_moved(base: &Table, head: &Table, key: &str) -> bool {
    match (
        base.get(key).and_then(Value::as_table),
        head.get(key).and_then(Value::as_table),
    ) {
        (Some(base), Some(head)) => {
            base.contains_key("path")
                && head.contains_key("path")
                && changed_keys(base, head) == BTreeSet::from(["version"])
        }
        _ => false,
    }
}

struct Row {
    name: String,
    version: String,
    source: Option<String>,
    dependencies: Vec<String>,
    children: Vec<usize>,
    /// `dependencies` as [`Lock::identity`]s: Cargo writes `name version` only
    /// while two versions are locked, so the spelling moves on its own.
    whole: Table,
}

/// The `[[package]]` rows of one side of a `Cargo.lock`.
struct Lock {
    rows: Vec<Row>,
}

impl Lock {
    fn parse(text: Option<&str>) -> Result<Self, RootFileError> {
        let fail = |message: String| RootFileError {
            file: RootFile::Lock,
            message,
        };
        let table = parse(RootFile::Lock, text)?;
        let listed = match table.get("package") {
            None => return Ok(Self { rows: Vec::new() }),
            Some(Value::Array(listed)) => listed,
            Some(_) => return Err(fail(String::from("`package` is not a list of rows"))),
        };
        let mut rows = Vec::with_capacity(listed.len());
        for row in listed {
            let whole = row
                .as_table()
                .ok_or_else(|| fail(String::from("a `[[package]]` row is not a table")))?;
            let text = |key: &str| whole.get(key).and_then(Value::as_str).map(str::to_owned);
            let (Some(name), Some(version)) = (text("name"), text("version")) else {
                return Err(fail(String::from(
                    "a `[[package]]` row has no `name` or `version`",
                )));
            };
            let dependencies = match whole.get("dependencies") {
                None => Vec::new(),
                Some(Value::Array(specs)) => specs
                    .iter()
                    .map(|spec| spec.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
                    .ok_or_else(|| fail(format!("`{name}` lists a dependency that is not text")))?,
                Some(_) => return Err(fail(format!("`{name}`'s dependencies are not a list"))),
            };
            rows.push(Row {
                name,
                version,
                source: text("source"),
                dependencies,
                children: Vec::new(),
                whole: whole.clone(),
            });
        }
        let mut lock = Self { rows };
        for at in 0..lock.rows.len() {
            let (children, identities): (Vec<Option<usize>>, Vec<String>) = lock.rows[at]
                .dependencies
                .iter()
                .map(|spec| {
                    let child = lock.resolve(spec);
                    (
                        child,
                        child.map_or_else(|| spec.clone(), |child| lock.identity(child)),
                    )
                })
                .unzip();
            let mut identities: Vec<Value> = identities.into_iter().map(Value::String).collect();
            identities.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            let row = &mut lock.rows[at];
            row.children = children.into_iter().flatten().collect();
            if !identities.is_empty() {
                row.whole
                    .insert(String::from("dependencies"), Value::Array(identities));
            }
        }
        Ok(lock)
    }

    /// A row as a `dependencies` entry names it with nothing left out, and a
    /// git commit aside: that row moving is its own change.
    fn identity(&self, index: usize) -> String {
        let row = &self.rows[index];
        // A sourceless child is the workspace's own, whose version `version` moves.
        row.source.as_deref().map_or_else(
            || row.name.clone(),
            |source| format!("{} {} {}", row.name, row.version, without_fragment(source)),
        )
    }

    /// Registry and git rows keyed by identity. Sourceless rows are the
    /// workspace's own (and path crates), which `version` retargets.
    fn sourced(&self) -> BTreeMap<RowKey<'_>, &Table> {
        self.rows
            .iter()
            .filter_map(|row| {
                let source = row.source.as_deref()?;
                Some((
                    (row.name.as_str(), row.version.as_str(), source),
                    &row.whole,
                ))
            })
            .collect()
    }

    fn index_of(&self, name: &str, version: &str, source: &str) -> Option<usize> {
        self.rows.iter().position(|row| {
            row.name == name && row.version == version && row.source.as_deref() == Some(source)
        })
    }

    /// The row a `dependencies` entry names: `name`, `name version`, or
    /// `name version (source)`, as Cargo writes just enough to be unique.
    fn resolve(&self, spec: &str) -> Option<usize> {
        let mut parts = spec.splitn(3, ' ');
        let name = parts.next()?;
        let version = parts.next();
        let source = parts
            .next()
            .map(|source| source.trim_start_matches('(').trim_end_matches(')'));
        let mut found = self.rows.iter().enumerate().filter(|(_, row)| {
            row.name == name
                && version.is_none_or(|version| row.version == version)
                && source.is_none_or(|source| row.source.as_deref() == Some(source))
        });
        let (index, _) = found.next()?;
        found.next().is_none().then_some(index)
    }

    fn child_identities(&self, index: usize) -> BTreeSet<String> {
        self.rows[index]
            .children
            .iter()
            .map(|&child| self.identity(child))
            .collect()
    }

    fn dependents(&self) -> Vec<Vec<usize>> {
        let mut dependents = vec![Vec::new(); self.rows.len()];
        for (parent, row) in self.rows.iter().enumerate() {
            for &child in &row.children {
                dependents[child].push(parent);
            }
        }
        dependents
    }
}

fn without_fragment(source: &str) -> &str {
    source.split_once('#').map_or(source, |(kept, _)| kept)
}

/// Whether `member`, whose lock row lists `row`, ships it. `cargo metadata
/// --no-deps` reports a source from before `[patch]`, so a name match on a
/// runtime declaration suffices; source and version requirement only choose
/// among declarations of one name.
fn ships(member: &Package, row: &Row) -> bool {
    let named = member
        .external_dependencies()
        .iter()
        .filter(|declared| declared.name == row.name)
        .collect();
    let locked = row.source.as_deref().map(without_fragment);
    let by_source = narrowed(named, |declared| {
        declared.source.as_deref().map(without_fragment) == locked
    });
    let version = Version::parse(&row.version).ok();
    narrowed(by_source, |declared| {
        version
            .as_ref()
            .is_none_or(|version| meets(&declared.req, version))
    })
    .iter()
    .any(|declared| declared.kind.is_runtime())
}

/// The declarations `keep` accepts, or all of them when it accepts none.
fn narrowed(
    declared: Vec<&ExternalDependency>,
    keep: impl Fn(&ExternalDependency) -> bool,
) -> Vec<&ExternalDependency> {
    let kept: Vec<_> = declared
        .iter()
        .copied()
        .filter(|declared| keep(declared))
        .collect();
    if kept.is_empty() {
        declared
    } else {
        kept
    }
}

/// `req` matches `version`, a pre-release counted as its release: semver keeps
/// pre-releases out of `*`, which a git dependency at `0.1.0-dev` is declared as.
fn meets(req: &VersionReq, version: &Version) -> bool {
    req.matches(version)
        || (!version.pre.is_empty()
            && req.matches(&Version::new(version.major, version.minor, version.patch)))
}

type RowKey<'a> = (&'a str, &'a str, &'a str);

/// Rows of `from` that `to` lacks or holds differently.
fn moved<'a>(
    from: &BTreeMap<RowKey<'a>, &Table>,
    to: &BTreeMap<RowKey<'_>, &Table>,
) -> Vec<RowKey<'a>> {
    from.iter()
        .filter(|(key, row)| to.get(*key) != Some(*row))
        .map(|(key, _)| *key)
        .collect()
}

fn lock_changes(workspace: &Workspace, base: &Lock, head: &Lock, changes: &mut RootChanges) {
    let (base_rows, head_rows) = (base.sourced(), head.sourced());
    for (lock, keys) in [
        (head, moved(&head_rows, &base_rows)),
        (base, moved(&base_rows, &head_rows)),
    ] {
        let starts: Vec<usize> = keys
            .into_iter()
            .filter_map(|(name, version, source)| lock.index_of(name, version, source))
            .collect();
        walk(workspace, lock, &starts, changes);
    }
    reselections(workspace, base, head, changes);
    reselections(workspace, head, base, changes);
}

/// A sourceless row that picks another of the versions locked: no sourced row
/// moves when every version stays locked for some other crate. A member reaches
/// the picked crate it ships; an out-of-workspace path crate starts a walk.
fn reselections(workspace: &Workspace, lock: &Lock, other: &Lock, changes: &mut RootChanges) {
    let members: BTreeMap<&str, &crate::plan::Package> = cargo_members(workspace)
        .map(|package| (package.id().name.as_str(), package))
        .collect();
    let mut starts = Vec::new();
    for (at, row) in lock.rows.iter().enumerate() {
        if row.source.is_some() {
            continue;
        }
        let Some(theirs) = other
            .rows
            .iter()
            .position(|them| them.source.is_none() && them.name == row.name)
        else {
            continue;
        };
        let picked = other.child_identities(theirs);
        let gone: Vec<usize> = row
            .children
            .iter()
            .copied()
            .filter(|&child| !picked.contains(&lock.identity(child)))
            .collect();
        if gone.is_empty() {
            continue;
        }
        match members.get(row.name.as_str()) {
            Some(member) => {
                for child in gone {
                    let child = &lock.rows[child];
                    if ships(member, child) {
                        reach(changes, member.id(), RootFile::Lock, child.name.clone());
                    }
                }
            }
            None => starts.push(at),
        }
    }
    walk(workspace, lock, &starts, changes);
}

/// Up the lock's reverse edges from `starts` to the members that declare the
/// last row passed through as a runtime dependency. A member row ends the walk:
/// a crate it ships reaches its dependents through the workspace graph.
fn walk(workspace: &Workspace, lock: &Lock, starts: &[usize], changes: &mut RootChanges) {
    if starts.is_empty() {
        return;
    }
    let members: BTreeMap<&str, &crate::plan::Package> = cargo_members(workspace)
        .map(|package| (package.id().name.as_str(), package))
        .collect();
    let member_of = |index: usize| {
        let row = &lock.rows[index];
        row.source
            .is_none()
            .then(|| members.get(row.name.as_str()).copied())
            .flatten()
    };
    let dependents = lock.dependents();
    let mut seen: BTreeSet<usize> = starts.iter().copied().collect();
    let mut queue: VecDeque<usize> = starts.iter().copied().collect();
    while let Some(at) = queue.pop_front() {
        for &parent in &dependents[at] {
            if let Some(member) = member_of(parent) {
                let row = &lock.rows[at];
                if ships(member, row) {
                    reach(changes, member.id(), RootFile::Lock, row.name.clone());
                }
            } else if seen.insert(parent) {
                queue.push_back(parent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use semver::{Version, VersionReq};

    use super::*;
    use crate::plan::{DependencyKind, Package, ResolvesDependenciesAt};

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn external(name: &str, kind: DependencyKind, inherited: bool) -> ExternalDependency {
        ExternalDependency {
            name: name.to_owned(),
            declared_as: name.to_owned(),
            kind,
            source: Some(CRATES_IO.to_owned()),
            req: VersionReq::STAR,
            inherited,
        }
    }

    /// `store` inherits `rusqlite` and dev-only `tempfile`, declares `log` on its
    /// own, and inherits `edition`; `cli` inherits nothing.
    fn workspace() -> Workspace {
        let package = |name: &str| {
            Package::new(
                PackageId::new(Ecosystem::Cargo, name),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                false,
                Vec::new(),
            )
        };
        Workspace::new([
            package("cli"),
            package("store")
                .with_external_dependencies(vec![
                    external("rusqlite", DependencyKind::Normal, true),
                    external("tempfile", DependencyKind::Development, true),
                    external("log", DependencyKind::Normal, false),
                    ExternalDependency {
                        source: Some(String::from(
                            "git+https://example.invalid/g.git?branch=main",
                        )),
                        ..external("gitdep", DependencyKind::Build, false)
                    },
                ])
                .with_inherited_fields(BTreeSet::from([String::from("edition")]))
                .with_inherited_path_dependencies(BTreeSet::from([String::from("core")])),
        ])
        .expect("workspace")
    }

    fn store() -> PackageId {
        PackageId::new(Ecosystem::Cargo, "store")
    }

    fn manifest_changes_of(base: &str, head: &str) -> RootChanges {
        root_changes(
            &workspace(),
            RootFiles {
                manifest: Some(base),
                lock: None,
            },
            RootFiles {
                manifest: Some(head),
                lock: None,
            },
        )
        .expect("changes")
    }

    fn reasons(changes: &RootChanges, id: &PackageId) -> Vec<String> {
        changes
            .members
            .get(id)
            .map(|reasons| {
                reasons
                    .iter()
                    .map(|reason| format!("{}: {}", reason.file.file_name(), reason.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    const MANIFEST: &str = r#"[workspace]
members = ["cli", "store"]

[workspace.package]
edition = "2021"
version = "0.1.0"

[workspace.dependencies]
core = { path = "core", version = "0.1.0" }
rusqlite = { version = "0.31", features = ["bundled"] }
tempfile = "3"
log = "0.4"

[workspace.metadata.x]
a = 1
"#;

    #[test]
    fn an_inherited_dependency_entry_reaches_its_runtime_inheritors_only() {
        for (from, to) in [
            ("\"0.31\"", "\"0.32\""),
            ("[\"bundled\"]", "[\"bundled\", \"functions\"]"),
        ] {
            let changes = manifest_changes_of(MANIFEST, &MANIFEST.replace(from, to));
            assert_eq!(
                reasons(&changes, &store()),
                ["Cargo.toml: [workspace.dependencies] rusqlite"]
            );
            assert_eq!(changes.members.len(), 1);
            assert!(changes.everywhere.is_empty());
        }
    }

    #[test]
    fn a_path_entry_reaches_its_runtime_inheritors_unless_only_its_version_moved() {
        for to in [
            "core = { path = \"core\", version = \"0.1.0\", default-features = false }",
            "core = { path = \"crates/core\", version = \"0.1.0\" }",
        ] {
            let changes = manifest_changes_of(
                MANIFEST,
                &MANIFEST.replace("core = { path = \"core\", version = \"0.1.0\" }", to),
            );
            assert_eq!(
                reasons(&changes, &store()),
                ["Cargo.toml: [workspace.dependencies] core"],
                "{to}"
            );
        }
        let changes =
            manifest_changes_of(MANIFEST, &MANIFEST.replace("\"0.1.0\" }", "\"0.2.0\" }"));
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn dev_and_written_out_entries_reach_nothing() {
        for (from, to) in [
            ("tempfile = \"3\"", "tempfile = \"4\""),
            ("log = \"0.4\"", "log = \"0.5\""),
        ] {
            let changes = manifest_changes_of(MANIFEST, &MANIFEST.replace(from, to));
            assert!(changes.is_empty(), "{from}: {changes:?}");
        }
    }

    #[test]
    fn an_inherited_package_field_reaches_its_inheritors_and_version_reaches_nothing() {
        let changes = manifest_changes_of(MANIFEST, &MANIFEST.replace("\"2021\"", "\"2024\""));
        assert_eq!(
            reasons(&changes, &store()),
            ["Cargo.toml: [workspace.package] edition"]
        );
        let changes = manifest_changes_of(MANIFEST, &MANIFEST.replace("\"0.1.0\"", "\"0.2.0\""));
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn workspace_settings_reach_everything_and_metadata_nothing() {
        let changes = manifest_changes_of(
            MANIFEST,
            &format!("{MANIFEST}\n[profile.release]\nlto = true\n"),
        );
        assert!(changes.members.is_empty());
        let names: Vec<&str> = changes.everywhere.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["[profile]"]);
        let changes = manifest_changes_of(
            MANIFEST,
            &MANIFEST.replace("[workspace]\n", "[workspace]\nresolver = \"2\"\n"),
        );
        let names: Vec<&str> = changes.everywhere.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["[workspace] resolver"]);
        let changes = manifest_changes_of(MANIFEST, &MANIFEST.replace("a = 1", "a = 2"));
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn formatting_alone_is_no_change() {
        let changes = manifest_changes_of(
            MANIFEST,
            &MANIFEST.replace("tempfile = \"3\"", "tempfile   =   \"3\" # pinned"),
        );
        assert!(changes.is_empty(), "{changes:?}");
    }

    const LOCK: &str = r#"version = 4

[[package]]
name = "cli"
version = "0.1.0"
dependencies = ["store"]

[[package]]
name = "fastrand"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "gitdep"
version = "0.1.0"
source = "git+https://example.invalid/g.git?branch=main#aaaa"

[[package]]
name = "libsqlite3-sys"
version = "0.28.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "rusqlite"
version = "0.31.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = ["libsqlite3-sys"]

[[package]]
name = "store"
version = "0.1.0"
dependencies = ["gitdep", "rusqlite", "tempfile"]

[[package]]
name = "tempfile"
version = "3.10.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = ["fastrand"]
"#;

    fn lock_changes_of(
        base: Option<&str>,
        head: Option<&str>,
    ) -> Result<RootChanges, RootFileError> {
        root_changes(
            &workspace(),
            RootFiles {
                manifest: None,
                lock: base,
            },
            RootFiles {
                manifest: None,
                lock: head,
            },
        )
    }

    #[test]
    fn a_transitive_row_is_named_by_the_dependency_the_member_declares() {
        let head = LOCK.replace("\"0.28.0\"", "\"0.28.1\"");
        let changes = lock_changes_of(Some(LOCK), Some(&head)).expect("changes");
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: rusqlite"]);
        assert_eq!(changes.members.len(), 1, "{changes:?}");
    }

    #[test]
    fn a_git_row_matches_its_declaration_without_the_commit() {
        let head = LOCK.replace("#aaaa", "#bbbb");
        let changes = lock_changes_of(Some(LOCK), Some(&head)).expect("changes");
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: gitdep"]);
    }

    #[test]
    fn a_dev_only_row_reaches_nothing() {
        let head = LOCK.replace("\"2.0.0\"", "\"2.1.0\"");
        let changes = lock_changes_of(Some(LOCK), Some(&head)).expect("changes");
        assert!(changes.is_empty(), "{changes:?}");
    }

    /// What `version` writes into the lock.
    #[test]
    fn a_member_row_moving_is_not_a_change() {
        let moved = LOCK.replace(
            "name = \"store\"\nversion = \"0.1.0\"",
            "name = \"store\"\nversion = \"0.2.0\"",
        );
        let (base, head) = (
            Lock::parse(Some(LOCK)).expect("base"),
            Lock::parse(Some(&moved)).expect("head"),
        );
        assert_eq!(base.sourced(), head.sourced());
        let changes = lock_changes_of(Some(LOCK), Some(&moved)).expect("changes");
        assert!(changes.is_empty(), "{changes:?}");
    }

    /// Both versions stay locked for other crates, so no sourced row moves; only
    /// the member's own row picks the other one.
    #[test]
    fn a_member_switching_between_locked_versions_is_a_change() {
        let lock = |picked: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.3.9\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"1111111111111111111111111111111111111111111111111111111111111111\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.4.21\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"2222222222222222222222222222222222222222222222222222222222222222\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"log {picked}\", \"tempfile\"]\n\n\
                 [[package]]\nname = \"tempfile\"\nversion = \"3.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"3333333333333333333333333333333333333333333333333333333333333333\"\n\
                 dependencies = [\"log 0.3.9\", \"log 0.4.21\"]\n"
            )
        };
        let changes =
            lock_changes_of(Some(&lock("0.3.9")), Some(&lock("0.4.21"))).expect("changes");
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: log"]);
    }

    #[test]
    fn a_member_switching_a_dev_dependency_between_locked_versions_asks_nothing() {
        let lock = |picked: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"other\"\nversion = \"1.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"1111111111111111111111111111111111111111111111111111111111111111\"\n\
                 dependencies = [\"tempfile 2.0.0\", \"tempfile 3.0.0\"]\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"tempfile {picked}\"]\n\n\
                 [[package]]\nname = \"tempfile\"\nversion = \"2.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"2222222222222222222222222222222222222222222222222222222222222222\"\n\n\
                 [[package]]\nname = \"tempfile\"\nversion = \"3.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"3333333333333333333333333333333333333333333333333333333333333333\"\n"
            )
        };
        let changes = lock_changes_of(Some(&lock("2.0.0")), Some(&lock("3.0.0"))).expect("changes");
        assert!(changes.members.is_empty(), "{changes:?}");
    }

    /// A path crate outside the workspace is a sourceless row the walk passes
    /// through to the member that declares it.
    #[test]
    fn a_lock_change_reaches_a_member_through_a_path_crate_outside_the_workspace() {
        let lock = |log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"cli\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\ndependencies = [\"log\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"{log}\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"1111111111111111111111111111111111111111111111111111111111111111\"\n"
            )
        };
        let helper = ExternalDependency {
            source: None,
            ..external("helper", DependencyKind::Normal, false)
        };
        let workspace = Workspace::new([Package::new(
            PackageId::new(Ecosystem::Cargo, "cli"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            false,
            Vec::new(),
        )
        .with_external_dependencies(vec![helper])])
        .expect("workspace");
        let changes = root_changes(
            &workspace,
            RootFiles {
                manifest: None,
                lock: Some(&lock("0.4.20")),
            },
            RootFiles {
                manifest: None,
                lock: Some(&lock("0.4.21")),
            },
        )
        .expect("changes");
        assert_eq!(
            reasons(&changes, &PackageId::new(Ecosystem::Cargo, "cli")),
            ["Cargo.lock: helper"]
        );
    }

    /// A member `[patch]`ed in under a registry crate is still the workspace's
    /// own row, which `version` retargets.
    #[test]
    fn a_member_moving_under_a_registry_row_is_not_a_change() {
        let lock = |core: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"core\"\nversion = \"{core}\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"1.0.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
                 checksum = \"1111111111111111111111111111111111111111111111111111111111111111\"\n\
                 dependencies = [\"core\"]\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\ndependencies = [\"log\"]\n"
            )
        };
        let changes = lock_changes_of(Some(&lock("0.1.0")), Some(&lock("0.2.0"))).expect("changes");
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn a_lock_added_counts_every_row_and_one_removed_walks_the_old_side() {
        let changes = lock_changes_of(None, Some(LOCK)).expect("changes");
        assert_eq!(
            reasons(&changes, &store()),
            ["Cargo.lock: gitdep", "Cargo.lock: rusqlite"]
        );
        let changes = lock_changes_of(Some(LOCK), None).expect("changes");
        assert_eq!(
            reasons(&changes, &store()),
            ["Cargo.lock: gitdep", "Cargo.lock: rusqlite"]
        );
    }

    /// Cargo writes `"name"` while one version of a crate is locked and
    /// `"name version"` once a second appears.
    #[test]
    fn a_dependency_re_spelled_for_a_second_version_is_not_a_change() {
        let base = LOCK
            .replace(
                "dependencies = [\"libsqlite3-sys\"]",
                "dependencies = [\"bitflags\", \"libsqlite3-sys\"]",
            )
            .replace(
                "[[package]]\nname = \"cli\"",
                "[[package]]\nname = \"bitflags\"\nversion = \"1.3.2\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
                 [[package]]\nname = \"cli\"",
            );
        let head = base
            .replace(
                "dependencies = [\"bitflags\", \"libsqlite3-sys\"]",
                "dependencies = [\"bitflags 1.3.2\", \"libsqlite3-sys\"]",
            )
            .replace(
                "dependencies = [\"fastrand\"]",
                "dependencies = [\"bitflags 2.4.0\", \"fastrand\"]",
            )
            .replace(
                "[[package]]\nname = \"cli\"",
                "[[package]]\nname = \"bitflags\"\nversion = \"2.4.0\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
                 [[package]]\nname = \"cli\"",
            );
        assert_ne!(base, head);
        let changes = lock_changes_of(Some(&base), Some(&head)).expect("changes");
        assert!(changes.is_empty(), "{changes:?}");
    }

    fn store_declaring(externals: Vec<ExternalDependency>) -> Workspace {
        Workspace::new([Package::new(
            store(),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            false,
            Vec::new(),
        )
        .with_external_dependencies(externals)])
        .expect("workspace")
    }

    fn lock_changes_in(workspace: &Workspace, base: &str, head: &str) -> RootChanges {
        root_changes(
            workspace,
            RootFiles {
                manifest: None,
                lock: Some(base),
            },
            RootFiles {
                manifest: None,
                lock: Some(head),
            },
        )
        .expect("changes")
    }

    /// `cargo metadata --no-deps` reports the source before `[patch]`; the lock
    /// holds the patched one.
    #[test]
    fn a_patched_row_ships_through_its_registry_declaration() {
        let lock = |commit: &str| {
            format!(
                "version = 4\n\n[[package]]\nname = \"rusqlite\"\nversion = \"0.31.0\"\n\
                 source = \"git+https://example.invalid/rusqlite.git?branch=fix#{commit}\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\ndependencies = [\"rusqlite\"]\n"
            )
        };
        let workspace = store_declaring(vec![ExternalDependency {
            req: VersionReq::parse("0.31").expect("req"),
            ..external("rusqlite", DependencyKind::Normal, true)
        }]);
        let changes = lock_changes_in(&workspace, &lock("aaaa"), &lock("bbbb"));
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: rusqlite"]);
    }

    /// `rand` at runtime from the registry, and from git as a dev-only rename.
    #[test]
    fn a_name_declared_from_two_sources_ships_by_the_source_that_matches() {
        const GIT: &str = "git+https://example.invalid/rand.git?branch=main";
        let lock = |registry: &str, commit: &str| {
            format!(
                "version = 4\n\n[[package]]\nname = \"rand\"\nversion = \"{registry}\"\n\
                 source = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"0.8.5\"\nsource = \"{GIT}#{commit}\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\ndependencies = [\
                 \"rand {registry} ({CRATES_IO})\", \"rand 0.8.5 ({GIT}#{commit})\"]\n"
            )
        };
        let workspace = store_declaring(vec![
            external("rand", DependencyKind::Normal, false),
            ExternalDependency {
                declared_as: String::from("rand-git"),
                source: Some(GIT.to_owned()),
                ..external("rand", DependencyKind::Development, false)
            },
        ]);
        let changes = lock_changes_in(&workspace, &lock("0.8.5", "aaaa"), &lock("0.8.5", "bbbb"));
        assert!(changes.is_empty(), "{changes:?}");
        let changes = lock_changes_in(&workspace, &lock("0.8.5", "aaaa"), &lock("0.8.6", "aaaa"));
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: rand"]);
    }

    /// `rand = "0.8"` at runtime, `rand09 = { package = "rand", version = "0.9" }`
    /// for tests only.
    #[test]
    fn a_name_declared_at_two_versions_ships_by_the_requirement_that_matches() {
        let lock = |old: &str, new: &str| {
            format!(
                "version = 4\n\n[[package]]\nname = \"rand\"\nversion = \"{old}\"\n\
                 source = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"{new}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"rand {old}\", \"rand {new}\"]\n"
            )
        };
        let workspace = store_declaring(vec![
            ExternalDependency {
                req: VersionReq::parse("0.8").expect("req"),
                ..external("rand", DependencyKind::Normal, false)
            },
            ExternalDependency {
                declared_as: String::from("rand09"),
                req: VersionReq::parse("0.9").expect("req"),
                ..external("rand", DependencyKind::Development, false)
            },
        ]);
        let changes = lock_changes_in(&workspace, &lock("0.8.5", "0.9.1"), &lock("0.8.5", "0.9.2"));
        assert!(changes.is_empty(), "{changes:?}");
        let changes = lock_changes_in(&workspace, &lock("0.8.5", "0.9.1"), &lock("0.8.6", "0.9.1"));
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: rand"]);
    }

    #[test]
    fn an_unreadable_side_is_an_error_not_an_empty_lock() {
        let err = lock_changes_of(Some("[[package]\nname = "), Some(LOCK)).expect_err("garbled");
        assert_eq!(err.file, RootFile::Lock);
        let err = lock_changes_of(Some("package = 1\n"), Some(LOCK)).expect_err("not rows");
        assert_eq!(err.file, RootFile::Lock);
    }
}
