//! What a repository's existing tags say about the tag oakum should write.
//!
//! `migrate` arrives at a history another tool cut. Neither bumpy nor
//! changesets keeps a tag-format key, so nothing carries over from their
//! config, and oakum's own default is package-prefixed wherever more than one
//! package is tag-managed. A changesets or bumpy monorepo tags `name@version`,
//! and the mismatch surfaces at the first `release` — the last step of a
//! cutover — although the tags were readable when `migrate` ran
//! (`okm-404.19`).
//!
//! Derivation is structural. Each tag is split into a workspace package name,
//! a separator, an optional `v`, and a version; the template that split
//! implies is a candidate, and a shape is derived only when one candidate
//! explains every tag. Nothing here consults a list of known formats, so a
//! shape nobody enumerated still derives — subject to the one gate below.
//!
//! [ADR-0004]'s 2026-08-19 amendment puts existing tag shapes on the derived
//! side of the split; [ADR-0030] is the read rule this mirrors.
//!
//! Nothing here guesses. A history that derives nothing leaves `tag-format`
//! unset, and `release`'s refusal then names the exact mismatch — better than
//! a wrong tag.
//!
//! [ADR-0004]: ../../../../docs/decisions/0004-derive-facts-configure-preference.md
//! [ADR-0030]: ../../../../docs/decisions/0030-derive-read-tag-shapes.md

use std::collections::BTreeSet;

use oakum::plan::Workspace;
use semver::Version;

use super::owned_files::PrivatePackages;

const PACKAGE: &str = "{{ package }}";
const VERSION: &str = "{{ version }}";

/// `OakumConfig::tag_managed` without its `include`/`exclude` term: the config
/// `migrate` writes states neither, so every member is selected and
/// publishability decides. Computed from the workspace rather than passed
/// beside it, so the count and what it describes cannot disagree.
pub(super) fn tag_managed_count(
    workspace: Option<&Workspace>,
    private_packages: PrivatePackages,
) -> usize {
    workspace.map_or(0, |workspace| {
        workspace
            .packages()
            .filter(|package| package.publishable() || private_packages.tag)
            .count()
    })
}

/// The one readable shape that names no package, so the one whose readability
/// depends on how many packages could own a tag.
const BARE: &str = "v{{ version }}";

/// A `tag-format` oakum's own reader can parse back. The field is private to
/// this module and [`ReadableTemplate::lookup`] is the only way to make one, so
/// the readable set is a property of the value rather than a check some caller
/// might skip. Everything downstream — the config writer, the report — takes
/// this rather than a bare string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReadableTemplate(&'static str);

impl ReadableTemplate {
    fn lookup(template: &str) -> Option<Self> {
        READABLE
            .into_iter()
            .find(|shape| *shape == template)
            .map(Self)
    }

    pub(super) const fn as_str(self) -> &'static str {
        self.0
    }

    /// Whether this is the shape that names no package, which a workspace with
    /// more than one tag-managed package cannot use.
    const fn is_bare(self) -> bool {
        matches!(self.0.as_bytes(), b"v{{ version }}")
    }
}

/// The shapes oakum reads back ([ADR-0030]). `oakum::tags::prefixed_version`
/// accepts `@`, `/v` and `-v` after a package name and
/// `oakum::tags::bare_version` a leading `v` alone; a `tag-format` outside
/// that set would write tags the next release could not attribute. Growing the
/// reader is what grows this.
///
/// [ADR-0030]: ../../../../docs/decisions/0030-derive-read-tag-shapes.md
const READABLE: [&str; 4] = [
    "{{ package }}@{{ version }}",
    "{{ package }}/v{{ version }}",
    "{{ package }}-v{{ version }}",
    "v{{ version }}",
];

