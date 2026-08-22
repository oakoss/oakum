//! Discovery output feeding the cascade: pnpm `optionalDependencies`
//! overrides must reach the planner as the effective edges (`okm-asb`).

use std::path::PathBuf;

use oakum::discover::discover_pnpm;
use oakum::plan::aggregate::{aggregate, BumpFile};
use oakum::plan::bump::{BumpLevel, Versioning};
use oakum::plan::cascade::CascadeAs;
use oakum::plan::workspace::{Ecosystem, PackageId};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pnpm-discover/optional-override")
}

/// The app declares `@oakum/lib` twice: exact `0.1.0` in `dependencies`
/// (breaks on any bump) shadowed by `>=0.1.0` in `optionalDependencies`
/// (survives). npm installs the optional declaration, so a patch bump of
/// lib must not cascade into app — acting on the shadowed exact range was
/// the false dependent bump this fixture pins against.
#[test]
fn cascade_uses_only_the_effective_optional_range() {
    let workspace = discover_pnpm(fixture_root(), fixture_root()).expect("discover");
    let intent = aggregate([BumpFile {
        id: String::from("okm-asb"),
        entries: vec![(
            PackageId::new(Ecosystem::Npm, "@oakum/lib"),
            BumpLevel::Patch,
        )],
        note: String::from("patch lib"),
    }]);

    let plan = oakum::plan::compose(
        &workspace,
        &intent,
        |_| Versioning::ZeroMajor,
        CascadeAs::Patch,
        |_, edge| Some(edge.range.clone()),
        |id| {
            workspace
                .get(id)
                .expect("package in workspace")
                .version()
                .clone()
        },
    )
    .expect("compose");

    let lib = PackageId::new(Ecosystem::Npm, "@oakum/lib");
    let app = PackageId::new(Ecosystem::Npm, "@oakum/app");
    let other = PackageId::new(Ecosystem::Npm, "@oakum/other");
    let exact = PackageId::new(Ecosystem::Npm, "@oakum/exact");
    assert!(plan.changes().contains_key(&lib), "lib bump is the intent");
    // Positive control: an unshadowed exact range on the same bump does
    // cascade, so app's absence below is the override at work rather than
    // the planner ignoring optional edges.
    assert!(
        plan.changes().contains_key(&exact),
        "@oakum/exact declares an unshadowed exact 0.1.0 and must cascade"
    );
    assert!(
        !plan.changes().contains_key(&app),
        "the effective >=0.1.0 optional range still resolves after a patch \
         bump; a cascade here means the shadowed exact range leaked through"
    );
    assert!(
        !plan.changes().contains_key(&other),
        "@oakum/other was not bumped and must not appear"
    );
}
