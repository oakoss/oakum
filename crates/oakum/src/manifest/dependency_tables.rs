//! A member manifest's dependency tables across a change (ADR-0040). Cargo's
//! entries come from discovery instead ([`crate::plan::declaration_changes`]).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value as Json};
use toml::{Table, Value as Toml};

use super::catalog_file::{catalog_target, CatalogFile, CatalogTarget};
use super::npm_spec::split_name_version;
use crate::plan::{DeclarationChanges, DependencyKind, Ecosystem, Outside};

const CARGO_TABLES: [&str; 5] = [
    "dependencies",
    "dev-dependencies",
    "dev_dependencies",
    "build-dependencies",
    "build_dependencies",
];

const CARGO_DEV_TABLES: [&str; 2] = ["dev-dependencies", "dev_dependencies"];

const NPM_TABLES: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];

#[derive(PartialEq)]
enum Rest {
    Cargo(Table),
    Npm(Map<String, Json>),
}

/// Whether the manifest changed outside its dependency tables, or in an entry
/// naming a member, which is never a dependency update (ADR-0040). A root
/// manifest's `[workspace.dependencies]` counts as a dependency table, and a
/// change confined to development tables, a member entry there included, is
/// [`Outside::DevelopmentOnly`].
///
/// `members` holds every key a member is declared under on either side, its
/// name and any rename or inherited alias, as discovery reads the edges.
///
/// # Errors
///
/// A side that does not parse as the ecosystem's manifest format, named by
/// `base` or `head`.
pub fn changed_outside_dependencies(
    ecosystem: Ecosystem,
    base: &str,
    head: &str,
    members: &BTreeSet<String>,
) -> Result<Outside, String> {
    let rest = |text: &str, side: &str, with_dev: bool| match ecosystem {
        Ecosystem::Cargo => cargo_rest(text, side, members, with_dev).map(Rest::Cargo),
        Ecosystem::Npm => npm_rest(text, side, members, with_dev).map(Rest::Npm),
    };
    if rest(base, "base", true)? == rest(head, "head", true)? {
        Ok(Outside::Same)
    } else if rest(base, "base", false)? == rest(head, "head", false)? {
        Ok(Outside::DevelopmentOnly)
    } else {
        Ok(Outside::Changed)
    }
}

fn cargo_rest(
    text: &str,
    side: &str,
    members: &BTreeSet<String>,
    with_dev: bool,
) -> Result<Table, String> {
    let mut table: Table = text
        .parse()
        .map_err(|err| format!("{side} side is not TOML: {err}"))?;
    let strip = |table: &mut Table| {
        if !with_dev {
            for key in CARGO_DEV_TABLES {
                table.remove(key);
            }
        }
        keep_member_entries(table, members);
    };
    strip(&mut table);
    if let Some(Toml::Table(targets)) = table.get_mut("target") {
        targets.retain(|_, spec| {
            let Toml::Table(spec) = spec else {
                return true;
            };
            strip(spec);
            !spec.is_empty()
        });
        if targets.is_empty() {
            table.remove("target");
        }
    }
    if let Some(Toml::Table(workspace)) = table.get_mut("workspace") {
        keep_member_entries_of(workspace, "dependencies", members);
    }
    Ok(table)
}

fn keep_member_entries(table: &mut Table, members: &BTreeSet<String>) {
    for key in CARGO_TABLES {
        keep_member_entries_of(table, key, members);
    }
}

fn keep_member_entries_of(table: &mut Table, key: &str, members: &BTreeSet<String>) {
    let Some(Toml::Table(entries)) = table.get_mut(key) else {
        return;
    };
    entries.retain(|name, entry| {
        let package = entry.get("package").and_then(Toml::as_str).unwrap_or(name);
        members.contains(package)
    });
    if entries.is_empty() {
        table.remove(key);
    }
}

fn npm_rest(
    text: &str,
    side: &str,
    members: &BTreeSet<String>,
    with_dev: bool,
) -> Result<Map<String, Json>, String> {
    let Json::Object(mut object) =
        serde_json::from_str(text).map_err(|err| format!("{side} side is not JSON: {err}"))?
    else {
        return Err(format!("{side} side is not a JSON object"));
    };
    if !with_dev {
        object.remove("devDependencies");
    }
    for key in NPM_TABLES {
        let Some(Json::Object(entries)) = object.get_mut(key) else {
            continue;
        };
        entries.retain(|name, spec| {
            let spec = spec.as_str().unwrap_or_default().trim();
            spec.starts_with("workspace:") || members.contains(&npm_target(name, spec).0)
        });
        if entries.is_empty() {
            object.remove(key);
        }
    }
    Ok(object)
}

