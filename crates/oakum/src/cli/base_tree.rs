//! The base side's Cargo workspace, as `cargo metadata --no-deps` reads it
//! from the base tree written out to a temporary directory (ADR-0039).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oakum::discover::{discover_cargo_under, running_toolchain};
use oakum::plan::Workspace;

use super::git::{Commit, Git, Op, TreeFile};
use super::CliError;

/// The base's Cargo workspace, or `None` when no `Cargo.toml` sits at or
/// above `start` there. `start` is the repository-relative directory HEAD
/// discovery ran cargo from, so the same `.cargo/config.toml` files apply,
/// and cargo is the toolchain's own `rustc` resolves to there.
/// `head_members` are HEAD's Cargo members as `(directory, name)`.
///
/// # Errors
///
/// Unverified when the tree cannot be read without fetching, an entry oakum
/// cannot place (a submodule, a link out of the tree) may hold a member, it
/// cannot be written out, no toolchain is found, or cargo cannot read it.
pub(super) fn base_workspace(
    git: &Git,
    base: &Commit,
    start: &str,
    head_root: &Path,
    head_members: &[(&str, &str)],
) -> Result<Option<Workspace>, CliError> {
    // A root commit's parent names no commit: compared against nothing.
    let commit = format!("{}^{{commit}}", base.as_str());
    if !git.predicate(Op::RefExists { reference: &commit })? {
        return Ok(None);
    }
    let listed = git.tree_files(base)?;
    if let Some(file) = listed
        .iter()
        .find(|file| !file.exact && cargo_reads(&file.path))
    {
        return Err(CliError::unverified(format!(
            "unverified: `{}` at {} is not a UTF-8 path, so cargo's reading of the base is unknown",
            file.path,
            base.as_str()
        )));
    }
    let files: Vec<&TreeFile> = listed.iter().filter(|file| file.exact).collect();
    let by_path: BTreeMap<&str, &TreeFile> = files
        .iter()
        .map(|file| (file.path.as_str(), *file))
        .collect();
    let submodules: Vec<&str> = files
        .iter()
        .filter(|file| file.mode == "160000")
        .map(|file| file.path.as_str())
        .collect();
    let holds_member = |entry: &str| {
        head_members
            .iter()
            .any(|(dir, _)| within(dir, entry) || within(entry, dir))
    };
    if let Some(entry) = submodules.iter().find(|entry| holds_member(entry)) {
        return Err(unplaced(entry, base));
    }
    if !ancestors(start).any(|dir| by_path.contains_key(in_dir(dir, "Cargo.toml").as_str())) {
        return Ok(None);
    }
    let links: Vec<&TreeFile> = files
        .iter()
        .copied()
        .filter(|file| file.mode == "120000")
        .collect();
    let link_objects: Vec<&str> = links.iter().map(|file| file.object.as_str()).collect();
    let mut targets: BTreeMap<&str, String> = BTreeMap::new();
    for (link, bytes) in links.iter().zip(git.objects(&link_objects)?) {
        let bytes = bytes.ok_or_else(|| missing(&link.path, base))?;
        if let Ok(target) = String::from_utf8(bytes) {
            targets.insert(link.object.as_str(), target);
        }
    }
    let layout = Layout::of(&files, &by_path, &targets);
    if let Some(entry) = layout.skipped.iter().find(|entry| cargo_reads(entry)) {
        return Err(unplaced(entry, base));
    }
    let wanted: Vec<(&str, &str)> = layout
        .files
        .iter()
        .filter(|(path, _)| cargo_reads(path) || layout.read_through_link.contains(path))
        .map(|(path, file)| (path.as_str(), file.object.as_str()))
        .collect();
    let objects: Vec<&str> = wanted.iter().map(|(_, object)| *object).collect();
    let mut bytes: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    for ((path, _), read) in wanted.iter().zip(git.objects(&objects)?) {
        bytes.insert(path, read.ok_or_else(|| missing(path, base))?);
    }
    let tree = Scratch::new().map_err(|err| written(&err))?;
    // A pin the base names may be a toolchain not installed here; cargo is the
    // toolchain HEAD's discovery ran instead.
    for (path, _) in layout
        .files
        .iter()
        .filter(|(path, _)| !is_toolchain_pin(path))
    {
        let contents = bytes.get(path.as_str()).map_or(&[][..], Vec::as_slice);
        write(tree.path(), path, contents).map_err(|err| written(&err))?;
    }
    for (path, target) in &layout.links {
        link(tree.path(), path, target).map_err(|err| written(&err))?;
    }
    let Some(workspace) = read_with_cargo(&tree, base, start, head_root)? else {
        return Ok(None);
    };
    let skipped: Vec<&str> = submodules
        .iter()
        .copied()
        .chain(layout.skipped.iter().map(String::as_str))
        .collect();
    if let Some(entry) = skipped.iter().find(|entry| holds_member(entry)) {
        return Err(unplaced(entry, base));
    }
    let lock_crates = base_lock_crates(git, base, &workspace, &by_path)?;
    if let Some(entry) = skipped
        .first()
        .filter(|_| member_missing(&workspace, head_members, lock_crates.as_ref()))
    {
        return Err(unplaced(entry, base));
    }
    Ok(Some(workspace))
}

