//! What changed in a package's own external dependency declarations between two
//! discoveries of it (ADR-0040). Inheritance is already resolved, so an entry
//! switched to `workspace = true` that resolves to the same declaration has not
//! moved.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use super::workspace::{DependencyKind, ExternalDependency};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EntryKey {
    kind: DependencyKind,
    target: Option<String>,
    declared_as: String,
}

impl EntryKey {
    fn of(dependency: &ExternalDependency) -> Self {
        Self {
            kind: dependency.kind,
            target: dependency.declaration.target.clone(),
            declared_as: dependency.declared_as.clone(),
        }
    }
}

/// An existing entry whose requirement, features, default features or source
/// moved, and nothing else about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub before: ExternalDependency,
    pub after: ExternalDependency,
}

/// How a manifest changed beside the entries [`declaration_changes`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outside {
    Same,
    DevelopmentOnly,
    Changed,
}

/// What a manifest's change amounts to, read through [`DeclarationChanges::change`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// No entry or key outside the tables differs. A manifest whose text still
    /// changed (formatting, comments) is the caller's to treat as any edit.
    Unchanged,
    DevelopmentOnly,
    DependencyUpdate,
    NeedsIntent,
}

/// Every moved entry is kept, even where the change also needs intent, since
/// each adds its own level under `dependency-updates = "release"` (ADR-0040).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationChanges<M = Moved> {
    pub(crate) moved: Vec<M>,
    pub(crate) needs_intent: bool,
    pub(crate) development: bool,
}

impl<M> Default for DeclarationChanges<M> {
    fn default() -> Self {
        Self {
            moved: Vec::new(),
            needs_intent: false,
            development: false,
        }
    }
}

impl<M> DeclarationChanges<M> {
    #[must_use]
    pub fn moved(&self) -> &[M] {
        &self.moved
    }

    #[must_use]
    pub fn change(&self, outside: Outside) -> Change {
        if self.needs_intent || outside == Outside::Changed {
            Change::NeedsIntent
        } else if !self.moved.is_empty() {
            Change::DependencyUpdate
        } else if self.development || outside == Outside::DevelopmentOnly {
            Change::DevelopmentOnly
        } else {
            Change::Unchanged
        }
    }
}

/// Entries naming a workspace member (`members`) are skipped: the cascade and
/// `version`'s rewrites own those edges (ADR-0008, ADR-0010), and an edit to
/// one is read as [`Outside`].
#[must_use]
pub fn declaration_changes(
    base: &[ExternalDependency],
    head: &[ExternalDependency],
    members: &BTreeSet<String>,
) -> DeclarationChanges {
    let index = |side: &[ExternalDependency]| -> BTreeMap<EntryKey, ExternalDependency> {
        side.iter()
            .filter(|dependency| !members.contains(&dependency.name))
            .map(|dependency| (EntryKey::of(dependency), dependency.clone()))
            .collect()
    };
    diff_entries(
        &index(base),
        &index(head),
        |key| key.kind == DependencyKind::Development,
        same_declaration,
        |before, after| {
            before.name == after.name && before.declaration.optional == after.declaration.optional
        },
        |_, before, after| Moved {
            before: before.clone(),
            after: after.clone(),
        },
    )
}

/// The one classification both ecosystems share: an equal entry is no change,
/// any development difference is development-only, an entry differing only in
/// what `movable` allows moved, and anything else needs intent.
pub(crate) fn diff_entries<K: Ord, E, M>(
    base: &BTreeMap<K, E>,
    head: &BTreeMap<K, E>,
    is_development: impl Fn(&K) -> bool,
    same: impl Fn(&E, &E) -> bool,
    movable: impl Fn(&E, &E) -> bool,
    moved: impl Fn(&K, &E, &E) -> M,
) -> DeclarationChanges<M> {
    let mut changes = DeclarationChanges::default();
    let keys: BTreeSet<&K> = base.keys().chain(head.keys()).collect();
    for key in keys {
        match (base.get(key), head.get(key)) {
            (Some(before), Some(after)) if same(before, after) => {}
            _ if is_development(key) => changes.development = true,
            (Some(before), Some(after)) if movable(before, after) => {
                changes.moved.push(moved(key, before, after));
            }
            _ => changes.needs_intent = true,
        }
    }
    changes
}