/// The package an entry installs, and its range: the key and spec, or an
/// `npm:` alias's target and version.
fn npm_target(key: &str, spec: &str) -> (String, String) {
    match spec.strip_prefix("npm:").and_then(split_name_version) {
        Some((target, version)) => (target.to_owned(), version.trim().to_owned()),
        None => (key.to_owned(), spec.trim().to_owned()),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NpmSide<'a> {
    pub manifest: &'a str,
    pub workspace_file: Option<&'a str>,
}

/// Both ranges resolved through the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmMoved {
    pub kind: DependencyKind,
    pub declared_as: String,
    pub before: String,
    pub after: String,
}

/// As [`crate::plan::declaration_changes`], for an npm member: the entries in
/// force, an `optionalDependencies` entry shadowing the same name in
/// `dependencies` as npm installs them. Entries naming a workspace member, by
/// name or through `workspace:`, are skipped (ADR-0008, ADR-0010).
///
/// # Errors
///
/// A side that does not parse, or a `catalog:` entry its catalog does not name.
pub fn npm_declaration_changes(
    base: NpmSide<'_>,
    head: NpmSide<'_>,
    members: &BTreeSet<String>,
) -> Result<DeclarationChanges<NpmMoved>, String> {
    let base = npm_entries(base, "base", members)?;
    let head = npm_entries(head, "head", members)?;
    let mut changes = DeclarationChanges::default();
    let keys: BTreeSet<&(DependencyKind, String)> = base.keys().chain(head.keys()).collect();
    for key in keys {
        let (kind, declared_as) = key;
        let development = *kind == DependencyKind::Development;
        match (base.get(key), head.get(key)) {
            (Some(before), Some(after)) if before == after => {}
            _ if development => changes.development = true,
            (Some(before), Some(after)) if before.target == after.target => {
                changes.moved.push(NpmMoved {
                    kind: *kind,
                    declared_as: declared_as.clone(),
                    before: before.range.clone(),
                    after: after.range.clone(),
                });
            }
            _ => changes.needs_intent = true,
        }
    }
    Ok(changes)
}

#[derive(Debug, PartialEq, Eq)]
struct NpmEntry {
    /// The key, or an `npm:` alias's target.
    target: String,
    range: String,
}

fn npm_entries(
    side: NpmSide<'_>,
    name: &str,
    members: &BTreeSet<String>,
) -> Result<BTreeMap<(DependencyKind, String), NpmEntry>, String> {
    let Json::Object(object) = serde_json::from_str(side.manifest)
        .map_err(|err| format!("{name} side is not JSON: {err}"))?
    else {
        return Err(format!("{name} side is not a JSON object"));
    };
    let catalogs = side
        .workspace_file
        .map(CatalogFile::parse)
        .transpose()
        .map_err(|err| format!("{name} side's pnpm-workspace.yaml does not parse: {err}"))?;
    let table = |key: &str| -> Result<BTreeMap<String, String>, String> {
        match object.get(key) {
            None => Ok(BTreeMap::new()),
            Some(Json::Object(entries)) => entries
                .iter()
                .map(|(dep, spec)| match spec {
                    Json::String(spec) => Ok((dep.clone(), spec.clone())),
                    _ => Err(format!("{name} side's `{key}.{dep}` is not a string")),
                })
                .collect(),
            Some(_) => Err(format!("{name} side's `{key}` is not an object")),
        }
    };
    let optional = table("optionalDependencies")?;
    let mut normal = table("dependencies")?;
    normal.retain(|dep, _| !optional.contains_key(dep));
    let mut entries = BTreeMap::new();
    for (kind, specs) in [
        (DependencyKind::Normal, normal),
        (DependencyKind::Optional, optional),
        (DependencyKind::Peer, table("peerDependencies")?),
        (DependencyKind::Development, table("devDependencies")?),
    ] {
        for (dep, spec) in specs {
            let spec = spec.trim();
            if spec.starts_with("workspace:") {
                continue;
            }
            let resolved = match spec.strip_prefix("catalog:") {
                Some(catalog) => resolve_catalog(catalogs.as_ref(), catalog.trim(), &dep)
                    .map_err(|why| format!("{name} side's `{dep}: {spec}` {why}"))?,
                None => spec.to_owned(),
            };
            let (target, range) = npm_target(&dep, &resolved);
            if members.contains(&target) {
                continue;
            }
            entries.insert((kind, dep), NpmEntry { target, range });
        }
    }
    Ok(entries)
}

