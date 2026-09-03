//! ADR-0005 Confirmation: every body oakum writes must be accepted by both
//! foreign parsers with the intended package names — not merely `Ok` / exit 0.
//!
//! knope's `changesets` crate retains quotes on keys and then matches nothing
//! (silent skip). `@changesets/parse` is the format gate behind `@changesets/cli`
//! (workspace membership is out of scope for this suite).
//!
//! JS install/spawn: [`support::changeset_foreign`] (okm-64b.1).
#![allow(clippy::disallowed_methods)]

mod support;

use std::collections::BTreeMap;

use changesets::{Change, ChangeType};
use oakum::changeset::{parse_packages_list, write, KnopePresence, PackagesError, WriteError};
use oakum::plan::BumpLevel;
use support::changeset_foreign::{
    assert_yaml_hole_is_repaired, js_runtime_dir, parse_js_raw, parse_with_changesets_parse,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectKnope {
    /// Intended names; knope must not retain quotes.
    NamesMatch,
    /// Keys keep surrounding quotes (documented silent skip).
    SilentSkip,
}

struct WrittenBody {
    label: &'static str,
    body: String,
    expected: Vec<(&'static str, BumpLevel)>,
    note: &'static str,
    knope: ExpectKnope,
}

fn oakum_bodies() -> Vec<WrittenBody> {
    [
        (
            "unscoped_multi",
            &[("core", BumpLevel::Minor), ("utils", BumpLevel::Patch)][..],
            "\nNotes here.\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "empty_note",
            &[("core", BumpLevel::Patch)],
            "",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "patch",
            &[("core", BumpLevel::Patch)],
            "p\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "minor",
            &[("core", BumpLevel::Minor)],
            "m\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "major",
            &[("core", BumpLevel::Major)],
            "M\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "unscoped_with_knope_flag",
            &[("core", BumpLevel::Patch)],
            "n\n",
            KnopePresence::Present,
            ExpectKnope::NamesMatch,
        ),
        (
            "hyphenated_unscoped",
            &[("oakum-cli", BumpLevel::Patch)],
            "hyphen\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "scoped_quoted",
            &[("@oakum/core", BumpLevel::Minor)],
            "note\n",
            KnopePresence::Absent,
            ExpectKnope::SilentSkip,
        ),
        (
            "multi_scoped",
            &[
                ("@oakum/core", BumpLevel::Minor),
                ("@oakum/pkg-name", BumpLevel::Patch),
            ],
            "two-scoped\n",
            KnopePresence::Absent,
            ExpectKnope::SilentSkip,
        ),
        (
            "mixed_scoped_unscoped",
            &[
                ("@oakum/core", BumpLevel::Minor),
                ("utils", BumpLevel::Patch),
            ],
            "mix\n",
            KnopePresence::Absent,
            ExpectKnope::SilentSkip,
        ),
    ]
    .into_iter()
    .map(|(label, entries, note, knope_flag, knope)| {
        let mut seen = BTreeMap::new();
        for (name, level) in entries {
            assert!(
                seen.insert(*name, *level).is_none(),
                "{label}: duplicate package `{name}` in fixture"
            );
        }
        let body =
            write(entries, note, knope_flag).unwrap_or_else(|e| panic!("{label}: write: {e}"));
        WrittenBody {
            label,
            body,
            expected: entries.to_vec(),
            note,
            knope,
        }
    })
    .collect()
}

#[test]
fn oakum_writes_accepted_by_knope_changesets_crate() {
    for case in oakum_bodies() {
        let change = Change::from_file_name_and_content(&format!("{}.md", case.label), &case.body)
            .unwrap_or_else(|e| panic!("{}: knope parse: {e}", case.label));

        let got: BTreeMap<String, ChangeType> = change
            .versioning
            .iter()
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();

        match case.knope {
            ExpectKnope::NamesMatch => {
                assert_eq!(
                    got.len(),
                    case.expected.len(),
                    "{}: package count (got {got:?})",
                    case.label
                );
                for (name, level) in &case.expected {
                    let ty = got.get(*name).unwrap_or_else(|| {
                        panic!(
                            "{}: missing package `{name}` (got names {:?})",
                            case.label,
                            got.keys()
                        )
                    });
                    assert_eq!(
                        ty,
                        &bump_to_change_type(*level),
                        "{}: level for `{name}`",
                        case.label
                    );
                }
                assert_eq!(
                    change.summary,
                    knope_summary(case.note),
                    "{}: summary",
                    case.label
                );
            }
            ExpectKnope::SilentSkip => {
                let expected_keys: Vec<String> = case
                    .expected
                    .iter()
                    .map(|(name, _)| knope_retained_key(name))
                    .collect();
                let mut got_keys: Vec<String> = got.keys().cloned().collect();
                got_keys.sort();
                let mut want_keys = expected_keys;
                want_keys.sort();
                assert_eq!(
                    got_keys, want_keys,
                    "{}: knope silent-skip keys",
                    case.label
                );
                for (name, level) in &case.expected {
                    let key = knope_retained_key(name);
                    assert_eq!(
                        got.get(&key),
                        Some(&bump_to_change_type(*level)),
                        "{}: level under retained key `{key}`",
                        case.label
                    );
                    if key != *name {
                        assert!(
                            !got.contains_key(*name),
                            "{}: intended name `{name}` must not appear when knope retains quotes",
                            case.label
                        );
                    }
                }
                assert_eq!(
                    change.summary,
                    knope_summary(case.note),
                    "{}: summary",
                    case.label
                );
            }
        }
    }
}

#[test]
fn quoted_unscoped_key_is_silent_skip_under_knope() {
    // Oakum never writes quoted unscoped keys; retained quotes are the Confirmation detector.
    let body = "---\n\"core\": patch\n---\n";
    let change = Change::from_file_name_and_content("quoted.md", body).expect("knope Ok");
    assert_eq!(
        change.versioning,
        changesets::Versioning::from(("\"core\"", ChangeType::Patch))
    );
}

#[test]
fn quoted_unscoped_key_is_accepted_by_changesets_parse() {
    let runtime = js_runtime_dir();
    let body = "---\n\"core\": patch\n---\n";
    let parsed = parse_with_changesets_parse(&runtime, body).expect("@changesets/parse");
    assert_eq!(parsed.releases.len(), 1);
    assert_eq!(parsed.releases[0].name, "core");
    assert_eq!(parsed.releases[0].bump, BumpLevel::Patch);
}

#[test]
fn ensure_js_deps_repairs_a_yaml_hole() {
    assert_yaml_hole_is_repaired();
}

#[test]
fn yaml_plain_keys_that_parse_renames_are_refused_by_oakum() {
    let runtime = js_runtime_dir();
    for (written, remapped) in [
        ("01", "1"),
        ("0x10", "16"),
        ("1e2", "100"),
        ("-0", "0"),
        ("0777", "777"),
        ("True", "true"),
        ("TRUE", "true"),
        ("False", "false"),
        ("FALSE", "false"),
        ("0o10", "8"),
        ("-.inf", "-Infinity"),
        (".nan", "NaN"),
        ("9007199254740993", "9007199254740992"),
    ] {
        let body = format!("---\n{written}: patch\n---\n");
        let parsed = parse_with_changesets_parse(&runtime, &body)
            .unwrap_or_else(|e| panic!("{written}: {e}"));
        assert_eq!(parsed.releases[0].name, remapped, "{written}");
        assert_eq!(
            write(&[(written, BumpLevel::Patch)], "", KnopePresence::Absent),
            Err(WriteError::InvalidPackageName(written.to_string())),
            "{written}"
        );
        assert!(
            matches!(
                parse_packages_list(&format!("{written}:patch")).unwrap_err(),
                PackagesError::InvalidPackageName { .. }
            ),
            "{written}"
        );
    }
}

#[test]
fn unquoted_scoped_key_is_rejected_by_changesets_parse() {
    let runtime = js_runtime_dir();
    let body = "---\n@oakum/core: minor\n---\n";
    let err = parse_with_changesets_parse(&runtime, body).expect_err("unquoted scoped YAML");
    assert!(
        err.to_lowercase().contains("reserved character @"),
        "expected YAML reserved-@ failure, got: {err}"
    );
}

#[test]
fn unquoted_scoped_key_is_accepted_by_knope() {
    let body = "---\n@oakum/core: minor\n---\n";
    let change = Change::from_file_name_and_content("unquoted.md", body).expect("knope Ok");
    assert_eq!(
        change.versioning,
        changesets::Versioning::from(("@oakum/core", ChangeType::Minor))
    );
}

#[test]
fn oakum_writes_accepted_by_changesets_parse() {
    let runtime = js_runtime_dir();

    for case in oakum_bodies() {
        let parsed = parse_with_changesets_parse(&runtime, &case.body)
            .unwrap_or_else(|e| panic!("{}: @changesets/parse: {e}", case.label));

        assert_eq!(
            parsed.releases.len(),
            case.expected.len(),
            "{}: release count",
            case.label
        );
        let mut by_name: BTreeMap<&str, BumpLevel> = BTreeMap::new();
        for release in &parsed.releases {
            assert!(
                by_name
                    .insert(release.name.as_str(), release.bump)
                    .is_none(),
                "{}: duplicate release name `{}`",
                case.label,
                release.name
            );
        }
        for (name, level) in &case.expected {
            let got = by_name.get(name).unwrap_or_else(|| {
                panic!(
                    "{}: missing `{name}` (got {:?})",
                    case.label,
                    by_name.keys()
                )
            });
            assert_eq!(got, level, "{}: type for `{name}`", case.label);
        }
        assert_eq!(
            parsed.summary,
            knope_summary(case.note),
            "{}: summary",
            case.label
        );
    }
}

#[test]
fn adr0028_none_accepted_by_oakum_and_changesets_parse() {
    use oakum::changeset::parse;

    let body = write(
        &[("core", BumpLevel::None)],
        "covered\n",
        KnopePresence::Absent,
    )
    .expect("write");
    let oakum = parse(&body).expect("oakum parse");
    assert_eq!(oakum.entries(), &[("core".to_string(), BumpLevel::None)]);

    let runtime = js_runtime_dir();
    let parsed = parse_with_changesets_parse(&runtime, &body).expect("@changesets/parse");
    assert_eq!(parsed.releases.len(), 1);
    assert_eq!(parsed.releases[0].name, "core");
    assert_eq!(parsed.releases[0].bump, BumpLevel::None);

    // knope treats `none` as Custom → patch semantics; out of Confirmation scope.
    let change = Change::from_file_name_and_content("none.md", &body).expect("knope Ok");
    let ty = change.versioning.iter().next().expect("one entry").1;
    assert!(
        matches!(ty, ChangeType::Custom(ref s) if s == "none"),
        "expected Custom(none), got {ty:?}"
    );
}

#[test]
fn adr0028_empty_frontmatter_accepted_by_oakum_and_changesets_parse() {
    use oakum::changeset::parse;

    let empty: [(&str, BumpLevel); 0] = [];
    let body = write(&empty, "docs only\n", KnopePresence::Absent).expect("write");
    let oakum = parse(&body).expect("oakum parse");
    assert!(oakum.entries().is_empty());
    assert_eq!(oakum.note(), "docs only\n");

    let runtime = js_runtime_dir();
    let parsed = parse_js_raw(&runtime, &body).expect("@changesets/parse");
    assert!(
        parsed.releases.is_empty(),
        "empty frontmatter should yield zero releases"
    );
    assert_eq!(parsed.summary, knope_summary("docs only\n"));
}

fn bump_to_change_type(level: BumpLevel) -> ChangeType {
    match level {
        BumpLevel::None => ChangeType::Custom(String::from("none")),
        BumpLevel::Patch => ChangeType::Patch,
        BumpLevel::Minor => ChangeType::Minor,
        BumpLevel::Major => ChangeType::Major,
    }
}

/// Package key as knope's splitter sees it (scoped names keep surrounding quotes).
fn knope_retained_key(name: &str) -> String {
    if name.starts_with('@') {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

/// Mirror knope's summary trim; `@changesets/parse` matches it on these fixtures.
fn knope_summary(note: &str) -> String {
    note.lines()
        .skip_while(|line| line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
