//! The releases dependency updates derive under `dependency-updates =
//! "release"` (ADR-0040): for each managed package, the updates in its range
//! read as one diff, at the level [`oakum::plan::update_level`] gives, with a
//! note naming what moved. Each update path counts whatever else reached the
//! package, so this reads the paths themselves, not coverage's strongest ask.

use std::collections::{BTreeMap, BTreeSet};

use oakum::manifest::{cargo_locked_versions, pnpm_locked_version};
use oakum::plan::{
    reach, render_update_note, update_level, DependencyUpdate, Ecosystem, NoteChange, NoteLine,
    Package, PackageId, UpdateReach, Workspace,
};
use semver::Version;

use super::dependency_updates::{followed, verdicts_for, HeadPackages, Scope};
use super::git::{Commit, Git, Op};
use super::root_files::{self, RootReach};
use super::CliError;

/// Whether a line names a declaration that moved or a version a lockfile
/// moved: the declared requirement wins where both name one dependency.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Said {
    Declared,
    Locked,
}

/// What one range's updates did to one package, with each note line's owner:
/// the package that declares or locks what moved.
#[derive(Default)]
struct Reached {
    reach: UpdateReach,
    lines: Vec<(String, Said, NoteLine)>,
}

impl Reached {
    fn add(&mut self, owner: &str, said: Said, line: NoteLine) {
        self.reach.reached = true;
        self.lines.push((owner.to_owned(), said, line));
    }

    /// A locked version gives way to the declared requirement its owner moved
    /// for the same dependency; a name moved by more than one owner is
    /// qualified by its owner.
    fn note(self) -> String {
        let declared: BTreeSet<(&str, &str)> = self
            .lines
            .iter()
            .filter(|(_, said, _)| *said == Said::Declared)
            .map(|(owner, _, line)| (owner.as_str(), line.name.as_str()))
            .collect();
        let kept: Vec<&(String, Said, NoteLine)> = self
            .lines
            .iter()
            .filter(|(owner, said, line)| {
                *said == Said::Declared || !declared.contains(&(owner.as_str(), line.name.as_str()))
            })
            .collect();
        let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (owner, _, line) in &kept {
            owners.entry(&line.name).or_default().insert(owner);
        }
        let lines: Vec<NoteLine> = kept
            .into_iter()
            .map(|(owner, _, line)| {
                let mut line = line.clone();
                if line.qualifier.is_none() && owners[line.name.as_str()].len() > 1 {
                    line.qualifier = Some(owner.clone());
                }
                line
            })
            .collect();
        render_update_note(&lines)
    }
}

/// The derived releases for the managed packages in `bases`, each read from
/// its base to `HEAD`. Packages sharing a base share one comparison.
///
/// # Errors
///
/// A side that cannot be read is `unverified`, as coverage reads it: an empty
/// plan there would say nothing moved when nothing was looked at.
pub(super) fn dependency_update_releases(
    git: &Git,
    workspace: &Workspace,
    managed: impl Fn(&Package) -> bool,
    bases: &BTreeMap<PackageId, String>,
) -> Result<Vec<DependencyUpdate>, CliError> {
    let mut by_base: BTreeMap<&str, BTreeSet<&PackageId>> = BTreeMap::new();
    for (id, base) in bases {
        by_base.entry(base.as_str()).or_default().insert(id);
    }
    let head = git.head()?;
    let mut out = Vec::new();
    for (base, ids) in by_base {
        let mut reached = reached_from(git, workspace, &managed, base, &head)?;
        for id in ids {
            let package = known(workspace, id)?;
            let Some(found) = reached.remove(id) else {
                continue;
            };
            if let Some(level) = update_level(package, found.reach) {
                out.push(DependencyUpdate {
                    package: id.clone(),
                    level,
                    note: found.note(),
                });
            }
        }
    }
    Ok(out)
}

/// Every id here was read off `workspace`, so one it lacks is a bug, said as
/// one rather than skipped.
fn known<'a>(workspace: &'a Workspace, id: &PackageId) -> Result<&'a Package, CliError> {
    workspace
        .get(id)
        .ok_or_else(|| CliError::new(format!("{id} is not in this workspace")))
}