fn resolve_catalog(
    catalogs: Option<&CatalogFile>,
    catalog: &str,
    dep: &str,
) -> Result<String, &'static str> {
    let catalogs = catalogs.ok_or("names no catalog entry")?;
    let name = (!catalog.is_empty() && catalog != "default").then_some(catalog);
    match catalog_target(
        name,
        dep,
        catalogs.catalog.is_some(),
        catalogs.has_default_table(),
    ) {
        CatalogTarget::Duplicate => {
            Err("resolves against both `catalog` and `catalogs.default`, which pnpm refuses")
        }
        CatalogTarget::At { path, .. } => catalogs
            .string_at(&path)
            .map(str::to_owned)
            .ok_or("names no catalog entry"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO: &str = r#"[package]
name = "cli"
version = "0.1.0"
description = "a tool"

[dependencies]
rusqlite = "0.31"

[dev-dependencies]
insta = "1"

[target.'cfg(windows)'.dependencies]
winapi = "0.3"
"#;

    fn members() -> BTreeSet<String> {
        BTreeSet::from([String::from("core")])
    }

    fn cargo_outside(base: &str, head: &str) -> Outside {
        changed_outside_dependencies(Ecosystem::Cargo, base, head, &members()).expect("parses")
    }

    fn cargo(base: &str, head: &str) -> bool {
        cargo_outside(base, head) == Outside::Changed
    }

    #[test]
    fn a_dev_table_edit_is_development_only_even_naming_a_member() {
        let dev = |entry: &str| CARGO.replace("insta = \"1\"", &format!("insta = \"1\"\n{entry}"));
        assert_eq!(
            cargo_outside(
                &dev(r#"core = { path = "../core" }"#),
                &dev(r#"core = { path = "../core", features = ["x"] }"#)
            ),
            Outside::DevelopmentOnly
        );
        let unix = |req: &str| {
            format!("{CARGO}\n[target.'cfg(unix)'.dev-dependencies]\ncore = {{ path = \"../core\", version = \"{req}\" }}\n")
        };
        assert_eq!(
            cargo_outside(&unix("1"), &unix("2")),
            Outside::DevelopmentOnly
        );
        assert_eq!(cargo_outside(CARGO, CARGO), Outside::Same);
    }

    #[test]
    fn an_inherited_alias_of_a_member_is_read_as_the_member() {
        let with = |features: &str| {
            CARGO.replace(
                "rusqlite = \"0.31\"",
                &format!("rusqlite = \"0.31\"\nmy-core = {{ workspace = true{features} }}"),
            )
        };
        let aliased = BTreeSet::from([String::from("core"), String::from("my-core")]);
        let read = |members: &BTreeSet<String>| {
            changed_outside_dependencies(
                Ecosystem::Cargo,
                &with(""),
                &with(", features = [\"x\"]"),
                members,
            )
            .expect("parses")
        };
        assert_eq!(read(&aliased), Outside::Changed);
        assert_eq!(read(&members()), Outside::Same);
    }

    #[test]
    fn a_first_target_section_of_dependencies_is_not_outside() {
        let base = CARGO.replace(
            "[target.'cfg(windows)'.dependencies]\nwinapi = \"0.3\"\n",
            "",
        );
        assert_eq!(cargo_outside(&base, CARGO), Outside::Same);
    }

    #[test]
    fn every_cargo_table_spelling_is_a_dependency_table() {
        for table in [
            "build-dependencies",
            "build_dependencies",
            "dev_dependencies",
        ] {
            let at = |req: &str| format!("{CARGO}\n[{table}]\ncc = \"{req}\"\n");
            assert!(!cargo(&at("1"), &at("1.1")), "{table}");
        }
        let unix = format!("{CARGO}\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n");
        assert!(!cargo(CARGO, &unix));
    }

    #[test]
    fn an_edit_to_an_entry_naming_a_member_is_outside() {
        let with = |entry: &str| {
            CARGO.replace(
                "rusqlite = \"0.31\"",
                &format!("rusqlite = \"0.31\"\ncore = {entry}"),
            )
        };
        assert!(cargo(
            &with(r#"{ path = "../core" }"#),
            &with(r#"{ path = "../core", features = ["x"] }"#)
        ));
        assert!(cargo(
            &with(r#"{ path = "../core" }"#),
            &with(r#"{ path = "../core", version = "2" }"#)
        ));
        let renamed = |features: &str| {
            CARGO.replace(
                "rusqlite = \"0.31\"",
                &format!("rusqlite = \"0.31\"\nmine = {{ package = \"core\", path = \"../core\"{features} }}"),
            )
        };
        assert!(cargo(&renamed(""), &renamed(", features = [\"x\"]")));
    }

    #[test]
    fn a_root_workspace_dependency_move_is_not_outside_but_a_member_entry_is() {
        let root = |rusqlite: &str, core: &str| {
            format!(
                "{CARGO}\n[workspace]\nmembers = [\"core\"]\n\n[workspace.dependencies]\nrusqlite = \"{rusqlite}\"\ncore = {{ path = \"core\"{core} }}\n"
            )
        };
        assert!(!cargo(&root("0.31", ""), &root("0.32", "")));
        assert!(cargo(
            &root("0.31", ""),
            &root("0.31", ", features = [\"x\"]")
        ));
        assert!(cargo(
            &root("0.31", ""),
            &root("0.31", "").replace("members = [\"core\"]", "members = [\"core\", \"cli\"]")
        ));
    }

    #[test]
    fn a_cargo_dependency_edit_changes_nothing_outside() {
        let head = CARGO
            .replace(
                "rusqlite = \"0.31\"",
                "rusqlite = { version = \"0.32\", features = [\"bundled\"] }",
            )
            .replace("insta = \"1\"", "insta = \"1.41\"\ntempfile = \"3\"")
            .replace("winapi = \"0.3\"", "winapi = \"0.4\"");
        assert!(!cargo(CARGO, &head));
    }

    #[test]
    fn a_cargo_edit_beside_the_dependencies_is_outside() {
        assert!(cargo(CARGO, &CARGO.replace("a tool", "the tool")));
        assert!(cargo(
            CARGO,
            &format!("{CARGO}\n[target.'cfg(windows)'.rustflags]\nx = 1\n")
        ));
        assert!(cargo(CARGO, &format!("{CARGO}\n[features]\nfast = []\n")));
    }

    #[test]
    fn formatting_alone_changes_nothing_outside() {
        assert!(!cargo(
            CARGO,
            &CARGO.replace("[package]", "# the cli\n[package]")
        ));
    }

    const NPM: &str = r#"{
  "name": "app",
  "version": "1.0.0",
  "description": "an app",
  "dependencies": { "react": "^18.2.0" },
  "devDependencies": { "vitest": "^2.0.0" },
  "peerDependencies": { "host": "^1" }
}"#;

    fn npm_outside(base: &str, head: &str) -> Outside {
        changed_outside_dependencies(Ecosystem::Npm, base, head, &members()).expect("parses")
    }

    fn npm(base: &str, head: &str) -> bool {
        npm_outside(base, head) == Outside::Changed
    }

    #[test]
    fn npm_dev_member_edits_are_development_only_and_spacing_is_read_through() {
        assert_eq!(
            npm_outside(
                &deps("devDependencies", r#""core": "workspace:^""#),
                &deps("devDependencies", r#""core": "workspace:~""#),
            ),
            Outside::DevelopmentOnly
        );
        assert!(npm(
            &deps("dependencies", r#""core": " workspace:^""#),
            &deps("dependencies", r#""core": " workspace:~""#),
        ));
        assert_eq!(
            npm_changes(
                &deps("dependencies", r#""lodash": "npm:lodash-es@ ^4""#),
                &deps("dependencies", r#""lodash": "npm:lodash-es@^4""#),
            ),
            DeclarationChanges::default()
        );
    }

    #[test]
    fn an_npm_edit_to_an_entry_naming_a_member_is_outside() {
        assert!(npm(
            &deps("dependencies", r#""core": "workspace:^""#),
            &deps("dependencies", r#""core": "workspace:~""#),
        ));
        assert!(npm(
            &deps("dependencies", r#""mine": "npm:core@^1""#),
            &deps("dependencies", r#""mine": "npm:core@^2""#),
        ));
        assert!(npm(
            &deps("dependencies", r#""x": "workspace:*""#),
            &deps("dependencies", r#""x": "workspace:^""#),
        ));
    }

    #[test]
    fn an_npm_dependency_edit_changes_nothing_outside() {
        let head = NPM
            .replace("^18.2.0", "^19.0.0")
            .replace("\"^1\"", "\"^1 || ^2\"")
            .replace(
                "\"devDependencies\"",
                "\"optionalDependencies\": { \"fsevents\": \"^2\" },\n  \"devDependencies\"",
            );
        assert!(!npm(NPM, &head));
    }

    #[test]
    fn an_npm_edit_beside_the_dependencies_is_outside() {
        assert!(npm(NPM, &NPM.replace("an app", "the app")));
        assert!(npm(
            NPM,
            &NPM.replace(
                "\"name\"",
                "\"peerDependenciesMeta\": { \"host\": { \"optional\": true } },\n  \"name\""
            )
        ));
    }

    fn npm_changes(base: &str, head: &str) -> DeclarationChanges<NpmMoved> {
        npm_changes_with(base, head, None, None).expect("reads")
    }

    fn npm_changes_with(
        base: &str,
        head: &str,
        base_yaml: Option<&str>,
        head_yaml: Option<&str>,
    ) -> Result<DeclarationChanges<NpmMoved>, String> {
        npm_declaration_changes(
            NpmSide {
                manifest: base,
                workspace_file: base_yaml,
            },
            NpmSide {
                manifest: head,
                workspace_file: head_yaml,
            },
            &BTreeSet::from([String::from("core")]),
        )
    }

    fn deps(section: &str, entries: &str) -> String {
        format!(r#"{{ "name": "app", "version": "1.0.0", "{section}": {{ {entries} }} }}"#)
    }

    #[test]
    fn an_npm_range_move_is_a_dependency_update() {
        let got = npm_changes(
            &deps("dependencies", r#""react": "^18.2.0""#),
            &deps("dependencies", r#""react": "^19.0.0""#),
        );
        assert_eq!(
            got,
            DeclarationChanges::<NpmMoved> {
                moved: vec![NpmMoved {
                    kind: DependencyKind::Normal,
                    declared_as: String::from("react"),
                    before: String::from("^18.2.0"),
                    after: String::from("^19.0.0"),
                }],
                ..DeclarationChanges::<NpmMoved>::default()
            }
        );
    }

    #[test]
    fn a_shadowed_dependencies_entry_is_not_read() {
        let manifest = |shadowed: &str| {
            format!(
                r#"{{ "dependencies": {{ "fsevents": "{shadowed}" }}, "optionalDependencies": {{ "fsevents": "^2" }} }}"#
            )
        };
        assert_eq!(
            npm_changes(&manifest("^1"), &manifest("^3")),
            DeclarationChanges::<NpmMoved>::default()
        );
    }

    #[test]
    fn an_alias_keeping_its_target_moves_and_one_changing_it_needs_intent() {
        let alias = |spec: &str| deps("dependencies", &format!(r#""lodash": "{spec}""#));
        let kept = npm_changes(
            &alias("npm:lodash-es@^4.17.0"),
            &alias("npm:lodash-es@^4.18.0"),
        );
        assert_eq!(kept.moved.len(), 1, "{kept:?}");
        assert_eq!(kept.moved[0].after, "^4.18.0");
        let retargeted = npm_changes(&alias("npm:lodash-es@^4"), &alias("npm:lodash@^4"));
        assert!(
            retargeted.needs_intent && retargeted.moved.is_empty(),
            "{retargeted:?}"
        );
    }

    #[test]
    fn a_catalog_switch_resolving_to_the_same_range_changes_nothing() {
        let yaml = "catalog:\n  react: ^18.2.0\n";
        let got = npm_changes_with(
            &deps("dependencies", r#""react": "^18.2.0""#),
            &deps("dependencies", r#""react": "catalog:""#),
            None,
            Some(yaml),
        )
        .expect("resolves");
        assert_eq!(got, DeclarationChanges::<NpmMoved>::default());
    }

    #[test]
    fn a_catalog_entry_naming_nothing_is_an_error() {
        let err = npm_changes_with(
            &deps("dependencies", r#""react": "^18.2.0""#),
            &deps("dependencies", r#""react": "catalog:web""#),
            None,
            Some("catalog:\n  react: ^18.2.0\n"),
        )
        .expect_err("no `web` catalog");
        assert!(err.contains("names no catalog entry"), "{err}");
    }

    #[test]
    fn npm_development_member_and_added_entries() {
        assert_eq!(
            npm_changes(
                &deps("devDependencies", r#""vitest": "^2""#),
                &deps("devDependencies", r#""vitest": "^3", "tsx": "^4""#),
            ),
            DeclarationChanges::<NpmMoved> {
                development: true,
                ..DeclarationChanges::<NpmMoved>::default()
            }
        );
        assert_eq!(
            npm_changes(
                &deps("dependencies", r#""core": "^1""#),
                &deps("dependencies", r#""core": "workspace:^""#),
            ),
            DeclarationChanges::<NpmMoved>::default()
        );
        assert!(
            npm_changes(
                &deps("peerDependencies", r#""host": "^1""#),
                &deps("peerDependencies", r#""host": "^1", "other": "^2""#),
            )
            .needs_intent
        );
        assert_eq!(
            npm_changes(
                &deps("dependencies", r#""x": "workspace:*""#),
                &deps("dependencies", r#""x": "workspace:^""#),
            ),
            DeclarationChanges::<NpmMoved>::default()
        );
    }

    #[test]
    fn an_entry_moving_between_tables_needs_intent() {
        for table in ["peerDependencies", "optionalDependencies"] {
            let got = npm_changes(
                &deps("dependencies", r#""react": "^18""#),
                &deps(table, r#""react": "^18""#),
            );
            assert!(got.needs_intent && got.moved.is_empty(), "{table}: {got:?}");
        }
    }

    #[test]
    fn default_and_named_catalogs_resolve() {
        let yaml = "catalog:\n  react: ^18.2.0\ncatalogs:\n  web:\n    react: ' ^19.0.0 '\n";
        let at = |spec: &str| deps("dependencies", &format!(r#""react": "{spec}""#));
        let read = |spec: &str| {
            npm_changes_with(&at("^18.2.0"), &at(spec), Some(yaml), Some(yaml)).expect(spec)
        };
        assert_eq!(read("catalog:default"), DeclarationChanges::default());
        assert_eq!(read("catalog:"), DeclarationChanges::default());
        assert_eq!(read("catalog:web").moved()[0].after, "^19.0.0");
    }

    #[test]
    fn a_duplicate_default_catalog_is_named() {
        let err = npm_changes_with(
            &deps("dependencies", r#""react": "^18""#),
            &deps("dependencies", r#""react": "catalog:""#),
            None,
            Some("catalog:\n  react: ^18\ncatalogs:\n  default:\n    react: ^18\n"),
        )
        .expect_err("two default catalogs");
        assert!(
            err.contains("both `catalog` and `catalogs.default`"),
            "{err}"
        );
    }

    #[test]
    fn a_malformed_npm_side_is_an_error_naming_it() {
        let good = deps("dependencies", r#""react": "^18""#);
        for (head, says) in [
            (String::from("[]"), "head side is not a JSON object"),
            (
                String::from(r#"{ "dependencies": [] }"#),
                "head side's `dependencies` is not an object",
            ),
            (
                deps("dependencies", r#""react": 1"#),
                "head side's `dependencies.react` is not a string",
            ),
        ] {
            assert_eq!(
                npm_changes_with(&good, &head, None, None),
                Err(String::from(says))
            );
        }
        let err = npm_changes_with(&good, &good, None, Some(":\n  - ["))
            .expect_err("yaml does not parse");
        assert!(err.starts_with("head side's pnpm-workspace.yaml"), "{err}");
    }

    #[test]
    fn an_unreadable_side_is_an_error_naming_it() {
        let err = changed_outside_dependencies(Ecosystem::Cargo, CARGO, "[package", &members())
            .expect_err("head is not TOML");
        assert!(err.starts_with("head side is not TOML"), "{err}");
        let err =
            changed_outside_dependencies(Ecosystem::Npm, "[]", NPM, &members()).expect_err("base");
        assert_eq!(err, "base side is not a JSON object");
    }
}
