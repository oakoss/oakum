//! Aggregate multiple bump files per package: highest level wins.
//!
//! Parsing the intersection grammar and reading rules live in [`crate::changeset`]
//! (`okm-ep0`, `okm-wnp`). This module takes already-resolved files — package
//! identities, levels, and notes — and folds them into one direct bump per
//! package. Cascade and version math are later passes over that result.
//!
//! Spec rule (`docs/specs/bump-files.md`): files naming the same package
//! accumulate; the highest level decides the bump, and every note appears in
//! the changelog.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::bump::BumpLevel;
use super::workspace::PackageId;

/// One bump file as the planner sees it after parsing and name resolution.
///
/// The resolver that builds this must give a non-empty `id` unique among the
/// files in one aggregate call; this type does not enforce that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BumpFile {
    /// Opaque identity (typically the filename). Consumption and `--explain`
    /// use this to name what was folded in.
    pub id: String,
    /// Packages named in the frontmatter, as resolved. Duplicate packages in
    /// one file are allowed here; [`aggregate`] collapses them to the higher
    /// level and attributes the note once.
    pub entries: Vec<(PackageId, BumpLevel)>,
    /// Markdown body after the closing `---`, kept verbatim.
    pub note: String,
}

/// What one bump file asked for a package, before the aggregate max is taken.
///
/// `level` is the file's request, not the eventual aggregate — so `--explain`
/// can say which file raised the ceiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contribution {
    source: String,
    level: BumpLevel,
    note: String,
}

impl Contribution {
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn level(&self) -> BumpLevel {
        self.level
    }

    #[must_use]
    pub fn note(&self) -> &str {
        &self.note
    }
}

/// The direct bump for one package after folding every bump file that named it.
///
/// Constructed only by [`aggregate`] (`first` / `absorb`); fields stay private
/// so the max-level invariant cannot drift from the contribution list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregatedBump {
    level: BumpLevel,
    contributions: Vec<Contribution>,
}

impl AggregatedBump {
    fn first(level: BumpLevel, source: String, note: String) -> Self {
        Self {
            level,
            contributions: alloc::vec![Contribution {
                source,
                level,
                note,
            }],
        }
    }

    fn absorb(&mut self, level: BumpLevel, source: &str, note: &str) {
        if level > self.level {
            self.level = level;
        }
        self.contributions.push(Contribution {
            source: String::from(source),
            level,
            note: String::from(note),
        });
        debug_assert_eq!(
            self.level,
            self.contributions
                .iter()
                .map(Contribution::level)
                .max()
                .expect("absorb leaves at least one contribution"),
        );
    }

    #[must_use]
    pub fn level(&self) -> BumpLevel {
        self.level
    }

    #[must_use]
    pub fn contributions(&self) -> &[Contribution] {
        &self.contributions
    }

    /// Notes in bump-file encounter order, including empty bodies.
    pub fn notes(&self) -> impl Iterator<Item = &str> {
        self.contributions.iter().map(Contribution::note)
    }
}