fn reached_from(
    git: &Git,
    workspace: &Workspace,
    managed: &impl Fn(&Package) -> bool,
    base: &str,
    head: &Commit,
) -> Result<BTreeMap<PackageId, Reached>, CliError> {
    let paths = git.paths(Op::ChangedPaths { from: base })?;
    let mut out: BTreeMap<PackageId, Reached> = BTreeMap::new();
    if paths.is_empty() {
        return Ok(out);
    }
    let base = Commit::from_log(base)?;
    let root = if root_files::touched(workspace, &paths) {
        root_files::between_commits(git, workspace, &base, head, &paths)?
    } else {
        RootReach::default()
    };
    let read = |wanted: &[&str]| followed(git, head, wanted);
    let verdicts = verdicts_for(
        git,
        workspace,
        (&base, HeadPackages::Commit(head)),
        &paths,
        &root,
        &read,
        Scope::Release,
    )?;
    for (origin, moved) in verdicts.moved_entries() {
        let level = moved.level();
        for target in reach(workspace, [origin], managed).managed.into_keys() {
            let found = out.entry(target).or_default();
            found.reach.declared = found.reach.declared.max(Some(level));
            for line in moved.lines() {
                found.add(&origin.name, Said::Declared, line);
            }
        }
    }
    let hits = root.lock_hits();
    if hits.is_empty() {
        return Ok(out);
    }
    let locks = Locks::read(git, workspace, &base, head, &hits)?;
    for (origin, name) in &hits {
        let owner = known(workspace, origin)?;
        // A dependency locked on one side only came or went with its
        // declaration, whose own change it belongs to.
        let Some(change) = locks.change(workspace, owner, name)? else {
            continue;
        };
        for (target, via) in root_files::lock_targets(workspace, origin, managed) {
            // Past a managed library, the note names the library: what moved
            // below it is its own release's to name.
            let line = match &via {
                Some(library) => NoteLine {
                    name: library.name.clone(),
                    qualifier: None,
                    change: NoteChange::DependenciesUpdated,
                },
                None => NoteLine {
                    name: name.clone(),
                    qualifier: None,
                    change: change.clone(),
                },
            };
            out.entry(target)
                .or_default()
                .add(&origin.name, Said::Locked, line);
        }
    }
    Ok(out)
}

/// Both sides of the lockfiles a range's lock hits name, and the Cargo
/// members' versions on each side, which pick their rows.
struct Locks {
    cargo: [Option<String>; 2],
    pnpm: [Option<String>; 2],
    cargo_versions: [BTreeMap<PackageId, String>; 2],
}

impl Locks {
    fn read(
        git: &Git,
        workspace: &Workspace,
        base: &Commit,
        head: &Commit,
        hits: &[(PackageId, String)],
    ) -> Result<Self, CliError> {
        let wants = |ecosystem| hits.iter().any(|(id, _)| id.ecosystem == ecosystem);
        let both = |path: Option<String>| -> Result<[Option<String>; 2], CliError> {
            let Some(path) = path else {
                return Ok([None, None]);
            };
            let one =
                |at: &Commit| followed(git, at, &[path.as_str()]).map(|mut read| read.remove(0));
            Ok([one(base)?, one(head)?])
        };
        let versions = |at: &Commit| -> Result<BTreeMap<PackageId, String>, CliError> {
            Ok(root_files::declared_at(git, workspace, at)?
                .into_iter()
                .map(|(id, package)| (id, package.version().to_string()))
                .collect())
        };
        let (cargo, cargo_versions) = if wants(Ecosystem::Cargo) {
            let lock = workspace.cargo_root_files().map(|(_, lock)| lock);
            (both(lock)?, [versions(base)?, versions(head)?])
        } else {
            ([None, None], [BTreeMap::new(), BTreeMap::new()])
        };
        let pnpm = if wants(Ecosystem::Npm) {
            both(root_files::pnpm_lock_path(workspace))?
        } else {
            [None, None]
        };
        Ok(Self {
            cargo,
            pnpm,
            cargo_versions,
        })
    }