/// Every field is named, so one added later is decided here rather than
/// ignored. `inherited` is how the entry is written, not what it declares.
fn same_declaration(before: &ExternalDependency, after: &ExternalDependency) -> bool {
    let ExternalDependency {
        name,
        declared_as: _,
        kind: _,
        source,
        req,
        inherited: _,
        declaration,
    } = before;
    *name == after.name
        && *source == after.source
        && *req == after.req
        && *declaration == after.declaration
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned;

    use semver::VersionReq;

    use super::*;
    use crate::plan::workspace::Declaration;

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn dep(declared_as: &str, kind: DependencyKind, req: &str) -> ExternalDependency {
        ExternalDependency {
            name: declared_as.to_owned(),
            declared_as: declared_as.to_owned(),
            kind,
            source: Some(CRATES_IO.to_owned()),
            req: VersionReq::parse(req).expect("req"),
            inherited: false,
            declaration: Declaration::default(),
        }
    }

    fn rusqlite(req: &str) -> ExternalDependency {
        dep("rusqlite", DependencyKind::Normal, req)
    }

    fn changes(base: &[ExternalDependency], head: &[ExternalDependency]) -> DeclarationChanges {
        declaration_changes(base, head, &BTreeSet::from([String::from("core")]))
    }

    #[test]
    fn a_moved_requirement_is_a_dependency_update() {
        let got = changes(&[rusqlite("^0.31")], &[rusqlite("^0.32")]);
        assert_eq!(
            got,
            DeclarationChanges {
                moved: Vec::from([Moved {
                    before: rusqlite("^0.31"),
                    after: rusqlite("^0.32"),
                }]),
                ..DeclarationChanges::default()
            }
        );
    }

    #[test]
    fn features_default_features_and_source_are_dependency_updates() {
        let with = |edit: fn(&mut ExternalDependency)| {
            let mut after = rusqlite("^0.31");
            edit(&mut after);
            changes(&[rusqlite("^0.31")], &[after])
        };
        for got in [
            with(|d| {
                d.declaration.features.insert(String::from("bundled"));
            }),
            with(|d| d.declaration.default_features = false),
            with(|d| d.source = Some(String::from("git+https://example.invalid/r.git"))),
        ] {
            assert_eq!(got.moved.len(), 1, "{got:?}");
            assert!(!got.needs_intent, "{got:?}");
        }
    }

    #[test]
    fn optional_and_a_renamed_package_need_intent() {
        let mut optional = rusqlite("^0.31");
        optional.declaration.optional = true;
        let mut renamed = rusqlite("^0.31");
        renamed.name = String::from("rusqlite-fork");
        for after in [optional, renamed] {
            let got = changes(&[rusqlite("^0.31")], &[after]);
            assert!(got.needs_intent && got.moved.is_empty(), "{got:?}");
        }
    }

    #[test]
    fn an_added_or_removed_runtime_entry_needs_intent() {
        assert!(changes(&[], &[rusqlite("^0.31")]).needs_intent);
        assert!(changes(&[rusqlite("^0.31")], &[]).needs_intent);
    }

    #[test]
    fn any_development_change_is_development_only() {
        let insta = |req: &str| dep("insta", DependencyKind::Development, req);
        for (base, head) in [
            (Vec::from([insta("^1.40")]), Vec::from([insta("^1.41")])),
            (Vec::new(), Vec::from([insta("^1.40")])),
            (Vec::from([insta("^1.40")]), Vec::new()),
        ] {
            assert_eq!(
                changes(&base, &head),
                DeclarationChanges {
                    development: true,
                    ..DeclarationChanges::default()
                }
            );
        }
    }

    #[test]
    fn a_name_moving_from_dev_to_runtime_needs_intent() {
        let got = changes(
            &[dep("insta", DependencyKind::Development, "^1")],
            &[dep("insta", DependencyKind::Normal, "^1")],
        );
        assert!(got.needs_intent && got.development, "{got:?}");
    }

    #[test]
    fn the_same_key_under_another_target_is_another_entry() {
        let windows = |req: &str| {
            let mut d = dep("winapi", DependencyKind::Normal, req);
            d.declaration.target = Some(String::from("cfg(windows)"));
            d
        };
        let got = changes(
            &[
                windows("^0.3"),
                dep("winapi", DependencyKind::Normal, "^0.3"),
            ],
            &[
                windows("^0.4"),
                dep("winapi", DependencyKind::Normal, "^0.3"),
            ],
        );
        assert_eq!(got.moved.len(), 1, "{got:?}");
        assert_eq!(
            got.moved[0].after.declaration.target.as_deref(),
            Some("cfg(windows)")
        );
    }

    #[test]
    fn an_entry_naming_a_member_is_not_read() {
        let core = |req: &str| dep("core", DependencyKind::Normal, req);
        assert_eq!(
            changes(&[core("^1")], &[core("^2")]),
            DeclarationChanges::default()
        );
        assert_eq!(changes(&[], &[core("^2")]), DeclarationChanges::default());
    }

    #[test]
    fn an_unchanged_entry_reports_nothing() {
        assert_eq!(
            changes(&[rusqlite("^0.31")], &[rusqlite("^0.31")]),
            DeclarationChanges::default()
        );
    }

    #[test]
    fn a_switch_to_inheritance_resolving_the_same_has_not_moved() {
        let inherited = ExternalDependency {
            inherited: true,
            ..rusqlite("^0.31")
        };
        assert_eq!(
            changes(&[rusqlite("^0.31")], &[inherited]),
            DeclarationChanges::default()
        );
    }

    #[test]
    fn a_path_crate_moving_directory_is_a_source_update() {
        let at = |dir: &str| ExternalDependency {
            source: None,
            declaration: Declaration {
                path: Some(dir.to_owned()),
                ..Declaration::default()
            },
            ..dep("vendored", DependencyKind::Normal, "*")
        };
        assert_eq!(changes(&[at("vendor/a")], &[at("vendor/b")]).moved.len(), 1);
    }

    #[test]
    fn change_ranks_intent_over_moves_over_development() {
        let all = DeclarationChanges::<Moved> {
            moved: Vec::from([Moved {
                before: rusqlite("^0.31"),
                after: rusqlite("^0.32"),
            }]),
            needs_intent: true,
            development: true,
        };
        assert_eq!(all.change(Outside::Same), Change::NeedsIntent);
        let moved = DeclarationChanges {
            needs_intent: false,
            ..all.clone()
        };
        assert_eq!(moved.change(Outside::Changed), Change::NeedsIntent);
        assert_eq!(moved.change(Outside::Same), Change::DependencyUpdate);
        assert_eq!(
            moved.change(Outside::DevelopmentOnly),
            Change::DependencyUpdate
        );
        let development = DeclarationChanges::<Moved> {
            development: true,
            ..DeclarationChanges::default()
        };
        assert_eq!(development.change(Outside::Same), Change::DevelopmentOnly);
        let none = DeclarationChanges::<Moved>::default();
        assert_eq!(
            none.change(Outside::DevelopmentOnly),
            Change::DevelopmentOnly
        );
        assert_eq!(none.change(Outside::Same), Change::Unchanged);
    }
}
