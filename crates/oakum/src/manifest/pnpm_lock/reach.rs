//! Which workspace projects a `pnpm-lock.yaml` change reaches (ADR-0039): an
//! entry added, removed or resolved differently, followed up the lockfile's
//! dependency edges to the projects whose `dependencies` or
//! `optionalDependencies` lead to it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Deserialize;

use super::read_model;

#[derive(Default, Deserialize)]
struct Lock {
    #[serde(default)]
    importers: BTreeMap<String, Importer>,
    #[serde(default)]
    packages: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    snapshots: BTreeMap<String, Snapshot>,
}

#[derive(Default, Deserialize)]
struct Importer {
    #[serde(default)]
    dependencies: BTreeMap<String, Resolved>,
    #[serde(rename = "optionalDependencies", default)]
    optional_dependencies: BTreeMap<String, Resolved>,
}

#[derive(Deserialize)]
struct Resolved {
    version: String,
}

#[derive(Default, Deserialize)]
struct Snapshot {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(rename = "optionalDependencies", default)]
    optional_dependencies: BTreeMap<String, String>,
}

/// Each workspace project, by its importer path in the lockfile, whose
/// runtime dependencies lead to an entry that changed between `base` and
/// `head`, with the direct dependencies that lead there. A linked workspace
/// project (`link:`) or a local directory (`file:`) is no entry: `version`
/// rewrites the first, and the second's content is not in the lockfile.
///
/// # Errors
///
/// A side that is not a lockfile oakum has measured, named by `base` or
/// `head`.
pub fn pnpm_lock_reach(
    base: Option<&str>,
    head: Option<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, (&'static str, String)> {
    let base_lock = parse(base).map_err(|err| ("base", err))?;
    let head_lock = parse(head).map_err(|err| ("head", err))?;
    let changed: BTreeSet<&str> = base_lock
        .snapshots
        .keys()
        .chain(head_lock.snapshots.keys())
        .filter(|key| !same_snapshot(&base_lock, &head_lock, key))
        .map(String::as_str)
        .collect();
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for lock in [&base_lock, &head_lock] {
        for (importer, names) in walk(lock, &changed) {
            out.entry(importer).or_default().extend(names);
        }
    }
    for (path, importer) in &head_lock.importers {
        let earlier = base_lock.importers.get(path);
        for (name, resolved) in importer.runtime() {
            let before = earlier.and_then(|earlier| earlier.runtime_version(name));
            if entry_key(name, &resolved.version).is_some()
                && before != Some(resolved.version.as_str())
            {
                out.entry(path.clone()).or_default().insert(name.clone());
            }
        }
    }
    Ok(out)
}

fn parse(text: Option<&str>) -> Result<Lock, String> {
    let Some(text) = text else {
        return Ok(Lock::default());
    };
    let (_, body, _) = read_model(text)?;
    serde_saphyr::from_str(body).map_err(|err| format!("the lockfile could not be read: {err}"))
}

fn same_snapshot(base: &Lock, head: &Lock, key: &str) -> bool {
    base.snapshots.get(key).map(Snapshot::edges) == head.snapshots.get(key).map(Snapshot::edges)
        && installed_as(base, key) == installed_as(head, key)
}

/// A package entry without `deprecated`, the one field the registry changes
/// after publish, and with an empty `bundledDependencies`, which newer pnpm
/// writes, read as absent; every other field (`hasBin`, `libc`, …) changes what
/// installs, even when a pnpm upgrade is what rewrote it.
fn installed_as(lock: &Lock, key: &str) -> Option<serde_json::Value> {
    let mut entry = lock.packages.get(package_key(key))?.clone();
    if let Some(fields) = entry.as_object_mut() {
        fields.remove("deprecated");
        if fields
            .get("bundledDependencies")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            fields.remove("bundledDependencies");
        }
    }
    Some(entry)
}

/// `name@version(peers…)` without its peer suffix, as `packages` keys it.
fn package_key(key: &str) -> &str {
    key.split_once('(').map_or(key, |(package, _)| package)
}

/// The snapshot key a dependency resolves to: `name@version`, or the
/// version itself when an alias already names the package
/// (`real@1.0.0`). `None` for a link or a local directory.
fn entry_key(name: &str, version: &str) -> Option<String> {
    if version.starts_with("link:") || version.starts_with("file:") {
        return None;
    }
    let bare = package_key(version);
    if bare.char_indices().any(|(at, c)| at > 0 && c == '@') {
        Some(version.to_owned())
    } else {
        Some(format!("{name}@{version}"))
    }
}

impl Importer {
    fn runtime(&self) -> impl Iterator<Item = (&String, &Resolved)> {
        self.dependencies.iter().chain(&self.optional_dependencies)
    }

    fn runtime_version(&self, name: &str) -> Option<&str> {
        self.optional_dependencies
            .get(name)
            .or_else(|| self.dependencies.get(name))
            .map(|resolved| resolved.version.as_str())
    }
}

impl Snapshot {
    fn edges(&self) -> BTreeSet<(&str, &str)> {
        self.dependencies
            .iter()
            .chain(&self.optional_dependencies)
            .map(|(name, version)| (name.as_str(), version.as_str()))
            .collect()
    }
}