    /// How `owner` locked `name` from one side to the other: its versions
    /// when they differ, an update below it when they do not, and `None` when
    /// one side does not lock it for `owner`.
    fn change(
        &self,
        workspace: &Workspace,
        owner: &Package,
        name: &str,
    ) -> Result<Option<NoteChange>, CliError> {
        let unparsed = |why: String| {
            CliError::unverified(format!(
                "unverified: the lockfile does not parse on one side of the range, so what {name} moved from cannot be named: {why}"
            ))
        };
        let versions = match owner.id().ecosystem {
            Ecosystem::Cargo => {
                let at = |side: usize| -> Result<Vec<String>, CliError> {
                    let Some(version) = self.cargo_versions[side].get(owner.id()) else {
                        return Ok(Vec::new());
                    };
                    cargo_locked_versions(
                        self.cargo[side].as_deref(),
                        &owner.id().name,
                        version,
                        name,
                    )
                    .map_err(|err| unparsed(err.message))
                };
                let ([from, to], left_out) = paired_majors(&at(0)?, &at(1)?);
                // Nothing moved in the majors both sides lock, so the hit was
                // the one that came or went.
                if left_out && from == to {
                    return Ok(None);
                }
                [from, to]
            }
            Ecosystem::Npm => {
                let importer = root_files::pnpm_importer(workspace, owner).ok_or_else(|| {
                    CliError::new(format!(
                        "{} is not under the pnpm lockfile's directory",
                        owner.id()
                    ))
                })?;
                // pnpm writes a version's peers after it, `1.2.3(react@18)`.
                let at = |side: usize| {
                    pnpm_locked_version(self.pnpm[side].as_deref(), &importer, name)
                        .map(|version| {
                            version.map(|text| {
                                text.split('(')
                                    .next()
                                    .unwrap_or(&text)
                                    .trim_end()
                                    .to_owned()
                            })
                        })
                        .map_err(unparsed)
                };
                [at(0)?, at(1)?]
            }
        };
        Ok(match versions {
            [Some(from), Some(to)] if from == to => Some(NoteChange::DependenciesUpdated),
            [Some(from), Some(to)] => Some(NoteChange::Moved { from, to }),
            _ => None,
        })
    }
}

/// An owner taking two majors of a crate locks both, and which majors it
/// takes changes only with its declarations, so each major is read only where
/// both sides lock it. One major on each side is one declaration, read whole,
/// so a lock moving across a major (`>=0.3`) still reads. Each side as one
/// list, `None` when empty, beside whether a major was left out.
fn paired_majors(before: &[String], after: &[String]) -> ([Option<String>; 2], bool) {
    let major = |version: &String| match Version::parse(version) {
        Ok(parsed) if parsed.major > 0 => format!("{}", parsed.major),
        Ok(parsed) if parsed.minor > 0 => format!("0.{}", parsed.minor),
        Ok(parsed) => format!("0.0.{}", parsed.patch),
        Err(_) => version.clone(),
    };
    let majors = |side: &[String]| side.iter().map(major).collect::<BTreeSet<_>>();
    let (was, now) = (majors(before), majors(after));
    let left_out = was != now && !(was.len() == 1 && now.len() == 1);
    let both: BTreeSet<String> = if left_out {
        was.intersection(&now).cloned().collect()
    } else {
        was.union(&now).cloned().collect()
    };
    let kept = |side: &[String]| {
        let kept: Vec<&str> = side
            .iter()
            .filter(|version| both.contains(&major(version)))
            .map(String::as_str)
            .collect();
        (!kept.is_empty()).then(|| kept.join(", "))
    };
    ([kept(before), kept(after)], left_out)
}

#[cfg(test)]
mod tests {
    use super::paired_majors;

    fn read(before: &[&str], after: &[&str]) -> ([Option<String>; 2], bool) {
        let owned = |side: &[&str]| side.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        paired_majors(&owned(before), &owned(after))
    }

    #[test]
    fn a_major_on_one_side_only_is_left_out() {
        let both = |from: &str, to: &str| [Some(from.to_owned()), Some(to.to_owned())];
        assert_eq!(
            read(&["0.4.34"], &["0.4.35"]),
            (both("0.4.34", "0.4.35"), false)
        );
        assert_eq!(
            read(&["0.4.34"], &["0.3.9", "0.4.34"]),
            (both("0.4.34", "0.4.34"), true)
        );
        assert_eq!(
            read(&["0.3.9", "0.4.34"], &["0.4.35"]),
            (both("0.4.34", "0.4.35"), true)
        );
        assert_eq!(
            read(&["0.3.9", "0.4.34"], &["0.3.9", "0.4.35"]),
            (both("0.3.9, 0.4.34", "0.3.9, 0.4.35"), false)
        );
        assert_eq!(
            read(&["0.3.9"], &["0.4.34"]),
            (both("0.3.9", "0.4.34"), false)
        );
        assert_eq!(
            read(&["0.3.9", "0.4.34"], &["0.4.34", "0.5.0"]),
            (both("0.4.34", "0.4.34"), true)
        );
        assert_eq!(read(&[], &["1.0.0"]), ([None, None], true));
    }
}
