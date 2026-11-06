//! Where a Cargo dependency entry lives in a manifest, and what it says about
//! its version: one reading for discovery and the writers (ADR-0026).

use toml_edit::{DocumentMut, Item};

use crate::plan::{DependencyKind, Ecosystem};

/// Which step of the walk to a member's dependency entry found nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryMiss {
    KindHasNoSection,
    TargetTable,
    TargetPredicate,
    SectionTable,
    Key,
}

/// Which step of the walk to a `[workspace.dependencies]` entry found nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceMiss {
    WorkspaceTable,
    DependenciesTable,
    Key,
}

/// The entry for `declared_as` under `kind`'s section, or under
/// `[target.<target>]` when the edge carries one.
pub(crate) fn dependency_entry<'d>(
    doc: &'d mut DocumentMut,
    kind: DependencyKind,
    target: Option<&str>,
    declared_as: &str,
) -> Result<&'d mut Item, EntryMiss> {
    let section = kind
        .section(Ecosystem::Cargo)
        .ok_or(EntryMiss::KindHasNoSection)?;
    let root = doc.as_item_mut();
    let deps = match target {
        Some(predicate) => {
            let targets = table_like_child(root, "target").ok_or(EntryMiss::TargetTable)?;
            let table = table_like_child(targets, predicate).ok_or(EntryMiss::TargetPredicate)?;
            section_table(table, section)
        }
        None => section_table(root, section),
    }
    .ok_or(EntryMiss::SectionTable)?;
    table_child(deps, declared_as).ok_or(EntryMiss::Key)
}

/// The `[workspace.dependencies]` entry for `declared_as`.
pub(crate) fn workspace_entry<'d>(
    doc: &'d mut DocumentMut,
    declared_as: &str,
) -> Result<&'d mut Item, WorkspaceMiss> {
    let workspace =
        table_like_child(doc.as_item_mut(), "workspace").ok_or(WorkspaceMiss::WorkspaceTable)?;
    let deps =
        table_like_child(workspace, "dependencies").ok_or(WorkspaceMiss::DependenciesTable)?;
    table_child(deps, declared_as).ok_or(WorkspaceMiss::Key)
}

/// What a member's entry says about its version, as discovery reads it to
/// tell a path dependency's authored `*` from no version at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberVersion {
    /// A version string, a `version` key, or `version.workspace = true`.
    Declared,
    /// `workspace = true` with no `version` key: the root entry decides.
    Inherited,
    /// A table with neither, such as path-only.
    Absent,
    /// Neither a string nor a table.
    Unexpected,
}

pub(crate) fn member_version(item: &Item) -> MemberVersion {
    if item.as_str().is_some() {
        return MemberVersion::Declared;
    }
    if item.as_table().is_none() && item.as_inline_table().is_none() {
        return MemberVersion::Unexpected;
    }
    if item.get("version").is_some() {
        MemberVersion::Declared
    } else if item_workspace_true(item) {
        MemberVersion::Inherited
    } else {
        MemberVersion::Absent
    }
}

/// Whether a `[workspace.dependencies]` entry declares a version; `None` for
/// an entry that is neither a string nor a table.
pub(crate) fn workspace_entry_declares_version(item: &Item) -> Option<bool> {
    if item.as_str().is_some() {
        return Some(true);
    }
    if item.as_table().is_none() && item.as_inline_table().is_none() {
        return None;
    }
    Some(item.get("version").is_some())
}

/// `Table::get_mut` does not insert. `Item::get_mut` does (`Item::None`), so
/// inline tables are opened only after `get` shows the key.
pub(crate) fn table_child<'a>(parent: &'a mut Item, key: &str) -> Option<&'a mut Item> {
    if parent.as_table().is_some() {
        return parent.as_table_mut()?.get_mut(key);
    }
    if parent.as_inline_table().is_none() || parent.get(key).is_none() {
        return None;
    }
    parent.get_mut(key)
}

/// A child that is itself a table, standard or inline.
fn table_like_child<'a>(parent: &'a mut Item, key: &str) -> Option<&'a mut Item> {
    table_child(parent, key).filter(|child| child.is_table() || child.is_inline_table())
}

/// Cargo still reads `dev_dependencies` and `build_dependencies`, deprecated
/// since edition 2021, so the hyphenated section falls back to them.
fn section_table<'a>(parent: &'a mut Item, section: &str) -> Option<&'a mut Item> {
    let hyphenated = parent
        .get(section)
        .is_some_and(|child| child.is_table() || child.is_inline_table());
    if hyphenated {
        table_like_child(parent, section)
    } else {
        table_like_child(parent, &section.replace('-', "_"))
    }
}