/// cargo's reading of the tree written to `tree`, run from `start` as the
/// cargo HEAD's discovery ran, found from where it ran; `None` for a
/// workspace with no members.
fn read_with_cargo(
    tree: &Scratch,
    base: &Commit,
    start: &str,
    head_root: &Path,
) -> Result<Option<Workspace>, CliError> {
    let start_dir = tree.path().join(start);
    std::fs::create_dir_all(&start_dir).map_err(|err| written(&err))?;
    let toolchain = running_toolchain(&head_root.join(start));
    discover_cargo_under(&start_dir, tree.path(), toolchain.as_ref()).map_err(|err| {
        CliError::unverified(format!(
            "unverified: cargo could not read the workspace at {}, so which rows the members shipped there is unknown: {}",
            base.as_str(),
            tree.relative(&err.to_string())
        ))
    })
}

/// An entry not written may have been a member cargo would have found, as a
/// `members` glob would: a member today the base lock holds as a workspace
/// crate but cargo did not list there. One the lock lacks is new; without a
/// lock to say what was a crate there, any member missing counts.
fn member_missing(
    workspace: &Workspace,
    head_members: &[(&str, &str)],
    lock_crates: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    head_members.iter().any(|(_, name)| {
        lock_crates.is_none_or(|crates| crates.contains(*name))
            && !workspace
                .packages()
                .any(|package| package.id().name == *name)
    })
}

/// The workspace's own crates in the base `Cargo.lock`: its rows without a
/// `source`. `None` when the base holds no lock there, or one oakum cannot
/// read.
fn base_lock_crates(
    git: &Git,
    base: &Commit,
    workspace: &Workspace,
    by_path: &BTreeMap<&str, &TreeFile>,
) -> Result<Option<std::collections::BTreeSet<String>>, CliError> {
    let path = in_dir(
        workspace.cargo_workspace_root().unwrap_or_default(),
        "Cargo.lock",
    );
    let Some(lock) = by_path
        .get(path.as_str())
        .filter(|file| file.mode != "120000")
    else {
        return Ok(None);
    };
    let bytes = git
        .objects(&[lock.object.as_str()])?
        .pop()
        .flatten()
        .ok_or_else(|| missing(&path, base))?;
    let Some(rows) = String::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|mut table| table.remove("package"))
        .and_then(|value| value.as_array().cloned())
    else {
        return Ok(None);
    };
    Ok(Some(
        rows.iter()
            .filter_map(toml::Value::as_table)
            .filter(|row| !row.contains_key("source"))
            .filter_map(|row| row.get("name")?.as_str().map(str::to_owned))
            .collect(),
    ))
}

fn unplaced(entry: &str, base: &Commit) -> CliError {
    CliError::unverified(format!(
        "unverified: `{entry}` at {} is a submodule or a link out of the tree, which oakum does not read, and a member may sit there, so which rows the members shipped there is unknown",
        base.as_str()
    ))
}

/// The base tree as written out: files, links kept as links where the
/// platform allows, and entries left out.
struct Layout<'t> {
    files: Vec<(String, &'t TreeFile)>,
    links: Vec<(String, String)>,
    /// Files a link with a name cargo reads leads to.
    read_through_link: Vec<String>,
    /// Entries not written, by the path cargo would reach them at.
    skipped: Vec<String>,
}