/// Bare drops out above one tag-managed package: [`derive`] refuses it there
/// and `release` reads such a tag as leftover ambiguity, so offering it would
/// name a shape the next command rejects.
fn offerable(tag_managed: usize) -> Vec<ReadableTemplate> {
    READABLE
        .into_iter()
        .map(ReadableTemplate)
        .filter(|shape| !shape.is_bare() || tag_managed <= 1)
        .collect()
}

/// What the existing tags settled, or why they settled nothing.
///
/// [`TagShape::Unread`] is the caller's to construct: reading the tags is git
/// I/O and this module is pure. It sits here so one `match` covers every
/// outcome a report has to distinguish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TagShape {
    /// No tags: nothing to derive from, and nothing worth saying.
    NoTags,
    /// The tags could not be read. Never "no tags" — that would collapse "we
    /// did not look" into "never released".
    Unread(String),
    /// Every tag renders from this template, and it is one oakum reads back.
    Derived(ReadableTemplate),
    /// Tags exist and derive nothing. `why` says so in the report's voice, and
    /// `offerable` is what this repository could adopt instead — settled here,
    /// where the tag-managed count that decides it is already known.
    Undecided {
        why: String,
        offerable: Vec<ReadableTemplate>,
    },
}

/// Every refusal carries the menu, so no construction can forget it.
fn undecided(why: String, tag_managed: usize) -> TagShape {
    TagShape::Undecided {
        why,
        offerable: offerable(tag_managed),
    }
}

/// The template every tag agrees on, if there is one.
///
/// `workspace` is `None` when discovery found no packages; only the
/// package-less shapes can derive then. Prefixed matching sees every member,
/// not only the tag-managed ones, the way [`oakum::tags`] does.
///
/// How many packages a bare tag could belong to decides whether that shape is
/// readable at all: ADR-0030 reads `v{semver}` only when exactly one
/// tag-managed package could own it, and with more the tag is leftover
/// ambiguity a release refuses.
pub(super) fn derive(
    tags: &[String],
    workspace: Option<&Workspace>,
    private_packages: PrivatePackages,
) -> TagShape {
    let tag_managed = tag_managed_count(workspace, private_packages);
    let Some(first) = tags.first() else {
        return TagShape::NoTags;
    };
    let names: Vec<&str> = workspace
        .map(|workspace| {
            workspace
                .packages()
                .map(|package| package.id().name.as_str())
                .collect()
        })
        .unwrap_or_default();

    let mut agreed = candidates(first, &names);
    if agreed.is_empty() {
        return undecided(unexplained(first), tag_managed);
    }
    for tag in &tags[1..] {
        let found = candidates(tag, &names);
        if found.is_empty() {
            return undecided(unexplained(tag), tag_managed);
        }
        agreed = &agreed & &found;
        if agreed.is_empty() {
            return undecided(
                format!("`{first}` and `{tag}` are not the same shape"),
                tag_managed,
            );
        }
    }

    let mut settled = agreed.iter();
    let (Some(template), None) = (settled.next(), settled.next()) else {
        return undecided(
            format!(
                "the existing tags fit more than one shape ({})",
                quoted(&agreed)
            ),
            tag_managed,
        );
    };
    if let Some(readable) = ReadableTemplate::lookup(template) {
        if readable.is_bare() && tag_managed > 1 {
            return undecided(
                format!(
                    "the existing tags are shaped `{BARE}`, which names no package; \
                     {tag_managed} packages are tag-managed here, so a release cannot tell \
                     which one such a tag belongs to"
                ),
                tag_managed,
            );
        }
        return TagShape::Derived(readable);
    }
    undecided(
        format!("the existing tags are shaped `{template}`, which oakum does not read"),
        tag_managed,
    )
}

fn unexplained(tag: &str) -> String {
    format!("no package name and version explain `{tag}`")
}