/// Fold bump files into one [`AggregatedBump`] per package.
///
/// Output keys follow [`PackageId`]'s `Ord` so snapshot fixtures compare stably.
/// Contribution order follows the input file order. Within one file, a package
/// appears at most once (duplicate entries collapse to the higher level first).
#[must_use]
pub fn aggregate(files: impl IntoIterator<Item = BumpFile>) -> BTreeMap<PackageId, AggregatedBump> {
    let mut out: BTreeMap<PackageId, AggregatedBump> = BTreeMap::new();
    for file in files {
        let mut per_file: BTreeMap<PackageId, BumpLevel> = BTreeMap::new();
        for (package, level) in file.entries {
            per_file
                .entry(package)
                .and_modify(|existing| {
                    if level > *existing {
                        *existing = level;
                    }
                })
                .or_insert(level);
        }
        for (package, level) in per_file {
            match out.get_mut(&package) {
                Some(aggregated) => aggregated.absorb(level, &file.id, &file.note),
                None => {
                    out.insert(
                        package,
                        AggregatedBump::first(level, file.id.clone(), file.note.clone()),
                    );
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::workspace::Ecosystem;
    use alloc::string::ToString;
    use alloc::vec;

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn npm(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Npm, name)
    }

    fn file(id: &str, entries: Vec<(PackageId, BumpLevel)>, note: &str) -> BumpFile {
        BumpFile {
            id: id.to_string(),
            entries,
            note: note.to_string(),
        }
    }

    /// Guide example: three patches and one minor → one minor with four notes.
    #[test]
    fn highest_level_wins_and_every_note_is_kept() {
        let core = cargo("core");
        let plan = aggregate([
            file("a.md", vec![(core.clone(), BumpLevel::Patch)], "fix one"),
            file("b.md", vec![(core.clone(), BumpLevel::Patch)], "fix two"),
            file("c.md", vec![(core.clone(), BumpLevel::Patch)], "fix three"),
            file("d.md", vec![(core.clone(), BumpLevel::Minor)], "feature"),
        ]);
        let aggregated = plan.get(&core).expect("core");
        assert_eq!(aggregated.level(), BumpLevel::Minor);
        assert_eq!(
            aggregated.notes().collect::<Vec<_>>(),
            ["fix one", "fix two", "fix three", "feature"]
        );
        assert_eq!(
            aggregated
                .contributions()
                .iter()
                .map(Contribution::level)
                .collect::<Vec<_>>(),
            [
                BumpLevel::Patch,
                BumpLevel::Patch,
                BumpLevel::Patch,
                BumpLevel::Minor
            ]
        );
        assert_eq!(
            aggregated
                .contributions()
                .iter()
                .map(Contribution::source)
                .collect::<Vec<_>>(),
            ["a.md", "b.md", "c.md", "d.md"]
        );
    }

    #[test]
    fn major_outranks_minor_and_patch() {
        let pkg = cargo("oakum");
        let plan = aggregate([
            file("patch.md", vec![(pkg.clone(), BumpLevel::Patch)], "p"),
            file("major.md", vec![(pkg.clone(), BumpLevel::Major)], "m"),
            file("minor.md", vec![(pkg.clone(), BumpLevel::Minor)], "n"),
        ]);
        assert_eq!(plan[&pkg].level(), BumpLevel::Major);
        assert_eq!(plan[&pkg].notes().count(), 3);
    }

    #[test]
    fn later_lower_level_does_not_lower_the_ceiling() {
        let pkg = cargo("oakum");
        let plan = aggregate([
            file(
                "major.md",
                vec![(pkg.clone(), BumpLevel::Major)],
                "breaking",
            ),
            file("patch.md", vec![(pkg.clone(), BumpLevel::Patch)], "typo"),
        ]);
        assert_eq!(plan[&pkg].level(), BumpLevel::Major);
        assert_eq!(plan[&pkg].notes().collect::<Vec<_>>(), ["breaking", "typo"]);
    }

    #[test]
    fn one_file_note_is_attributed_to_each_named_package() {
        let core = cargo("core");
        let utils = cargo("utils");
        let plan = aggregate([file(
            "shared.md",
            vec![
                (core.clone(), BumpLevel::Minor),
                (utils.clone(), BumpLevel::Patch),
            ],
            "shared note",
        )]);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[&core].level(), BumpLevel::Minor);
        assert_eq!(plan[&utils].level(), BumpLevel::Patch);
        assert_eq!(plan[&core].notes().collect::<Vec<_>>(), ["shared note"]);
        assert_eq!(plan[&utils].notes().collect::<Vec<_>>(), ["shared note"]);
        assert_eq!(plan[&core].contributions()[0].source(), "shared.md");
    }

    #[test]
    fn packages_accumulate_independently() {
        let core = cargo("core");
        let utils = cargo("utils");
        let plan = aggregate([
            file(
                "core-only.md",
                vec![(core.clone(), BumpLevel::Major)],
                "core",
            ),
            file(
                "utils-only.md",
                vec![(utils.clone(), BumpLevel::Patch)],
                "utils",
            ),
            file(
                "both.md",
                vec![
                    (core.clone(), BumpLevel::Patch),
                    (utils.clone(), BumpLevel::Minor),
                ],
                "both",
            ),
        ]);
        assert_eq!(plan[&core].level(), BumpLevel::Major);
        assert_eq!(plan[&utils].level(), BumpLevel::Minor);
        assert_eq!(plan[&core].notes().collect::<Vec<_>>(), ["core", "both"]);
        assert_eq!(plan[&utils].notes().collect::<Vec<_>>(), ["utils", "both"]);
    }

    #[test]
    fn empty_input_yields_empty_plan() {
        assert!(aggregate([]).is_empty());
    }

    #[test]
    fn file_with_no_entries_contributes_nothing() {
        let plan = aggregate([file("empty.md", vec![], "orphan note")]);
        assert!(plan.is_empty());
    }

    #[test]
    fn empty_note_is_still_retained() {
        let pkg = cargo("core");
        let plan = aggregate([file("blank.md", vec![(pkg.clone(), BumpLevel::Patch)], "")]);
        assert_eq!(plan[&pkg].notes().collect::<Vec<_>>(), [""]);
    }

    #[test]
    fn duplicate_entries_in_one_file_take_the_higher_level_once() {
        let pkg = cargo("core");
        let plan = aggregate([file(
            "dup.md",
            vec![
                (pkg.clone(), BumpLevel::Patch),
                (pkg.clone(), BumpLevel::Minor),
                (pkg.clone(), BumpLevel::Patch),
            ],
            "once",
        )]);
        let aggregated = &plan[&pkg];
        assert_eq!(aggregated.level(), BumpLevel::Minor);
        assert_eq!(aggregated.contributions().len(), 1);
        assert_eq!(aggregated.contributions()[0].level(), BumpLevel::Minor);
        assert_eq!(aggregated.notes().collect::<Vec<_>>(), ["once"]);
    }

    #[test]
    fn within_file_earlier_higher_level_beats_later_lower() {
        let pkg = cargo("core");
        let plan = aggregate([file(
            "dup.md",
            vec![
                (pkg.clone(), BumpLevel::Minor),
                (pkg.clone(), BumpLevel::Patch),
            ],
            "once",
        )]);
        let aggregated = &plan[&pkg];
        assert_eq!(aggregated.level(), BumpLevel::Minor);
        assert_eq!(aggregated.contributions().len(), 1);
        assert_eq!(aggregated.contributions()[0].level(), BumpLevel::Minor);
    }

    #[test]
    fn equal_levels_across_files_keep_every_note() {
        let pkg = cargo("core");
        let plan = aggregate([
            file("a.md", vec![(pkg.clone(), BumpLevel::Minor)], "one"),
            file("b.md", vec![(pkg.clone(), BumpLevel::Minor)], "two"),
        ]);
        let aggregated = &plan[&pkg];
        assert_eq!(aggregated.level(), BumpLevel::Minor);
        assert_eq!(aggregated.notes().collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(
            aggregated
                .contributions()
                .iter()
                .map(Contribution::level)
                .collect::<Vec<_>>(),
            [BumpLevel::Minor, BumpLevel::Minor]
        );
    }

    #[test]
    fn same_name_different_ecosystem_does_not_merge() {
        let crate_pkg = cargo("shared");
        let npm_pkg = npm("shared");
        let plan = aggregate([
            file(
                "crate.md",
                vec![(crate_pkg.clone(), BumpLevel::Major)],
                "cargo",
            ),
            file("js.md", vec![(npm_pkg.clone(), BumpLevel::Patch)], "npm"),
        ]);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[&crate_pkg].level(), BumpLevel::Major);
        assert_eq!(plan[&npm_pkg].level(), BumpLevel::Patch);
    }

    #[test]
    fn output_packages_are_ordered_by_package_id() {
        let plan = aggregate([
            file("z.md", vec![(cargo("z-last"), BumpLevel::Patch)], "z"),
            file("a.md", vec![(cargo("a-first"), BumpLevel::Patch)], "a"),
        ]);
        let names: Vec<_> = plan.keys().map(|id| id.name.as_str()).collect();
        assert_eq!(names, ["a-first", "z-last"]);
    }
}
