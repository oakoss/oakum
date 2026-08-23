//! Rewrite the `Cargo.lock` rows a workspace version bump invalidates.
//!
//! A member's `version` in `Cargo.toml` is copied into a sourceless
//! `[[package]]` row. Leaving that row behind breaks the next
//! `--locked` build (ADR-0023). Registry and git rows keep a `source`
//! key, so a crates.io crate with the same name is not an invalidated
//! entry. Regenerating the lockfile would also retarget unrelated
//! crates; this helper only rewrites the matching local row and
//! sourceless `"name version"` dependency specs that named the old
//! version.
//!
//! Filesystem writes belong to `version`. This returns the new text.

use semver::Version;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

use super::set_preserving_decor;

/// One workspace (or other path) package whose lockfile row is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CargoLockBump<'a> {
    pub name: &'a str,
    pub from: &'a Version,
    pub to: &'a Version,
}

/// # Errors
///
/// Returns [`CargoLockError`] when the text is not a Cargo lockfile, a
/// bump names no local `[[package]]` at `from` (or already `to`), or two
/// local rows share that identity.
pub fn retarget_cargo_lock(
    text: &str,
    bumps: &[CargoLockBump<'_>],
) -> Result<String, CargoLockError> {
    if bumps.is_empty() {
        return Ok(text.to_owned());
    }
    let mut doc: DocumentMut = text.parse().map_err(CargoLockError::Toml)?;
    match doc.get_mut("package") {
        None => {
            return Err(CargoLockError::MissingPackage {
                name: bumps[0].name.to_owned(),
                from: bumps[0].from.to_string(),
            });
        }
        Some(item) => {
            let Some(packages) = item.as_array_of_tables_mut() else {
                return Err(CargoLockError::NotAPackageList);
            };
            let moved = apply_package_bumps(packages, bumps)?;
            rewrite_dependency_specs(packages, bumps, &moved);
        }
    }
    Ok(doc.to_string())
}

/// For each bump, whether the matched local row was at `from` and got
/// rewritten. Spec rewrites run only for those: after the row is already
/// at `to`, sourceless `"name <from>"` is Cargo's encoding of a registry
/// twin, not the workspace member.
fn apply_package_bumps(
    packages: &mut ArrayOfTables,
    bumps: &[CargoLockBump<'_>],
) -> Result<Vec<bool>, CargoLockError> {
    let mut moved = Vec::with_capacity(bumps.len());
    for bump in bumps {
        let from = bump.from.to_string();
        let to = bump.to.to_string();
        let mut matches = Vec::new();
        for (index, table) in packages.iter().enumerate() {
            if !is_local_package(table) {
                continue;
            }
            if table_str(table, "name") != Some(bump.name) {
                continue;
            }
            match table_str(table, "version") {
                Some(version) if version == from || version == to => matches.push(index),
                _ => {}
            }
        }
        match matches.as_slice() {
            [] => {
                return Err(CargoLockError::MissingPackage {
                    name: bump.name.to_owned(),
                    from,
                });
            }
            [index] => {
                let table = packages
                    .get_mut(*index)
                    .expect("index came from this array");
                let rewrite = table_str(table, "version") == Some(from.as_str()) && from != to;
                if rewrite {
                    set_preserving_decor(&mut table["version"], to);
                }
                moved.push(rewrite);
            }
            _ => {
                return Err(CargoLockError::AmbiguousPackage {
                    name: bump.name.to_owned(),
                    from,
                    to,
                });
            }
        }
    }
    Ok(moved)
}

fn rewrite_dependency_specs(
    packages: &mut ArrayOfTables,
    bumps: &[CargoLockBump<'_>],
    moved: &[bool],
) {
    for table in packages.iter_mut() {
        let Some(deps) = table.get_mut("dependencies").and_then(Item::as_array_mut) else {
            continue;
        };
        for value in deps.iter_mut() {
            let Some(spec) = value.as_str() else {
                continue;
            };
            let Some(next) = retarget_dep_spec(spec, bumps, moved) else {
                continue;
            };
            set_value_str(value, next);
        }
    }
}

/// Sourceless `"name <from>"` only, and only for bumps that moved a local
/// row. A trailing `(source)` names a registry or git package. After the
/// local row is already at `to`, the same sourceless spec names a twin.
fn retarget_dep_spec(spec: &str, bumps: &[CargoLockBump<'_>], moved: &[bool]) -> Option<String> {
    for (bump, &did_move) in bumps.iter().zip(moved) {
        if !did_move {
            continue;
        }
        let from = format!("{} {}", bump.name, bump.from);
        if spec == from {
            return Some(format!("{} {}", bump.name, bump.to));
        }
    }
    None
}

fn is_local_package(table: &Table) -> bool {
    match table.get("source") {
        None => true,
        Some(item) => match item.as_str() {
            Some(source) => source.starts_with("path+") || source.starts_with("vfs+"),
            None => false,
        },
    }
}

fn table_str<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

fn set_value_str(value: &mut Value, next: String) {
    let decor = value.decor().clone();
    *value = next.into();
    *value.decor_mut() = decor;
}

/// Failures from [`retarget_cargo_lock`].
#[derive(Debug)]
pub enum CargoLockError {
    Toml(toml_edit::TomlError),
    NotAPackageList,
    MissingPackage {
        name: String,
        from: String,
    },
    AmbiguousPackage {
        name: String,
        from: String,
        to: String,
    },
}

impl core::fmt::Display for CargoLockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Toml(err) => write!(f, "{err}"),
            Self::NotAPackageList => f.write_str("Cargo.lock `package` is not an array of tables"),
            Self::MissingPackage { name, from } => {
                write!(f, "Cargo.lock has no local package {name} at {from}")
            }
            Self::AmbiguousPackage { name, from, to } => {
                write!(
                    f,
                    "Cargo.lock has more than one local package {name} at {from} or {to}"
                )
            }
        }
    }
}

impl std::error::Error for CargoLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toml(err) => Some(err),
            Self::NotAPackageList | Self::MissingPackage { .. } | Self::AmbiguousPackage { .. } => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::{retarget_cargo_lock, CargoLockBump, CargoLockError};

    fn v(text: &str) -> Version {
        text.parse().expect("version")
    }

    fn bump<'a>(name: &'a str, from: &'a Version, to: &'a Version) -> CargoLockBump<'a> {
        CargoLockBump { name, from, to }
    }

    fn lock(body: &str) -> String {
        format!(
            "# This file is automatically @generated by Cargo.\n\
             # It is not intended for manual editing.\n\
             version = 4\n\
             \n\
             {body}"
        )
    }

    #[test]
    fn empty_bumps_returns_the_input() {
        let src = lock("[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n");
        assert_eq!(retarget_cargo_lock(&src, &[]).expect("ok"), src);
    }

    #[test]
    fn local_package_version_is_rewritten_registry_twin_is_not() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             \n\
             [[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
             checksum = \"abcd\"\n\
             \n\
             [[package]]\n\
             name = \"other-reg\"\n\
             version = \"0.1.0\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
             checksum = \"ef01\"\n",
        );
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert!(out.contains("name = \"demo\"\nversion = \"0.2.0\"\n"));
        assert!(out.contains(
            "name = \"demo\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\""
        ));
        assert!(out.contains("checksum = \"abcd\""));
        assert!(out.contains(
            "name = \"other-reg\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"ef01\""
        ));
    }

    #[test]
    fn sourceless_name_version_dep_is_rewritten_sourced_spec_is_not() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             \n\
             [[package]]\n\
             name = \"app\"\n\
             version = \"0.1.0\"\n\
             dependencies = [\n\
             \"demo 0.1.0\",\n\
             \"demo 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)\",\n\
             \"unrelated\",\n\
             ]\n",
        );
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert!(out.contains("\"demo 0.2.0\""));
        assert!(
            out.contains("\"demo 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)\"")
        );
        assert!(out.contains("\"unrelated\""));
    }

    #[test]
    fn unique_name_dep_spec_is_left_alone() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             \n\
             [[package]]\n\
             name = \"app\"\n\
             version = \"0.1.0\"\n\
             dependencies = [\n\
             \"demo\",\n\
             ]\n",
        );
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert!(out.contains("\"demo\",\n"));
        assert!(!out.contains("\"demo 0.2.0\""));
    }

    #[test]
    fn already_at_target_is_idempotent() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock("[[package]]\nname = \"demo\"\nversion = \"0.2.0\"\n");
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert_eq!(out, src);
    }

    #[test]
    fn missing_local_package_is_an_error() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        );
        let err = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect_err("missing");
        assert!(
            matches!(
                err,
                CargoLockError::MissingPackage { ref name, ref from } if name == "demo" && from == "0.1.0"
            ),
            "{err}"
        );
    }

    #[test]
    fn version_line_decor_is_kept() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock("[[package]]\nname = \"demo\"\nversion = \"0.1.0\"   # keep\n");
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert!(out.contains("version = \"0.2.0\"   # keep\n"));
    }

    #[test]
    fn neighbor_packages_are_untouched() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             \n\
             [[package]]\n\
             name = \"other\"\n\
             version = \"1.2.3\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        );
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert!(out.contains("name = \"other\"\nversion = \"1.2.3\"\n"));
    }

    #[test]
    fn already_at_target_does_not_rewrite_a_registry_twin_spec() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.2.0\"\n\
             \n\
             [[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
             checksum = \"abcd\"\n\
             \n\
             [[package]]\n\
             name = \"app\"\n\
             version = \"0.1.0\"\n\
             dependencies = [\n\
             \"demo 0.1.0\",\n\
             ]\n",
        );
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert_eq!(out, src);
    }

    #[test]
    fn unmoved_bump_does_not_rewrite_specs_when_a_sibling_moves() {
        let demo_from = v("0.1.0");
        let demo_to = v("0.2.0");
        let other_from = v("1.0.0");
        let other_to = v("1.1.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.2.0\"\n\
             \n\
             [[package]]\n\
             name = \"other\"\n\
             version = \"1.0.0\"\n\
             \n\
             [[package]]\n\
             name = \"app\"\n\
             version = \"0.1.0\"\n\
             dependencies = [\n\
             \"demo 0.1.0\",\n\
             \"other 1.0.0\",\n\
             ]\n",
        );
        let out = retarget_cargo_lock(
            &src,
            &[
                bump("demo", &demo_from, &demo_to),
                bump("other", &other_from, &other_to),
            ],
        )
        .expect("ok");
        assert!(out.contains("name = \"demo\"\nversion = \"0.2.0\""));
        assert!(out.contains("name = \"other\"\nversion = \"1.1.0\""));
        assert!(out.contains("\"demo 0.1.0\""));
        assert!(out.contains("\"other 1.1.0\""));
        assert!(!out.contains("\"demo 0.2.0\""));
        assert!(!out.contains("\"other 1.0.0\""));
    }

    #[test]
    fn two_local_rows_at_from_or_to_are_ambiguous() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             \n\
             [[package]]\n\
             name = \"demo\"\n\
             version = \"0.2.0\"\n",
        );
        let err = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect_err("ambiguous");
        assert!(
            matches!(
                err,
                CargoLockError::AmbiguousPackage { ref name, ref from, ref to }
                    if name == "demo" && from == "0.1.0" && to == "0.2.0"
            ),
            "{err}"
        );
    }

    #[test]
    fn path_source_is_local_git_source_is_not() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock(
            "[[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             source = \"path+file:///tmp/demo\"\n\
             \n\
             [[package]]\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             source = \"git+https://example.com/demo\"\n",
        );
        let out = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect("ok");
        assert!(out.contains("version = \"0.2.0\"\nsource = \"path+file:///tmp/demo\""));
        assert!(out.contains("version = \"0.1.0\"\nsource = \"git+https://example.com/demo\""));
    }

    #[test]
    fn non_string_source_is_not_local() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = lock("[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\nsource = 1\n");
        let err = retarget_cargo_lock(&src, &[bump("demo", &from, &to)]).expect_err("not local");
        assert!(
            matches!(err, CargoLockError::MissingPackage { ref name, .. } if name == "demo"),
            "{err}"
        );
    }

    #[test]
    fn package_key_that_is_not_a_table_array_is_an_error() {
        let from = v("0.1.0");
        let to = v("0.2.0");
        let src = "# This file is automatically @generated by Cargo.\nversion = 4\npackage = 4\n";
        let err = retarget_cargo_lock(src, &[bump("demo", &from, &to)]).expect_err("shape");
        assert!(matches!(err, CargoLockError::NotAPackageList), "{err}");
    }
}