fn quoted(templates: &BTreeSet<String>) -> String {
    templates
        .iter()
        .map(|template| format!("`{template}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every template that renders `tag`. More than one is possible when two
/// package names both split it; the agreement across tags is what settles it.
fn candidates(tag: &str, names: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    found.extend(package_less(tag));
    for name in names {
        if let Some(rest) = tag.strip_prefix(name) {
            found.extend(prefixed(rest));
        }
    }
    found
}

/// A tag that is a version on its own, with or without the customary `v`.
fn package_less(tag: &str) -> Option<String> {
    if let Some(rest) = tag.strip_prefix('v') {
        if Version::parse(rest).is_ok() {
            return Some(format!("v{VERSION}"));
        }
    }
    Version::parse(tag).is_ok().then(|| VERSION.to_owned())
}

/// `rest` is what follows a package name. Each split of it into a separator
/// and a version yields one template.
///
/// A separator holds no alphanumerics, which is what keeps a sibling package's
/// name out of one (`foo` does not explain `foo-bar@1.0.0`) and keeps `-v` from
/// splitting two ways. Separators only grow, so the first alphanumeric ends the
/// search.
fn prefixed(rest: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (index, _) in rest.char_indices().skip(1) {
        let (separator, tail) = rest.split_at(index);
        if separator.chars().any(char::is_alphanumeric) {
            break;
        }
        if tail
            .strip_prefix('v')
            .is_some_and(|version| Version::parse(version).is_ok())
        {
            found.push(format!("{PACKAGE}{separator}v{VERSION}"));
        }
        if Version::parse(tail).is_ok() {
            found.push(format!("{PACKAGE}{separator}{VERSION}"));
        }
    }
    found
}

#[cfg(test)]
mod derivation {
    use oakum::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
    use semver::Version;

    use super::{derive, undecided, ReadableTemplate, TagShape, READABLE};
    use crate::cli::owned_files::PrivatePackages;
    use crate::cli::release::default_tag_template;

    /// `written_tag_format` suppresses a derived shape by comparing this
    /// module's `READABLE` strings against `release`'s own template literals.
    /// Two spellings of the same template — `{{package}}` for `{{ package }}`,
    /// which minijinja renders identically — would turn the suppression off
    /// silently and start writing a `tag-format` that restates the default.
    #[test]
    fn release_defaults_are_spelled_the_way_this_module_spells_them() {
        for managed in [0, 1, 2, 3] {
            assert!(
                ReadableTemplate::lookup(default_tag_template(managed)).is_some(),
                "`release`'s default for {managed} tag-managed package(s) \
                 is not one of this module's readable shapes: {}",
                default_tag_template(managed)
            );
        }
    }

    /// Fixture packages are unpublishable, so the tag axis is what makes them
    /// tag-managed — which is the shape this derivation is for.
    fn tag_all() -> PrivatePackages {
        PrivatePackages {
            version: true,
            tag: true,
        }
    }

    fn workspace(names: &[&str]) -> Workspace {
        let packages: Vec<Package> = names
            .iter()
            .map(|name| {
                Package::new(
                    PackageId::new(Ecosystem::Cargo, *name),
                    Version::new(0, 1, 0),
                    ResolvesDependenciesAt::Install,
                    true,
                    Vec::new(),
                )
            })
            .collect();
        Workspace::new(packages).expect("workspace")
    }

    /// Every package tag-managed, which is the ordinary case and the one that
    /// makes a bare shape ambiguous once there is more than one.
    fn shape(tags: &[&str], names: &[&str]) -> TagShape {
        let tags: Vec<String> = tags.iter().map(|tag| (*tag).to_owned()).collect();
        derive(&tags, Some(&workspace(names)), tag_all())
    }

    fn derived(tags: &[&str], names: &[&str]) -> &'static str {
        match shape(tags, names) {
            TagShape::Derived(template) => template.as_str(),
            other => panic!("expected a derived shape for {tags:?}, got {other:?}"),
        }
    }

    fn refused(tags: &[&str], names: &[&str]) -> String {
        match shape(tags, names) {
            TagShape::Undecided { why, .. } => why,
            other => panic!("expected a refusal for {tags:?}, got {other:?}"),
        }
    }

    /// ADR-0012's four formats, which are also the four oakum reads back.
    #[test]
    fn every_readable_shape_derives() {
        assert_eq!(
            derived(
                &["review-cycle@0.17.0", "prose@0.1.0"],
                &["review-cycle", "prose"]
            ),
            "{{ package }}@{{ version }}"
        );
        assert_eq!(
            derived(&["linesmith/v0.2.0"], &["linesmith"]),
            "{{ package }}/v{{ version }}"
        );
        assert_eq!(
            derived(&["linesmith-core-v0.1.3"], &["linesmith", "linesmith-core"]),
            "{{ package }}-v{{ version }}"
        );
        assert_eq!(derived(&["v0.1.0", "v0.2.0"], &["oakum"]), "v{{ version }}");
    }

    #[test]
    fn a_scoped_npm_name_is_one_package_name_not_a_separator() {
        let packages = vec![Package::new(
            PackageId::new(Ecosystem::Npm, "@jbabin91/mui-theme"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )];
        let workspace = Workspace::new(packages).expect("workspace");
        let tags = vec![String::from("@jbabin91/mui-theme@1.4.0")];
        assert_eq!(
            derive(&tags, Some(&workspace), tag_all()),
            TagShape::Derived(
                ReadableTemplate::lookup("{{ package }}@{{ version }}").expect("readable")
            )
        );
    }

    /// A prerelease is part of the version, not part of the separator.
    #[test]
    fn a_prerelease_tag_derives_the_same_shape() {
        assert_eq!(
            derived(&["oakum@0.2.0-rc.1", "oakum@0.1.0"], &["oakum"]),
            "{{ package }}@{{ version }}"
        );
    }

    /// The longer name is the one that splits; the shorter leaves a remainder
    /// with letters in it, and a separator holds none.
    #[test]
    fn a_sibling_prefix_name_does_not_explain_a_longer_package_tag() {
        assert_eq!(
            derived(&["linesmith-core@0.2.0"], &["linesmith", "linesmith-core"]),
            "{{ package }}@{{ version }}"
        );
    }

    /// linesmith's own history. Refusing is the point: `release` then names
    /// the mismatch against whatever the repository picks.
    #[test]
    fn tags_that_disagree_derive_nothing() {
        let why = refused(
            &["linesmith/v0.2.0", "linesmith-core-v0.1.3"],
            &["linesmith", "linesmith-core"],
        );
        assert_eq!(
            why,
            "`linesmith/v0.2.0` and `linesmith-core-v0.1.3` are not the same shape"
        );
    }

    #[test]
    fn one_unexplained_tag_derives_nothing() {
        for tag in ["nightly", "v1", "release-2024-01-01"] {
            let why = refused(&["oakum@0.1.0", tag], &["oakum"]);
            assert_eq!(why, format!("no package name and version explain `{tag}`"));
        }
    }

    /// Structurally sound, and oakum's reader has no production for it, so
    /// writing it would cut tags the next release could not attribute.
    #[test]
    fn a_shape_oakum_cannot_read_back_derives_nothing() {
        let why = refused(&["oakum_1.0.0", "oakum_1.1.0"], &["oakum"]);
        assert_eq!(
            why,
            "the existing tags are shaped `{{ package }}_{{ version }}`, which oakum does not read"
        );
        let slash_without_v = refused(&["oakum/1.0.0"], &["oakum"]);
        assert_eq!(
            slash_without_v,
            "the existing tags are shaped `{{ package }}/{{ version }}`, which oakum does not read"
        );
    }

    /// The other direction of the menu's one rule: a lone tag-managed package
    /// can own a bare tag, so the refusal offers every shape oakum reads.
    #[test]
    fn one_tag_managed_package_is_offered_the_bare_shape_too() {
        let tags = vec![String::from("oakum_1.0.0"), String::from("oakum_1.1.0")];
        let TagShape::Undecided { offerable, .. } =
            derive(&tags, Some(&workspace(&["oakum"])), tag_all())
        else {
            panic!("a shape oakum cannot read derives nothing");
        };
        assert!(
            offerable.iter().any(|shape| shape.is_bare()),
            "{offerable:?}"
        );
        assert_eq!(offerable.len(), READABLE.len());
    }

    /// `pkg-v1.0.0` splits as `-` plus a `v` prefix or as `-v` with none. Both
    /// render the same template, so it is one candidate, not an ambiguity.
    #[test]
    fn a_v_prefix_and_a_v_bearing_separator_are_one_template() {
        assert_eq!(
            derived(&["oakum-v1.0.0"], &["oakum"]),
            "{{ package }}-v{{ version }}"
        );
    }

    /// Not reachable through a Cargo or npm name, which is why the names here
    /// are contrived: two of them would have to split one tag at different
    /// punctuation. Refusing is still the answer if one ever appears.
    #[test]
    fn a_tag_two_package_names_split_differently_derives_nothing() {
        let why = refused(&["oakum.@1.0.0"], &["oakum", "oakum."]);
        assert_eq!(
            why,
            "the existing tags fit more than one shape (`{{ package }}.@{{ version }}`, `{{ package }}@{{ version }}`)"
        );
    }

    #[test]
    fn no_tags_is_not_a_refusal() {
        assert_eq!(
            derive(&[], Some(&workspace(&["oakum"])), tag_all()),
            TagShape::NoTags
        );
    }

    /// Discovery found nothing, so only the package-less shapes are available.
    #[test]
    fn without_a_workspace_only_package_less_shapes_derive() {
        let bare = vec![String::from("v0.1.0")];
        assert_eq!(
            derive(&bare, None, tag_all()),
            TagShape::Derived(ReadableTemplate::lookup("v{{ version }}").expect("readable"))
        );
        let prefixed = vec![String::from("oakum@0.1.0")];
        assert_eq!(
            derive(&prefixed, None, tag_all()),
            undecided(
                String::from("no package name and version explain `oakum@0.1.0`"),
                0,
            )
        );
    }

    /// ADR-0030 reads a bare tag only when one tag-managed package could own
    /// it. Deriving that shape for a workspace with more would write the very
    /// config the next release refuses as leftover ambiguity.
    #[test]
    fn a_bare_shape_is_undecided_when_more_than_one_package_is_tag_managed() {
        let tags = vec![String::from("v0.1.0"), String::from("v0.2.0")];
        let single = derive(&tags, Some(&workspace(&["alpha"])), tag_all());
        assert_eq!(
            single,
            TagShape::Derived(ReadableTemplate::lookup("v{{ version }}").expect("readable"))
        );
        let TagShape::Undecided { why, offerable } =
            derive(&tags, Some(&workspace(&["alpha", "beta"])), tag_all())
        else {
            panic!("two tag-managed packages cannot own one bare tag");
        };
        assert!(why.contains("names no package"), "{why}");
        assert!(why.contains("2 packages are tag-managed"), "{why}");
        // The same refusal must not then offer the shape it ruled out.
        assert!(
            !offerable.iter().any(|shape| shape.is_bare()),
            "a refusal naming bare as unusable cannot offer it: {offerable:?}"
        );
        assert_eq!(offerable.len(), READABLE.len() - 1);
    }

    /// A bare version with no `v` is a shape oakum will not write, and saying
    /// so beats silence.
    #[test]
    fn a_package_less_tag_without_the_v_derives_nothing() {
        let why = refused(&["0.1.0", "0.2.0"], &["oakum"]);
        assert_eq!(
            why,
            "the existing tags are shaped `{{ version }}`, which oakum does not read"
        );
    }
}