impl<'t> Layout<'t> {
    /// On unix a link inside the tree is written as a link, so cargo follows
    /// it as in a checkout; elsewhere what it leads to is copied under it.
    fn of(
        files: &[&'t TreeFile],
        by_path: &BTreeMap<&str, &'t TreeFile>,
        targets: &BTreeMap<&str, String>,
    ) -> Self {
        let mut layout = Self {
            files: Vec::new(),
            links: Vec::new(),
            read_through_link: Vec::new(),
            skipped: Vec::new(),
        };
        for file in files {
            match file.mode.as_str() {
                "100644" | "100755" => layout.files.push((file.path.clone(), *file)),
                "120000" => layout.link(file, files, by_path, targets),
                _ => {}
            }
        }
        layout
    }

    fn link(
        &mut self,
        file: &'t TreeFile,
        files: &[&'t TreeFile],
        by_path: &BTreeMap<&str, &'t TreeFile>,
        targets: &BTreeMap<&str, String>,
    ) {
        if cfg!(unix) {
            let dir = file.path.rsplit_once('/').map_or("", |(dir, _)| dir);
            let target = targets.get(file.object.as_str());
            let led = target
                .filter(|target| !target.starts_with('/'))
                .and_then(|target| physical(dir, target, by_path, targets, 40));
            let (Some(target), Some(led)) = (target, led) else {
                self.skipped.push(file.path.clone());
                return;
            };
            self.links.push((file.path.clone(), target.clone()));
            // cargo reads a file by the name it is reached at, so one a link
            // names as a manifest or config needs its content.
            let prefix = format!("{led}/");
            for inner in files {
                let reached = if inner.path == led {
                    file.path.clone()
                } else if let Some(rest) = inner.path.strip_prefix(&prefix) {
                    format!("{}/{rest}", file.path)
                } else {
                    continue;
                };
                if !cargo_reads(&reached) {
                    continue;
                }
                // An inner link reached under a name cargo reads leads on to
                // the file that needs the content.
                let content = if inner.mode == "120000" {
                    let dir = inner.path.rsplit_once('/').map_or("", |(dir, _)| dir);
                    targets
                        .get(inner.object.as_str())
                        .and_then(|target| physical(dir, target, by_path, targets, 40))
                } else {
                    Some(inner.path.clone())
                };
                match content {
                    Some(content) => self.read_through_link.push(content),
                    None => self.skipped.push(reached),
                }
            }
            return;
        }
        match resolve(&file.path, by_path, targets) {
            Some(Target::File(target)) => self.files.push((file.path.clone(), target)),
            Some(Target::Dir(dir)) => {
                let prefix = if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                };
                for inner in files.iter().filter(|inner| inner.path.starts_with(&prefix)) {
                    if let Some(Target::File(target)) = resolve(&inner.path, by_path, targets) {
                        let rest = &inner.path[prefix.len()..];
                        self.files.push((format!("{}/{rest}", file.path), target));
                    }
                }
            }
            None => self.skipped.push(file.path.clone()),
        }
    }
}

#[cfg(unix)]
fn link(root: &Path, path: &str, target: &str) -> std::io::Result<()> {
    let at = root.join(path);
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(target, at)
}

#[cfg(not(unix))]
fn link(_: &Path, _: &str, _: &str) -> std::io::Result<()> {
    Ok(())
}

fn missing(path: &str, base: &Commit) -> CliError {
    CliError::unverified(format!(
        "unverified: `{path}` at {} is not in this partial clone, and oakum reads the base without fetching, so which rows the members shipped there is unknown; a clone without `--filter` lets it read the base",
        base.as_str()
    ))
}

/// Content for what `cargo metadata --no-deps` reads: manifests, config, and
/// the TOML a config `include`s, which any `.toml` may be. Any other file only
/// has to exist, for target discovery.
fn cargo_reads(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        || path.ends_with(".cargo/config")
}

fn is_toolchain_pin(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "rust-toolchain" || name == "rust-toolchain.toml"
}

/// `path` is `dir` or under it.
fn within(path: &str, dir: &str) -> bool {
    path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

enum Target<'t> {
    File(&'t TreeFile),
    Dir(String),
}

/// What a path resolves to inside the tree; `None` for a link leaving the
/// tree, an absolute one, a missing target, or a chain too long to be
/// anything but a loop.
fn resolve<'t>(
    path: &str,
    by_path: &BTreeMap<&str, &'t TreeFile>,
    targets: &BTreeMap<&str, String>,
) -> Option<Target<'t>> {
    let mut at = path.to_owned();
    for _ in 0..40 {
        let Some(file) = by_path.get(at.as_str()).copied() else {
            let prefix = format!("{at}/");
            return by_path
                .range(prefix.as_str()..)
                .next()
                .filter(|(next, _)| next.starts_with(&prefix))
                .map(|_| Target::Dir(at));
        };
        if file.mode != "120000" {
            return Some(Target::File(file));
        }
        let target = targets.get(file.object.as_str())?;
        if target.starts_with('/') {
            return None;
        }
        let dir = at.rsplit_once('/').map_or("", |(dir, _)| dir);
        at = normalized(dir, target)?;
    }
    None
}