pub(crate) fn item_workspace_true(item: &Item) -> bool {
    if let Some(table) = item.as_table() {
        return table.get("workspace").and_then(Item::as_bool) == Some(true);
    }
    if let Some(table) = item.as_inline_table() {
        return table.get("workspace").and_then(toml_edit::Value::as_bool) == Some(true);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        text: &str,
        kind: DependencyKind,
        target: Option<&str>,
    ) -> Result<MemberVersion, EntryMiss> {
        let mut doc: DocumentMut = text.parse().expect("toml");
        dependency_entry(&mut doc, kind, target, "core").map(|item| member_version(item))
    }

    #[test]
    fn each_entry_shape_reads_as_its_version_kind() {
        let normal = DependencyKind::Normal;
        for (text, read) in [
            ("[dependencies]\ncore = \"1\"\n", MemberVersion::Declared),
            (
                "[dependencies]\ncore = { path = \"x\", version = \"*\" }\n",
                MemberVersion::Declared,
            ),
            (
                "[dependencies]\ncore = { path = \"x\" }\n",
                MemberVersion::Absent,
            ),
            ("[dependencies.core]\npath = \"x\"\n", MemberVersion::Absent),
            (
                "[dependencies]\ncore = { workspace = true }\n",
                MemberVersion::Inherited,
            ),
            (
                "[dependencies]\ncore = { version.workspace = true }\n",
                MemberVersion::Declared,
            ),
            ("[dependencies]\ncore = 1\n", MemberVersion::Unexpected),
            (
                "dependencies = { core = { path = \"x\" } }\n",
                MemberVersion::Absent,
            ),
        ] {
            assert_eq!(entry(text, normal, None), Ok(read), "{text}");
        }
    }

    #[test]
    fn each_missing_step_is_named() {
        let normal = DependencyKind::Normal;
        let unix = Some("cfg(unix)");
        for (text, kind, target, miss) in [
            ("", DependencyKind::Peer, None, EntryMiss::KindHasNoSection),
            ("[dependencies]\n", normal, unix, EntryMiss::TargetTable),
            (
                "[target.other.dependencies]\n",
                normal,
                unix,
                EntryMiss::TargetPredicate,
            ),
            (
                "[target.'cfg(unix)'.dev-dependencies]\n",
                normal,
                unix,
                EntryMiss::SectionTable,
            ),
            (
                "[dev-dependencies]\ncore = \"1\"\n",
                normal,
                None,
                EntryMiss::SectionTable,
            ),
            (
                "[dependencies]\nother = \"1\"\n",
                normal,
                None,
                EntryMiss::Key,
            ),
        ] {
            assert_eq!(entry(text, kind, target), Err(miss), "{text}");
        }
        assert_eq!(
            entry(
                "[target.'cfg(unix)'.dependencies]\ncore = { path = \"x\" }\n",
                normal,
                unix
            ),
            Ok(MemberVersion::Absent)
        );
    }

    /// Cargo still reads the underscore spellings, deprecated since edition
    /// 2021, and `cargo metadata` lists their edges as `dev` and `build`.
    #[test]
    fn an_underscore_section_is_the_same_section() {
        let unix = Some("cfg(unix)");
        for (text, kind, target) in [
            (
                "[dev_dependencies]\ncore = { path = \"x\" }\n",
                DependencyKind::Development,
                None,
            ),
            (
                "[build_dependencies]\ncore = { path = \"x\" }\n",
                DependencyKind::Build,
                None,
            ),
            (
                "[target.'cfg(unix)'.dev_dependencies]\ncore = { path = \"x\" }\n",
                DependencyKind::Development,
                unix,
            ),
        ] {
            assert_eq!(
                entry(text, kind, target),
                Ok(MemberVersion::Absent),
                "{text}"
            );
        }
    }

    #[test]
    fn a_workspace_entry_declares_a_version_or_names_its_miss() {
        let read = |text: &str| {
            let mut doc: DocumentMut = text.parse().expect("toml");
            workspace_entry(&mut doc, "core").map(|item| workspace_entry_declares_version(item))
        };
        assert_eq!(
            read("[workspace.dependencies]\ncore = \"1\"\n"),
            Ok(Some(true))
        );
        assert_eq!(
            read("[workspace.dependencies]\ncore = { path = \"x\" }\n"),
            Ok(Some(false))
        );
        assert_eq!(read("[workspace.dependencies]\ncore = 1\n"), Ok(None));
        assert_eq!(read("[package]\n"), Err(WorkspaceMiss::WorkspaceTable));
        assert_eq!(read("[workspace]\n"), Err(WorkspaceMiss::DependenciesTable));
        assert_eq!(read("[workspace.dependencies]\n"), Err(WorkspaceMiss::Key));
    }
}
