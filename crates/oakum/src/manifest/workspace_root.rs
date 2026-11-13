//! Which Cargo members a change to the workspace's root `Cargo.toml` or
//! `Cargo.lock` reaches (ADR-0039), from both sides' text. Reading the sides is
//! the caller's.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use semver::{Version, VersionReq};
use toml::{Table, Value};

use super::Declarations;
use crate::plan::{Bounds, Ecosystem, ExternalDependency, Package, PackageId, Workspace};

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
const PACKAGE_OWN: [&str; 15] = [
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
    "lints",
    "badges",
];

/// Base lock rows are read against `base_members` (a member it lacks was
/// absent there), head rows against `workspace`'s own declarations.
///
/// # Errors
///
/// [`RootFileError`] when either side of a file that differs is not TOML, or
/// is a lock whose rows cannot be read.
pub fn root_changes(
    workspace: &Workspace,
    base_members: &BTreeMap<PackageId, Declarations>,
    base: RootFiles<'_>,
    head: RootFiles<'_>,
) -> Result<RootChanges, RootFileError> {
    let mut changes = RootChanges::default();
    if base.manifest != head.manifest {
        manifest_changes(
            workspace,
            base_members,
            &parse(RootFile::Manifest, base.manifest)?,
            &parse(RootFile::Manifest, head.manifest)?,
            &mut changes,
        );
    }
    if base.lock != head.lock {
        lock_changes(
            workspace,
            base_members,
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

fn manifest_changes(
    workspace: &Workspace,
    base_members: &BTreeMap<PackageId, Declarations>,
    base: &Table,
    head: &Table,
    changes: &mut RootChanges,
) {
    for key in changed_keys(base, head) {
        if key == "workspace" {
            workspace_changes(workspace, base_members, base, head, changes);
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

fn workspace_changes(
    workspace: &Workspace,
    base_members: &BTreeMap<PackageId, Declarations>,
    base: &Table,
    head: &Table,
    changes: &mut RootChanges,
) {
    let (base, head) = (sub_table(base, "workspace"), sub_table(head, "workspace"));
    for key in changed_keys(&base, &head) {
        match key {
            "metadata" => {}
            "dependencies" => {
                let (base, head) = (sub_table(&base, key), sub_table(&head, key));
                for declared in changed_keys(&base, &head) {
                    let edge_to = cargo_members(workspace)
                        .filter(|package| package.inherited_path_dependencies().contains(declared))
                        .flat_map(Package::dependencies)
                        .find(|edge| edge.declared_as == declared)
                        .and_then(|edge| workspace.get(&edge.on));
                    if only_a_path_version_moved(&base, &head, declared)
                        || edge_to.is_some_and(|member| {
                            base_members.get(member.id()).is_some_and(|was| {
                                retargeted_by_version(
                                    &base,
                                    &head,
                                    declared,
                                    (&was.version, member.version()),
                                )
                            })
                        })
                    {
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

/// Whether `key` moved only as `version` retargets an edge: the base
/// requirement met the member's version `was`, and the head one is what
/// `version` writes retargeting it at `now`. Any other edit can change what
/// cargo builds through a `[patch]`.
fn retargeted_by_version(
    base: &Table,
    head: &Table,
    key: &str,
    (was, now): (&Version, &Version),
) -> bool {
    let requirement = |value: Option<&Value>| -> Option<(String, Option<Table>)> {
        match value? {
            Value::String(text) => Some((text.clone(), None)),
            Value::Table(table) => {
                let mut rest = table.clone();
                let text = rest.remove("version")?.as_str()?.to_owned();
                Some((text, Some(rest)))
            }
            _ => None,
        }
    };
    let (Some((base_text, base_rest)), Some((head_text, head_rest))) =
        (requirement(base.get(key)), requirement(head.get(key)))
    else {
        return false;
    };
    base_rest == head_rest
        && Bounds::from_cargo_text(&base_text).is_ok_and(|bounds| {
            bounds.matches(was)
                && bounds
                    .retargeted(now)
                    .is_some_and(|written| written == head_text)
        })
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
/// runtime declaration suffices. Among declarations of one name, those
/// matching the row's source and version both decide; with none, those
/// matching its version, since `[patch]` rewrites a source but must meet the
/// requirement; with none of those, all of them stand. A declaration whose
/// source is unknown neither matches nor rules out a row by source: it joins
/// the declarations a source match chose when its requirement matches, and
/// otherwise stands with the rest.
fn ships(member: Member<'_>, row: &Row) -> bool {
    let of_row = |declared: &&ExternalDependency| declared.name == row.name;
    let named: Vec<&ExternalDependency> = member.declared.iter().filter(of_row).collect();
    let unknown: Vec<&ExternalDependency> = member.source_unknown.iter().filter(of_row).collect();
    let locked = row.source.as_deref().map(without_fragment);
    let version = Version::parse(&row.version).ok();
    let by_version = |declared: &ExternalDependency| {
        version
            .as_ref()
            .is_none_or(|version| meets(&declared.req, version))
    };
    let mut both: Vec<&ExternalDependency> = named
        .iter()
        .copied()
        .filter(|declared| {
            declared.source.as_deref().map(without_fragment) == locked && by_version(declared)
        })
        .collect();
    let chosen = if both.is_empty() {
        narrowed(named.into_iter().chain(unknown).collect(), by_version)
    } else {
        both.extend(unknown.into_iter().filter(|declared| by_version(declared)));
        both
    };
    chosen.iter().any(|declared| declared.kind.is_runtime())
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

/// A member as one side's lock rows are read against: its version and
/// declarations on that side. Graph edges are `HEAD`'s on both.
#[derive(Clone, Copy)]
struct Member<'w> {
    package: &'w Package,
    version: &'w Version,
    declared: &'w [ExternalDependency],
    source_unknown: &'w [ExternalDependency],
}

impl<'w> Member<'w> {
    fn id(self) -> &'w PackageId {
        self.package.id()
    }

    fn at_head(package: &'w Package) -> Self {
        Self {
            package,
            version: package.version(),
            declared: package.external_dependencies(),
            source_unknown: &[],
        }
    }
}

type Members<'w> = BTreeMap<&'w str, Member<'w>>;

fn lock_changes(
    workspace: &Workspace,
    base_members: &BTreeMap<PackageId, Declarations>,
    base: &Lock,
    head: &Lock,
    changes: &mut RootChanges,
) {
    let (base_rows, head_rows) = (base.sourced(), head.sourced());
    let on_head: Members<'_> = cargo_members(workspace)
        .map(|package| (package.id().name.as_str(), Member::at_head(package)))
        .collect();
    let on_base: Members<'_> = cargo_members(workspace)
        .filter_map(|package| {
            let declared = base_members.get(package.id())?;
            Some((
                package.id().name.as_str(),
                Member {
                    package,
                    version: &declared.version,
                    declared: &declared.external_dependencies,
                    source_unknown: &declared.source_unknown,
                },
            ))
        })
        .collect();
    let [base_side, head_side] = sourceless([&on_base, &on_head], base, head);
    for (lock, side, keys) in [
        (head, &head_side, moved(&head_rows, &base_rows)),
        (base, &base_side, moved(&base_rows, &head_rows)),
    ] {
        let starts: Vec<usize> = keys
            .into_iter()
            .filter_map(|(name, version, source)| lock.index_of(name, version, source))
            .collect();
        walk(lock, side, &starts, changes);
    }
    reselections(base, head, &base_side, changes);
    reselections(head, base, &head_side, changes);
}

/// One side's sourceless rows: each one's counterpart in the other side, and
/// the member it is, if any. Sourced rows hold `None` in both.
struct Sourceless<'w> {
    counterpart: Vec<Option<usize>>,
    member: Vec<Option<Member<'w>>>,
    /// The member an unpaired row of its name may be, its version unknown.
    maybe_member: Vec<Option<Member<'w>>>,
}

/// Cargo locks a member and a path crate outside the workspace of one name
/// when their versions differ. A name held by one sourceless row a side is
/// matched by name; otherwise the member is the row at its version on that
/// side, paired with its other side's row, and the rest pair by version, the
/// one row a side left over pairing with the other's (`version` moved it).
fn sourceless<'w>(members: [&Members<'w>; 2], base: &Lock, head: &Lock) -> [Sourceless<'w>; 2] {
    let blank = |lock: &Lock| Sourceless {
        counterpart: vec![None; lock.rows.len()],
        member: vec![None; lock.rows.len()],
        maybe_member: vec![None; lock.rows.len()],
    };
    let mut sides = [blank(base), blank(head)];
    let rows_named = |lock: &Lock, name: &str| -> Vec<usize> {
        (0..lock.rows.len())
            .filter(|&at| lock.rows[at].source.is_none() && lock.rows[at].name == name)
            .collect()
    };
    let names: BTreeSet<&str> = base
        .rows
        .iter()
        .chain(&head.rows)
        .filter(|row| row.source.is_none())
        .map(|row| row.name.as_str())
        .collect();
    for name in names {
        let member = members.map(|members| members.get(name).copied());
        let (mut on_base, mut on_head) = (rows_named(base, name), rows_named(head, name));
        if on_base.len() <= 1 && on_head.len() <= 1 {
            for &at in &on_base {
                sides[0].counterpart[at] = on_head.first().copied();
                sides[0].member[at] = member[0];
            }
            for &at in &on_head {
                sides[1].counterpart[at] = on_base.first().copied();
                sides[1].member[at] = member[1];
            }
            continue;
        }
        let mut members_rows = [None, None];
        for (side, lock, rows) in [(0, base, &on_base), (1, head, &on_head)] {
            for &at in rows {
                let found = member[side].filter(|member| {
                    Version::parse(&lock.rows[at].version).is_ok_and(|v| v == *member.version)
                });
                sides[side].member[at] = found;
                if found.is_some() {
                    members_rows[side] = Some(at);
                }
            }
        }
        let mut pairs = Vec::new();
        if let [Some(at_base), Some(at_head)] = members_rows {
            pairs.push((at_base, at_head));
            on_base.retain(|&at| at != at_base);
            on_head.retain(|&at| at != at_head);
        }
        pairs.extend(paired_by_version(base, head, &on_base, &on_head));
        for (at_base, at_head) in pairs {
            sides[0].counterpart[at_base] = Some(at_head);
            sides[1].counterpart[at_head] = Some(at_base);
            if sides[0].member[at_base].is_some() || sides[1].member[at_head].is_some() {
                sides[0].member[at_base] = member[0];
                sides[1].member[at_head] = member[1];
            }
        }
        for (side, lock) in [(0, base), (1, head)] {
            for at in 0..lock.rows.len() {
                let unpaired = lock.rows[at].source.is_none()
                    && lock.rows[at].name == name
                    && sides[side].counterpart[at].is_none()
                    && sides[side].member[at].is_none();
                if unpaired {
                    sides[side].maybe_member[at] = member[side];
                }
            }
        }
    }
    sides
}

fn paired_by_version(
    base: &Lock,
    head: &Lock,
    on_base: &[usize],
    on_head: &[usize],
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut base_left = Vec::new();
    let mut head_left = on_head.to_vec();
    for &at in on_base {
        let version = &base.rows[at].version;
        match head_left
            .iter()
            .position(|&them| head.rows[them].version == *version)
        {
            Some(pos) => pairs.push((at, head_left.remove(pos))),
            None => base_left.push(at),
        }
    }
    if let ([at_base], [at_head]) = (base_left.as_slice(), head_left.as_slice()) {
        pairs.push((*at_base, *at_head));
    }
    pairs
}

/// A sourceless row that picks another of the versions locked: no sourced row
/// moves when every version stays locked for some other crate. A member reaches
/// the picked crate it ships and walks on from there; an out-of-workspace path
/// crate starts a walk.
fn reselections(lock: &Lock, other: &Lock, side: &Sourceless<'_>, changes: &mut RootChanges) {
    let mut starts = Vec::new();
    for (at, row) in lock.rows.iter().enumerate() {
        if row.source.is_some() {
            continue;
        }
        // An unpaired row compares against what every row of its name picked
        // on the other side; a name new to this side is a crate added, whose
        // directory or manifest already counts.
        let theirs: Vec<usize> = side.counterpart[at].map_or_else(
            || {
                (0..other.rows.len())
                    .filter(|&them| {
                        other.rows[them].source.is_none() && other.rows[them].name == row.name
                    })
                    .collect()
            },
            |theirs| vec![theirs],
        );
        if theirs.is_empty() {
            continue;
        }
        let picked: BTreeSet<String> = theirs
            .into_iter()
            .flat_map(|them| other.child_identities(them))
            .collect();
        let gone: Vec<usize> = row
            .children
            .iter()
            .copied()
            .filter(|&child| !picked.contains(&lock.identity(child)))
            .collect();
        if gone.is_empty() {
            continue;
        }
        // An unpaired row of a member's name may be the member, so both it
        // and a walk from it are asked.
        let mut shipped = false;
        if let Some(member) = side.member[at].or(side.maybe_member[at]) {
            for &child in &gone {
                let child = &lock.rows[child];
                if ships(member, child) {
                    reach(changes, member.id(), RootFile::Lock, child.name.clone());
                    shipped = true;
                }
            }
        }
        if shipped || side.member[at].is_none() {
            starts.push(at);
        }
    }
    walk(lock, side, &starts, changes);
}

/// Up the lock's reverse edges from `starts` to the members that declare the
/// last row passed through as a runtime dependency. A member's row passes the
/// walk on only to members listing it with no runtime workspace graph edge to
/// it (one under a `[patch]` keyed by a registry's name or in
/// `.cargo/config.toml`, which discovery does not read); a member on such an
/// edge is reached through the graph yet still passes the walk on, and a path
/// crate outside the workspace is not walked to.
fn walk(lock: &Lock, side: &Sourceless<'_>, starts: &[usize], changes: &mut RootChanges) {
    if starts.is_empty() {
        return;
    }
    let dependents = lock.dependents();
    let mut seen: BTreeSet<usize> = starts.iter().copied().collect();
    let mut queue: VecDeque<usize> = starts.iter().copied().collect();
    while let Some(at) = queue.pop_front() {
        let row = &lock.rows[at];
        let below = side.member[at];
        for &parent in &dependents[at] {
            let Some(member) = side.member[parent] else {
                if (below.is_none() || lock.rows[parent].source.is_some()) && seen.insert(parent) {
                    queue.push_back(parent);
                }
                continue;
            };
            let on_edge = below.is_some_and(|below| {
                member
                    .package
                    .dependencies()
                    .iter()
                    .any(|edge| edge.on == *below.id() && edge.kind.is_runtime())
            });
            // A member on such an edge is the graph's to reach, but the graph
            // stops at a `[patch]`ed declaration above it, so the walk goes on.
            if on_edge || ships(member, row) {
                if !on_edge {
                    reach(changes, member.id(), RootFile::Lock, row.name.clone());
                }
                if seen.insert(parent) {
                    queue.push_back(parent);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use semver::{Version, VersionReq};

    use super::*;
    use crate::plan::{DeclaredRange, Dependency, DependencyKind, Package, ResolvesDependenciesAt};

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

    /// Each member as `HEAD` declares it, for a base that declared the same.
    fn as_declared(workspace: &Workspace) -> BTreeMap<PackageId, Declarations> {
        cargo_members(workspace)
            .map(|package| {
                (
                    package.id().clone(),
                    Declarations {
                        version: package.version().clone(),
                        external_dependencies: package.external_dependencies().to_vec(),
                        source_unknown: Vec::new(),
                    },
                )
            })
            .collect()
    }

    fn manifest_changes_of(base: &str, head: &str) -> RootChanges {
        root_changes(
            &workspace(),
            &BTreeMap::new(),
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

    /// They reach the root package through its directory.
    #[test]
    fn a_root_packages_own_tables_reach_nothing_here() {
        let base = format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n{MANIFEST}");
        for added in [
            "[dependencies]\nlog = \"0.4\"\n",
            "[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n",
            "[features]\nfast = []\n",
            "[[bin]]\nname = \"app\"\n",
            "[lints.rust]\nunsafe_code = \"forbid\"\n",
            "[badges]\nmaintenance = { status = \"passively-maintained\" }\n",
        ] {
            let changes = manifest_changes_of(&base, &format!("{base}\n{added}"));
            assert!(changes.is_empty(), "{added}: {changes:?}");
        }
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
        let workspace = workspace();
        root_changes(
            &workspace,
            &as_declared(&workspace),
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
            &as_declared(&workspace),
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

    /// Members `cli` and `helper 0.1.0`, which uses `log 0.3.9`; `cli` uses a
    /// path crate outside the workspace, `helper 0.2.0`, which uses `log`.
    fn helper_twice(cli: &str, outer_log: &str) -> String {
        helper_twice_at(cli, "0.1.0", outer_log)
    }

    fn helper_twice_at(cli: &str, helper: &str, outer_log: &str) -> String {
        format!(
            "version = 4\n\n\
             [[package]]\nname = \"cli\"\nversion = \"{cli}\"\ndependencies = [\"helper 0.2.0\"]\n\n\
             [[package]]\nname = \"helper\"\nversion = \"{helper}\"\ndependencies = [\"log 0.3.9\"]\n\n\
             [[package]]\nname = \"helper\"\nversion = \"0.2.0\"\n\
             dependencies = [\"log {outer_log}\"]\n\n\
             [[package]]\nname = \"log\"\nversion = \"0.3.9\"\nsource = \"{CRATES_IO}\"\n\n\
             [[package]]\nname = \"log\"\nversion = \"{outer_log}\"\nsource = \"{CRATES_IO}\"\n"
        )
    }

    fn helper_twice_workspace() -> Workspace {
        helper_twice_workspace_at(&Version::new(0, 1, 0))
    }

    fn helper_twice_workspace_at(helper: &Version) -> Workspace {
        let member = |name: &str| {
            Package::new(
                PackageId::new(Ecosystem::Cargo, name),
                if name == "helper" {
                    helper.clone()
                } else {
                    Version::new(0, 1, 0)
                },
                ResolvesDependenciesAt::Install,
                false,
                Vec::new(),
            )
        };
        Workspace::new([
            member("cli").with_external_dependencies(vec![ExternalDependency {
                source: None,
                req: VersionReq::parse("0.2").expect("req"),
                ..external("helper", DependencyKind::Normal, false)
            }]),
            member("helper").with_external_dependencies(vec![ExternalDependency {
                req: VersionReq::parse("0.3").expect("req"),
                ..external("log", DependencyKind::Normal, false)
            }]),
        ])
        .expect("workspace")
    }

    #[test]
    fn a_path_crate_named_like_a_member_is_walked_through() {
        let changes = lock_changes_in(
            &helper_twice_workspace(),
            &helper_twice("0.1.0", "0.4.20"),
            &helper_twice("0.1.0", "0.4.21"),
        );
        assert_eq!(
            reasons(&changes, &PackageId::new(Ecosystem::Cargo, "cli")),
            ["Cargo.lock: helper"]
        );
        assert_eq!(changes.members.len(), 1, "{changes:?}");
    }

    /// `version` moving `cli`'s row leaves both `helper` rows as they were.
    #[test]
    fn same_named_sourceless_rows_pair_by_version() {
        let changes = lock_changes_in(
            &helper_twice_workspace(),
            &helper_twice("0.1.0", "0.4.21"),
            &helper_twice("0.2.0", "0.4.21"),
        );
        assert!(changes.is_empty(), "{changes:?}");
        let changes = lock_changes_in(
            &helper_twice_workspace_at(&Version::new(0, 1, 1)),
            &helper_twice_at("0.1.0", "0.1.0", "0.4.21"),
            &helper_twice_at("0.1.0", "0.1.1", "0.4.21"),
        );
        assert!(changes.is_empty(), "{changes:?}");
        let switched = replaced_once(
            &helper_twice_at("0.1.0", "0.1.1", "0.4.21"),
            "dependencies = [\"log 0.3.9\"]",
            "dependencies = [\"log 0.4.21\"]",
        );
        let changes = lock_changes_in(
            &helper_twice_workspace_at(&Version::new(0, 1, 1)),
            &helper_twice_at("0.1.0", "0.1.0", "0.4.21"),
            &switched,
        );
        assert_eq!(
            reasons(&changes, &PackageId::new(Ecosystem::Cargo, "helper")),
            ["Cargo.lock: log"]
        );
    }

    /// Both `helper` rows move and neither changes what it picks.
    #[test]
    fn same_named_rows_both_moving_with_the_same_picks_ask_nothing() {
        let lock = |cli_helper: &str, member: &str, outer: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"cli\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"helper {cli_helper}\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{member}\"\n\
                 dependencies = [\"log 0.3.9\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{outer}\"\n\
                 dependencies = [\"log 0.4.21\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.3.9\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.4.21\"\nsource = \"{CRATES_IO}\"\n"
            )
        };
        let changes = lock_changes_in(
            &helper_twice_workspace_at(&Version::new(0, 1, 1)),
            &lock("0.2.0", "0.1.0", "0.2.0"),
            &lock("0.2.1", "0.1.1", "0.2.1"),
        );
        assert!(changes.is_empty(), "{changes:?}");
    }

    /// Both `helper` rows move, so neither pairs; the member still picked
    /// another locked `log`.
    #[test]
    fn a_member_left_unpaired_still_compares_what_it_picked() {
        let lock = |cli_helper: &str, member: &str, member_log: &str, outer: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"cli\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"helper {cli_helper}\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{member}\"\n\
                 dependencies = [\"log {member_log}\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{outer}\"\n\
                 dependencies = [\"log 0.4.21\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.3.9\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.4.21\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"other\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\
                 dependencies = [\"log 0.3.9\"]\n"
            )
        };
        let changes = lock_changes_in(
            &helper_twice_workspace_at(&Version::new(0, 1, 1)),
            &lock("0.2.0", "0.1.0", "0.3.9", "0.2.0"),
            &lock("0.2.1", "0.1.1", "0.4.21", "0.2.1"),
        );
        assert_eq!(
            reasons(&changes, &PackageId::new(Ecosystem::Cargo, "helper")),
            ["Cargo.lock: log"]
        );
    }

    /// The member's old row is the member's: a walk up from it stops there
    /// rather than passing through to `wrap` and on to `cli`.
    #[test]
    fn a_member_row_paired_by_version_ends_the_walk_on_both_sides() {
        let lock = |member: &str, member_log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"cli\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"helper 0.2.0\", \"wrap\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{member}\"\n\
                 dependencies = [\"log {member_log}\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"0.2.0\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.3.9\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.4.21\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"other\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\
                 dependencies = [\"log 0.3.9\", \"log 0.4.21\"]\n\n\
                 [[package]]\nname = \"wrap\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"helper {member}\"]\n"
            )
        };
        let mut workspace = helper_twice_workspace_at(&Version::new(0, 1, 1));
        let cli = PackageId::new(Ecosystem::Cargo, "cli");
        workspace = Workspace::new(workspace.packages().map(|package| {
            if package.id() == &cli {
                package.clone().with_external_dependencies(vec![
                    ExternalDependency {
                        source: None,
                        ..external("helper", DependencyKind::Normal, false)
                    },
                    ExternalDependency {
                        source: None,
                        ..external("wrap", DependencyKind::Normal, false)
                    },
                ])
            } else {
                package.clone()
            }
        }))
        .expect("workspace");
        let changes = lock_changes_in(
            &workspace,
            &lock("0.1.0", "0.3.9"),
            &lock("0.1.1", "0.4.21"),
        );
        assert!(reasons(&changes, &cli).is_empty(), "{changes:?}");
    }

    fn replaced_once(text: &str, from: &str, to: &str) -> String {
        assert!(text.contains(from), "{from:?} is not in:\n{text}");
        text.replacen(from, to, 1)
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
        lock_changes_across(workspace, &as_declared(workspace), base, head)
    }

    fn lock_changes_across(
        workspace: &Workspace,
        base_members: &BTreeMap<PackageId, Declarations>,
        base: &str,
        head: &str,
    ) -> RootChanges {
        root_changes(
            workspace,
            base_members,
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

    fn app() -> PackageId {
        PackageId::new(Ecosystem::Cargo, "app")
    }

    fn helper() -> PackageId {
        PackageId::new(Ecosystem::Cargo, "helper")
    }

    /// `app` declares `helper = "0.1"` from a registry, `[patch]`ed to the
    /// member `helper` with no graph edge, as from a named registry; `helper`
    /// declares `log` and `wrap` as `helper_kind`.
    fn patched_helper_workspace(
        app_also: Vec<Dependency>,
        helper_kind: DependencyKind,
    ) -> Workspace {
        let member = |id: PackageId, dependencies: Vec<Dependency>| {
            Package::new(
                id,
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                false,
                dependencies,
            )
        };
        Workspace::new([
            member(app(), app_also).with_external_dependencies(vec![ExternalDependency {
                req: VersionReq::parse("0.1").expect("req"),
                ..external("helper", DependencyKind::Normal, false)
            }]),
            member(helper(), Vec::new()).with_external_dependencies(vec![
                external("log", helper_kind, false),
                external("wrap", helper_kind, false),
            ]),
        ])
        .expect("workspace")
    }

    /// As `cargo generate-lockfile` writes it for [`patched_helper_workspace`].
    fn patched_helper_lock(log: &str) -> String {
        format!(
            "version = 4\n\n\
             [[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n\n\
             [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\ndependencies = [\"log\"]\n\n\
             [[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"{CRATES_IO}\"\n"
        )
    }

    /// `cargo metadata --no-deps` reports `app`'s declaration from the registry,
    /// so no workspace graph edge carries `helper`'s change to it.
    #[test]
    fn a_member_patched_in_for_a_registry_declaration_carries_a_lock_change_on() {
        let changes = lock_changes_in(
            &patched_helper_workspace(Vec::new(), DependencyKind::Normal),
            &patched_helper_lock("0.4.34"),
            &patched_helper_lock("0.4.35"),
        );
        assert_eq!(reasons(&changes, &helper()), ["Cargo.lock: log"]);
        assert_eq!(reasons(&changes, &app()), ["Cargo.lock: helper"]);
    }

    /// `[patch.crates-io]` puts the member under a registry crate, which
    /// carries the walk on to the member that declares it.
    #[test]
    fn a_registry_crate_above_a_patched_member_carries_the_walk_on() {
        let lock = |log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"wrapper\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\ndependencies = [\"log\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"wrapper\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\
                 dependencies = [\"helper\"]\n"
            )
        };
        let member = |id: PackageId, external: Vec<ExternalDependency>| {
            Package::new(
                id,
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                false,
                Vec::new(),
            )
            .with_external_dependencies(external)
        };
        let workspace = Workspace::new([
            member(
                app(),
                vec![external("wrapper", DependencyKind::Normal, false)],
            ),
            member(
                helper(),
                vec![external("log", DependencyKind::Normal, false)],
            ),
        ])
        .expect("workspace");
        let changes = lock_changes_in(&workspace, &lock("0.4.34"), &lock("0.4.35"));
        assert_eq!(reasons(&changes, &helper()), ["Cargo.lock: log"]);
        assert_eq!(reasons(&changes, &app()), ["Cargo.lock: wrapper"]);
    }

    /// `helper`'s own row lists the same `wrap` on both sides.
    #[test]
    fn a_patched_member_carries_a_change_below_its_dependency_on() {
        let lock = |log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\ndependencies = [\"wrap\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"wrap\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\
                 dependencies = [\"log\"]\n"
            )
        };
        let changes = lock_changes_in(
            &patched_helper_workspace(Vec::new(), DependencyKind::Normal),
            &lock("0.4.34"),
            &lock("0.4.35"),
        );
        assert_eq!(reasons(&changes, &helper()), ["Cargo.lock: wrap"]);
        assert_eq!(reasons(&changes, &app()), ["Cargo.lock: helper"]);
    }

    #[test]
    fn a_patched_member_picking_another_locked_version_carries_it_on() {
        let lock = |picked: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"log {picked}\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.3.9\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.4.21\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"other\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\
                 dependencies = [\"log 0.3.9\", \"log 0.4.21\"]\n"
            )
        };
        let changes = lock_changes_in(
            &patched_helper_workspace(Vec::new(), DependencyKind::Normal),
            &lock("0.3.9"),
            &lock("0.4.21"),
        );
        assert_eq!(reasons(&changes, &helper()), ["Cargo.lock: log"]);
        assert_eq!(reasons(&changes, &app()), ["Cargo.lock: helper"]);
    }

    #[test]
    fn a_patched_member_that_ships_nothing_carries_nothing_on() {
        let changes = lock_changes_in(
            &patched_helper_workspace(Vec::new(), DependencyKind::Development),
            &patched_helper_lock("0.4.34"),
            &patched_helper_lock("0.4.35"),
        );
        assert!(changes.is_empty(), "{changes:?}");
    }

    /// `middle` reaches `helper`'s change through the graph, which stops at
    /// `app`'s registry declaration of `middle`; the walk carries it on.
    #[test]
    fn a_walk_crosses_a_path_edge_to_a_patched_member_above_it() {
        let lock = |log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"middle\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\ndependencies = [\"log\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"middle\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n"
            )
        };
        let middle = PackageId::new(Ecosystem::Cargo, "middle");
        let member = |id: PackageId, edges: Vec<Dependency>, external: Vec<ExternalDependency>| {
            Package::new(
                id,
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                false,
                edges,
            )
            .with_external_dependencies(external)
        };
        let workspace = Workspace::new([
            member(
                app(),
                Vec::new(),
                vec![external("middle", DependencyKind::Normal, false)],
            ),
            member(
                middle.clone(),
                vec![Dependency {
                    on: helper(),
                    kind: DependencyKind::Normal,
                    declared_as: String::from("helper"),
                    target: None,
                    range: DeclaredRange::PathLinked,
                }],
                Vec::new(),
            ),
            member(
                helper(),
                Vec::new(),
                vec![external("log", DependencyKind::Normal, false)],
            ),
        ])
        .expect("workspace");
        let changes = lock_changes_in(&workspace, &lock("0.4.34"), &lock("0.4.35"));
        assert_eq!(reasons(&changes, &helper()), ["Cargo.lock: log"]);
        assert!(reasons(&changes, &middle).is_empty(), "{changes:?}");
        assert_eq!(reasons(&changes, &app()), ["Cargo.lock: middle"]);
    }

    /// The workspace graph carries a member's change along a path edge.
    #[test]
    fn a_member_on_a_path_edge_is_not_reached_through_the_lock_too() {
        let path_edge = Dependency {
            on: helper(),
            kind: DependencyKind::Normal,
            declared_as: String::from("helper-path"),
            target: None,
            range: DeclaredRange::PathLinked,
        };
        let changes = lock_changes_in(
            &patched_helper_workspace(vec![path_edge], DependencyKind::Normal),
            &patched_helper_lock("0.4.34"),
            &patched_helper_lock("0.4.35"),
        );
        assert_eq!(reasons(&changes, &helper()), ["Cargo.lock: log"]);
        assert_eq!(changes.members.len(), 1, "{changes:?}");
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

    /// A runtime git `rand` `[patch]`ed to a path crate shares the path
    /// source with a dev alias at another version; neither matches both.
    #[test]
    fn a_dev_declaration_matching_only_the_source_hides_no_runtime_one() {
        let lock = |log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"0.8.6\"\ndependencies = [\"log\"]\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"0.9.4\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"rand 0.8.6\", \"rand 0.9.4\"]\n"
            )
        };
        let workspace = store_declaring(vec![
            ExternalDependency {
                source: Some(String::from("git+https://example.invalid/rand.git")),
                ..external("rand", DependencyKind::Normal, false)
            },
            ExternalDependency {
                declared_as: String::from("rand09"),
                source: None,
                req: VersionReq::parse("0.9").expect("req"),
                ..external("rand", DependencyKind::Development, false)
            },
        ]);
        let changes = lock_changes_in(&workspace, &lock("0.4.34"), &lock("0.4.35"));
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: rand"]);
    }

    /// `[patch]` met only the dev alias's requirement, so the path row is the
    /// dev `rand`; the version, not the source the patch rewrote, says so.
    #[test]
    fn a_patched_dev_alias_is_told_apart_by_its_version() {
        let lock = |log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"0.8.5\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"0.9.4\"\ndependencies = [\"log\"]\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"rand 0.8.5\", \"rand 0.9.4\"]\n"
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
        let changes = lock_changes_in(&workspace, &lock("0.4.34"), &lock("0.4.35"));
        assert!(changes.is_empty(), "{changes:?}");
    }

    /// The removed `0.9.4` row matches no requirement `HEAD` declares, so
    /// read against `HEAD` it would fall back to the runtime `rand` too.
    #[test]
    fn a_base_row_is_read_against_the_base_declarations() {
        let lock = |alias: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"0.8.5\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"rand\"\nversion = \"{alias}\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"rand 0.8.5\", \"rand {alias}\"]\n"
            )
        };
        let alias = |req: &str| ExternalDependency {
            declared_as: String::from("rand_alias"),
            req: VersionReq::parse(req).expect("req"),
            ..external("rand", DependencyKind::Development, true)
        };
        let runtime = ExternalDependency {
            req: VersionReq::parse("0.8").expect("req"),
            ..external("rand", DependencyKind::Normal, false)
        };
        let workspace = store_declaring(vec![runtime.clone(), alias("0.10")]);
        let at_base = BTreeMap::from([(
            store(),
            Declarations {
                version: Version::new(0, 1, 0),
                external_dependencies: vec![runtime, alias("0.9")],
                source_unknown: Vec::new(),
            },
        )]);
        let (base, head) = (lock("0.9.4"), lock("0.10.0"));
        let changes = lock_changes_across(&workspace, &at_base, &base, &head);
        assert!(changes.is_empty(), "{changes:?}");
        let changes = lock_changes_in(&workspace, &base, &head);
        assert_eq!(reasons(&changes, &store()), ["Cargo.lock: rand"]);
    }

    /// `store` ships `shared` from crates.io and tests with `shared` from
    /// `other`, both `*`; only `other`'s row moves. At the base one of the two
    /// is from a registry no config named.
    #[test]
    fn a_declaration_of_unknown_source_neither_matches_nor_rules_out_a_row() {
        const OTHER: &str = "sparse+https://other.invalid/index/";
        let lock = |dev: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"shared\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"shared\"\nversion = \"{dev}\"\nsource = \"{OTHER}\"\n\n\
                 [[package]]\nname = \"store\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"shared 1.0.0\", \"shared {dev}\"]\n"
            )
        };
        let runtime = external("shared", DependencyKind::Normal, false);
        let dev = ExternalDependency {
            declared_as: String::from("shared_dev"),
            source: Some(OTHER.to_owned()),
            ..external("shared", DependencyKind::Development, false)
        };
        let workspace = store_declaring(vec![runtime.clone(), dev.clone()]);
        let (base, head) = (lock("1.1.0"), lock("1.2.0"));
        let at_base = |known: Vec<ExternalDependency>, unknown: Vec<ExternalDependency>| {
            BTreeMap::from([(
                store(),
                Declarations {
                    version: Version::new(0, 1, 0),
                    external_dependencies: known,
                    source_unknown: unknown,
                },
            )])
        };
        let known = at_base(vec![runtime.clone(), dev.clone()], Vec::new());
        let changes = lock_changes_across(&workspace, &known, &base, &head);
        assert!(changes.is_empty(), "{changes:?}");
        let dev_elsewhere = ExternalDependency {
            source: Some(CRATES_IO.to_owned()),
            ..dev.clone()
        };
        for (known, unknown) in [
            (runtime.clone(), dev.clone()),
            (dev, runtime.clone()),
            (dev_elsewhere, runtime),
        ] {
            let changes = lock_changes_across(
                &workspace,
                &at_base(vec![known.clone()], vec![unknown.clone()]),
                &base,
                &head,
            );
            assert_eq!(
                reasons(&changes, &store()),
                ["Cargo.lock: shared"],
                "{known:?} beside {unknown:?}"
            );
        }
    }

    /// The member `helper` and the path crate `cli` uses both move version and
    /// swap their picks of `log`; the member's base version pairs its rows.
    #[test]
    fn a_member_and_a_same_named_path_crate_swapping_picks_reach_the_path_crates_user() {
        let lock = |member: &str, member_log: &str, outside: &str, outside_log: &str| {
            format!(
                "version = 4\n\n\
                 [[package]]\nname = \"cli\"\nversion = \"0.1.0\"\ndependencies = [\"helper {outside}\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{member}\"\ndependencies = [\"log {member_log}\"]\n\n\
                 [[package]]\nname = \"helper\"\nversion = \"{outside}\"\ndependencies = [\"log {outside_log}\"]\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.3.9\"\nsource = \"{CRATES_IO}\"\n\n\
                 [[package]]\nname = \"log\"\nversion = \"0.4.21\"\nsource = \"{CRATES_IO}\"\n"
            )
        };
        let base = lock("0.1.0", "0.3.9", "0.5.0", "0.4.21");
        let head = lock("0.2.0", "0.4.21", "0.6.0", "0.3.9");
        let workspace = helper_twice_workspace_at(&Version::new(0, 2, 0));
        let mut at_base = as_declared(&workspace);
        let helper = PackageId::new(Ecosystem::Cargo, "helper");
        at_base.get_mut(&helper).expect("helper").version = Version::new(0, 1, 0);
        let changes = lock_changes_across(&workspace, &at_base, &base, &head);
        assert_eq!(
            reasons(&changes, &PackageId::new(Ecosystem::Cargo, "cli")),
            ["Cargo.lock: helper"]
        );
        let changes = lock_changes_in(&workspace, &base, &head);
        assert!(
            reasons(&changes, &PackageId::new(Ecosystem::Cargo, "cli")).is_empty(),
            "{changes:?}"
        );
    }

    #[test]
    fn an_unreadable_side_is_an_error_not_an_empty_lock() {
        let err = lock_changes_of(Some("[[package]\nname = "), Some(LOCK)).expect_err("garbled");
        assert_eq!(err.file, RootFile::Lock);
        let err = lock_changes_of(Some("package = 1\n"), Some(LOCK)).expect_err("not rows");
        assert_eq!(err.file, RootFile::Lock);
    }
}