/// Where `target`, read from `dir`, leads on disk: each component that is a
/// link is followed before the next `..` applies, as the filesystem does.
/// `None` when it leaves the tree, is absolute, or links more than `depth`
/// deep.
fn physical(
    dir: &str,
    target: &str,
    by_path: &BTreeMap<&str, &TreeFile>,
    targets: &BTreeMap<&str, String>,
    depth: u8,
) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut parts: Vec<String> = dir
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => {
                parts.push(part.to_owned());
                let here = parts.join("/");
                if let Some(file) = by_path
                    .get(here.as_str())
                    .filter(|file| file.mode == "120000")
                {
                    let next = targets.get(file.object.as_str())?;
                    if next.starts_with('/') || depth == 0 {
                        return None;
                    }
                    parts.pop();
                    let resolved = physical(&parts.join("/"), next, by_path, targets, depth - 1)?;
                    parts = resolved
                        .split('/')
                        .filter(|part| !part.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
            }
        }
    }
    Some(parts.join("/"))
}

/// `dir`/`target` with `.` and `..` resolved; `None` when it climbs out.
fn normalized(dir: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn ancestors(start: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(start), |dir| {
        (!dir.is_empty()).then(|| dir.rsplit_once('/').map_or("", |(parent, _)| parent))
    })
}

fn in_dir(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn write(root: &Path, path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let target = root.join(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, bytes)
}

fn written(err: &std::io::Error) -> CliError {
    CliError::unverified(format!(
        "unverified: the base tree could not be written to a temporary directory for cargo to read: {err}"
    ))
}

/// A directory under the system temp dir, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        let parent = std::env::temp_dir();
        let mut attempt = 0_u32;
        loop {
            let dir = parent.join(format!("oakum-base-{}-{attempt}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => {
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// `text` with this directory's own paths, as given or canonical, made
    /// relative, so a message names the repository's files.
    fn relative(&self, text: &str) -> String {
        let canonical = std::fs::canonicalize(&self.0).unwrap_or_else(|_| self.0.clone());
        [canonical, self.0.clone()]
            .iter()
            .fold(text.to_owned(), |text, dir| {
                let dir = dir.to_string_lossy();
                text.replace(&format!("{dir}{}", std::path::MAIN_SEPARATOR), "")
                    .replace(dir.as_ref(), ".")
            })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// [`base_workspace`] over a repository `build` makes in a scratch directory,
/// with the pack count before and after, for `git`'s tests: only that module
/// may spawn git.
#[cfg(test)]
pub(super) fn base_workspace_in_scratch(
    head_members: &[(&str, &str)],
    build: impl FnOnce(&Path) -> (PathBuf, String),
) -> (Result<Option<Workspace>, CliError>, usize, usize) {
    let scratch = Scratch::new().expect("scratch");
    let (clone, base) = build(scratch.path());
    let packs = || {
        std::fs::read_dir(clone.join(".git/objects/pack"))
            .expect("packs")
            .count()
    };
    let before = packs();
    let base = Commit::from_log(&base).expect("base");
    let read = base_workspace(&Git::at(&clone), &base, "", &clone, head_members);
    (read, before, packs())
}

#[cfg(test)]
mod tests {
    use super::{ancestors, normalized, within};

    #[test]
    fn a_link_resolves_inside_the_tree_only() {
        assert_eq!(
            normalized("a/b", "../c/Cargo.toml").as_deref(),
            Some("a/c/Cargo.toml")
        );
        assert_eq!(normalized("a", "./x").as_deref(), Some("a/x"));
        assert_eq!(normalized("a", "../../x"), None);
    }

    #[test]
    fn within_stops_at_a_path_boundary() {
        assert!(within("vendor/lib", "vendor/lib"));
        assert!(within("vendor/lib/x", "vendor/lib"));
        assert!(!within("vendor/library", "vendor/lib"));
    }

    #[test]
    fn ancestors_end_at_the_root() {
        assert_eq!(ancestors("a/b").collect::<Vec<_>>(), ["a/b", "a", ""]);
        assert_eq!(ancestors("").collect::<Vec<_>>(), [""]);
    }
}