/// Up the lockfile's edges from each changed entry this side holds, to the
/// importers whose runtime dependencies lead there, each with the direct
/// dependency it came through.
fn walk(lock: &Lock, changed: &BTreeSet<&str>) -> BTreeMap<String, BTreeSet<String>> {
    let mut dependents: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (key, snapshot) in &lock.snapshots {
        for (name, version) in snapshot.edges() {
            if let Some(child) = entry_key(name, version) {
                dependents.entry(child).or_default().push(key);
            }
        }
    }
    let mut importers: BTreeMap<String, Vec<(&str, &str)>> = BTreeMap::new();
    for (path, importer) in &lock.importers {
        for (name, resolved) in importer.runtime() {
            if let Some(child) = entry_key(name, &resolved.version) {
                importers.entry(child).or_default().push((path, name));
            }
        }
    }
    let mut seen: BTreeSet<String> = changed
        .iter()
        .filter(|key| lock.snapshots.contains_key(**key))
        .map(|key| (*key).to_owned())
        .collect();
    let mut queue: VecDeque<String> = seen.iter().cloned().collect();
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    while let Some(key) = queue.pop_front() {
        for (path, name) in importers.get(&key).into_iter().flatten() {
            out.entry((*path).to_owned())
                .or_default()
                .insert((*name).to_owned());
        }
        for parent in dependents.get(&key).into_iter().flatten() {
            if seen.insert((*parent).to_owned()) {
                queue.push_back((*parent).to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .: {}

  packages/app:
    dependencies:
      is-odd:
        specifier: ^3.0.0
        version: 3.0.1
      lib:
        specifier: workspace:^
        version: link:../lib

  packages/tool:
    devDependencies:
      is-even:
        specifier: 1.0.0
        version: 1.0.0

packages:

  is-even@1.0.0:
    resolution: {integrity: sha512-even}

  is-number@6.0.0:
    resolution: {integrity: sha512-six}

  is-odd@3.0.1:
    resolution: {integrity: sha512-odd}

snapshots:

  is-even@1.0.0:
    dependencies:
      is-odd: 0.1.2

  is-number@6.0.0: {}

  is-odd@3.0.1:
    dependencies:
      is-number: 6.0.0
";

    fn reach(base: &str, head: &str) -> BTreeMap<String, BTreeSet<String>> {
        pnpm_lock_reach(Some(base), Some(head)).expect("parses")
    }

    fn reached(path: &str, names: &[&str]) -> (String, BTreeSet<String>) {
        (
            path.to_owned(),
            names.iter().map(|name| (*name).to_owned()).collect(),
        )
    }

    #[test]
    fn a_transitive_entry_reaches_the_importer_that_ships_it() {
        let head = LOCK.replace("sha512-six", "sha512-six-again");
        assert_eq!(
            reach(LOCK, &head),
            BTreeMap::from([reached("packages/app", &["is-odd"])])
        );
    }

    #[test]
    fn an_entry_only_a_dev_dependency_leads_to_reaches_nothing() {
        let head = LOCK.replace("sha512-even", "sha512-even-again");
        assert_eq!(reach(LOCK, &head), BTreeMap::new());
    }

    #[test]
    fn a_deprecation_reaches_nothing_but_a_field_pnpm_installs_by_does() {
        let field = |line: &str| {
            LOCK.replace(
                "    resolution: {integrity: sha512-six}\n",
                &format!("    resolution: {{integrity: sha512-six}}\n    {line}\n"),
            )
        };
        for inert in ["deprecated: use something else", "bundledDependencies: []"] {
            assert_eq!(reach(LOCK, &field(inert)), BTreeMap::new(), "{inert}");
        }
        assert_eq!(
            reach(LOCK, &field("hasBin: true")),
            BTreeMap::from([reached("packages/app", &["is-odd"])])
        );
    }

    #[test]
    fn a_direct_dependency_resolved_anew_reaches_its_importer() {
        let head = LOCK
            .replace("        version: 3.0.1", "        version: 3.0.2")
            .replace(
                "  is-odd@3.0.1:\n    resolution",
                "  is-odd@3.0.2:\n    resolution",
            )
            .replace(
                "  is-odd@3.0.1:\n    dependencies",
                "  is-odd@3.0.2:\n    dependencies",
            );
        assert_eq!(
            reach(LOCK, &head),
            BTreeMap::from([reached("packages/app", &["is-odd"])])
        );
    }

    #[test]
    fn a_linked_member_and_an_unchanged_lock_reach_nothing() {
        let head = LOCK.replace("specifier: workspace:^", "specifier: workspace:~");
        assert_eq!(reach(LOCK, &head), BTreeMap::new());
        assert_eq!(reach(LOCK, LOCK), BTreeMap::new());
    }

    #[test]
    fn an_alias_resolves_to_the_entry_it_names() {
        assert_eq!(
            entry_key("pad", "left-pad@1.3.0"),
            Some(String::from("left-pad@1.3.0"))
        );
        assert_eq!(
            entry_key("react-dom", "18.2.0(react@18.2.0)"),
            Some(String::from("react-dom@18.2.0(react@18.2.0)"))
        );
        assert_eq!(
            entry_key("x", "@scope/real@1.0.0"),
            Some(String::from("@scope/real@1.0.0"))
        );
        assert_eq!(entry_key("lib", "link:../lib"), None);
    }

    #[test]
    fn an_unmeasured_side_is_named() {
        let (side, _) =
            pnpm_lock_reach(Some(LOCK), Some("lockfileVersion: '6.0'\n")).expect_err("unmeasured");
        assert_eq!(side, "head");
    }
}
