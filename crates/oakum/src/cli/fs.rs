//! Containment-safe filesystem primitives over a repository [`Dir`].
//!
//! Identity is the held capability, not the ambient path it was opened from.
//! Subprocess callers that still need a path must go through
//! [`super::repository::Repository::ambient_path`].

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::fs::{Dir, File, OpenOptions};

use super::CliError;

/// Repo-relative paths in CLI output use `/`, matching git. Replace the
/// platform separator, not a literal `\` in a Unix filename.
pub(super) fn repo_path_display(path: &Path) -> String {
    path.display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// The mark every staging name carries (the writer below interpolates it); a
/// hard kill between `create_new` and the rename leaves one behind, and
/// nothing sweeps it, so the commands name it instead.
const STAGING_MARK: &str = ".oakum-write.";

pub(super) fn is_staging_name(name: &str) -> bool {
    name.starts_with('.') && name.contains(STAGING_MARK)
}

/// Staging files left under `sub` by an interrupted write, as `sub/name`
/// (`name` alone for `.`).
///
/// # Errors
///
/// A listing that cannot be read; a missing `sub` is an empty list.
pub(super) fn stray_staging_files(
    dir: &Dir,
    sub: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(sub) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to read `{sub}`: {err}"
            ))));
        }
    };
    let mut strays = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| CliError::new(format!("failed to read `{sub}`: {err}")))?;
        // The writer only ever creates regular files; a directory or symlink
        // with the name is not oakum's to speak for.
        let is_file = entry.metadata().is_ok_and(|meta| meta.is_file());
        if let Some(name) = entry.file_name().to_str() {
            if is_file && is_staging_name(name) {
                strays.push(if sub == "." {
                    name.to_owned()
                } else {
                    format!("{sub}/{name}")
                });
            }
        }
    }
    strays.sort();
    Ok(strays)
}

/// The one line every command prints for a stray. An interrupted write and a
/// run still in progress leave the same file, so it states what was seen and
/// conditions the advice on no run being in progress.
pub(super) fn stray_staging_message(path: &str) -> String {
    format!("`{path}` is an oakum staging file; if no oakum run is in progress, remove it")
}

/// Print one line per stray staging file under `.changeset/`. Runs before a
/// command decides whether it has anything else to do, so the
/// already-initialized and already-migrated paths report them too.
///
/// # Errors
///
/// A listing that cannot be read.
pub(super) fn report_stray_staging(dir: &Dir) -> Result<(), Box<dyn std::error::Error>> {
    for path in stray_staging_files(dir, ".changeset")? {
        println!("{}", stray_staging_message(&path));
    }
    Ok(())
}

/// Replace `target` via a sibling temp file so rename stays on one filesystem
/// (no EXDEV across mounts). Staging uses `create_new` so a pre-existing
/// path cannot redirect the write. On collision, pick another name rather
/// than removing the entry; sweeping orphans would reintroduce that race.
pub(super) fn write_file_via_rename(
    dir: &Dir,
    target: &Path,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::new("write target has no file name"))?;
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut attempt: u32 = 0;
    let (tmp, mut staged) = loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let candidate = parent.join(format!(
            ".{file_name}{STAGING_MARK}{}.{nanos}.{attempt}",
            std::process::id()
        ));
        match dir.open_with(&candidate, OpenOptions::new().create_new(true).write(true)) {
            Ok(file) => break (candidate, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(err) => {
                return Err(Box::new(CliError::new(format!(
                    "failed to stage `{}`: {err}",
                    repo_path_display(target)
                ))));
            }
        }
    };
    staged.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!(
            "failed to stage `{}`: {err}",
            repo_path_display(target)
        ))
    })?;
    drop(staged);
    dir.rename(&tmp, dir, target).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!(
            "failed to replace `{}`: {err}",
            repo_path_display(target)
        ))
    })?;
    Ok(())
}

/// `create_new` so a file that appears between the check and the write is not replaced.
pub(super) fn write_file_exclusive(dir: &Dir, target: &Path, body: &str) -> io::Result<()> {
    let mut file = dir
        .open_with(target, OpenOptions::new().create_new(true).write(true))
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to create `{}`: {err}", repo_path_display(target)),
            )
        })?;
    file.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(target);
        io::Error::new(
            err.kind(),
            format!("failed to write `{}`: {err}", repo_path_display(target)),
        )
    })?;
    Ok(())
}

pub(super) fn open_read_only(dir: &Dir, path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    dir.open_with(path, &options)
}

/// `repo_path` is the discovery-time canonical prefix for absolute symlink
/// targets only; it is not reopened.
pub(super) fn resolve_capability_path(
    dir: &Dir,
    repo_path: &Path,
    path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut pending = relative_components(path)?;
    let mut resolved = PathBuf::new();
    let mut followed = 0;
    while let Some(component) = pending.pop_front() {
        match component {
            PendingComponent::Parent => {
                if !resolved.pop() {
                    return Err(outside_repository(path));
                }
            }
            PendingComponent::Normal(component) => {
                let candidate = resolved.join(&component);
                let metadata = dir.symlink_metadata(&candidate).map_err(|err| {
                    path_error(format!(
                        "failed to resolve `{}` within the repository: {err}",
                        repo_path_display(path)
                    ))
                })?;
                if !metadata.file_type().is_symlink() {
                    resolved.push(component);
                    continue;
                }
                followed += 1;
                if followed > 40 {
                    return Err(path_error(format!(
                        "`{}` contains too many symbolic links",
                        repo_path_display(path)
                    )));
                }
                let target = match dir.read_link_contents(&candidate) {
                    Ok(target) => target,
                    Err(err) => {
                        #[cfg(not(windows))]
                        {
                            return Err(path_error(format!(
                                "failed to resolve `{}` within the repository: {err}",
                                repo_path_display(path)
                            )));
                        }
                        #[cfg(windows)]
                        {
                            let _ = err;
                            match read_symlink_via_ambient(repo_path, &candidate) {
                                Ok(target) => target,
                                Err(_) => return Err(outside_repository(path)),
                            }
                        }
                    }
                };
                #[cfg(windows)]
                let target = win32_from_nt_symlink_target(&target);
                let target = if target.is_absolute() {
                    resolved.clear();
                    contained_absolute_target(repo_path, &target)
                        .ok_or_else(|| outside_repository(path))?
                } else {
                    target
                };
                let mut target_components = relative_components(&target)?;
                while let Some(target_component) = target_components.pop_back() {
                    pending.push_front(target_component);
                }
            }
        }
    }
    if resolved.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(resolved)
    }
}

enum PendingComponent {
    Parent,
    Normal(OsString),
}

fn relative_components(
    path: &Path,
) -> Result<VecDeque<PendingComponent>, Box<dyn std::error::Error>> {
    let mut components = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => {
                components.push_back(PendingComponent::Normal(component.to_owned()));
            }
            Component::CurDir => {}
            Component::ParentDir => components.push_back(PendingComponent::Parent),
            Component::RootDir | Component::Prefix(_) => return Err(outside_repository(path)),
        }
    }
    Ok(components)
}

/// cap-std's Windows `read_link` strips `\??\` and leaves `UNC\host\share`.
/// That spelling is not a Win32 UNC path, so `Path::is_absolute` is false
/// and the walk looks for a `UNC` directory. Restore the `\\` form.
#[cfg(any(windows, test))]
fn win32_from_nt_symlink_target(target: &Path) -> PathBuf {
    let text = target.to_string_lossy().replace('/', "\\");
    let win32 = if let Some(rest) = text.strip_prefix(r"\??\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\??\") {
        rest.to_owned()
    } else if let Some(rest) = text.strip_prefix(r"UNC\") {
        format!(r"\\{rest}")
    } else {
        text
    };
    PathBuf::from(win32)
}

/// cap-std's Windows `open` of a reparse point whose target is UNC fails with
/// `NotFound` (os error 2, GHA `windows-latest`). The link is still in the
/// repository; read its text via the ambient path so we can classify it.
#[cfg(windows)]
fn read_symlink_via_ambient(repo_path: &Path, candidate: &Path) -> io::Result<PathBuf> {
    std::fs::read_link(repo_path.join(candidate)).or_else(|_| {
        std::fs::read_link(PathBuf::from(normalized_windows_path(repo_path)).join(candidate))
    })
}

#[cfg(not(windows))]
fn contained_absolute_target(repo_path: &Path, target: &Path) -> Option<PathBuf> {
    fs::canonicalize(target)
        .ok()?
        .strip_prefix(repo_path)
        .ok()
        .map(Path::to_path_buf)
}

#[cfg(windows)]
fn contained_absolute_target(repo_path: &Path, target: &Path) -> Option<PathBuf> {
    let repo = normalized_windows_path(repo_path);
    let target = normalized_windows_path(&fs::canonicalize(target).ok()?);
    contained_windows_path(&repo, &target)
}

#[cfg(any(windows, test))]
fn contained_windows_path(repo: &str, target: &str) -> Option<PathBuf> {
    let prefix = target.get(..repo.len())?;
    if !prefix.eq_ignore_ascii_case(repo) {
        return None;
    }
    let remainder = target.get(repo.len()..)?;
    let repo_ends_with_separator = repo.ends_with('\\') || repo.ends_with('/');
    if !remainder.is_empty()
        && !repo_ends_with_separator
        && !remainder.starts_with('\\')
        && !remainder.starts_with('/')
    {
        return None;
    }
    Some(PathBuf::from(remainder.trim_start_matches(|character| {
        character == '\\' || character == '/'
    })))
}

/// Unix tests compile this so CI type-checks the Windows prefix strip.
#[cfg(any(windows, test))]
pub(super) fn normalized_windows_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('/', "\\");
    let path = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path
    };
    loopback_admin_share(&path).unwrap_or(path)
}

/// `\\localhost\C$\foo` is the same volume as `C:\foo`. A remote host's `C$`
/// is not: that would treat another machine's drive as this one.
#[cfg(any(windows, test))]
fn loopback_admin_share(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\")?;
    let mut parts = rest.splitn(3, '\\');
    let host = parts.next()?;
    let share = parts.next()?;
    let tail = parts.next();
    if !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1") {
        return None;
    }
    let bytes = share.as_bytes();
    if share.len() != 2 || bytes.get(1) != Some(&b'$') || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let drive = bytes[0].to_ascii_uppercase() as char;
    Some(match tail {
        Some(tail) if !tail.is_empty() => format!(r"{drive}:\{tail}"),
        _ => format!(r"{drive}:\"),
    })
}

fn outside_repository(path: &Path) -> Box<dyn std::error::Error> {
    path_error(format!(
        "`{}` resolves outside the repository",
        repo_path_display(path)
    ))
}

fn path_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(CliError::new(message))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        contained_windows_path, normalized_windows_path, repo_path_display,
        win32_from_nt_symlink_target,
    };

    #[test]
    fn repo_path_display_uses_forward_slashes() {
        assert_eq!(
            repo_path_display(&Path::new(".github/workflows").join("ci.yml")),
            ".github/workflows/ci.yml"
        );
    }

    #[cfg(windows)]
    #[test]
    fn repo_path_display_rewrites_a_backslash_separator() {
        assert_eq!(
            repo_path_display(Path::new(r".github\workflows\ci.yml")),
            ".github/workflows/ci.yml"
        );
    }

    #[test]
    fn windows_drive_root_contains_files() {
        assert_eq!(
            contained_windows_path(r"C:\", r"c:\config.toml"),
            Some(PathBuf::from("config.toml"))
        );
        assert_eq!(
            contained_windows_path(r"C:\repo", r"c:\repository\config.toml"),
            None
        );
    }

    #[test]
    fn path_component_case_is_ignored_for_windows_containment() {
        assert_eq!(
            contained_windows_path(r"C:\Users\repo", r"c:\users\REPO\.changeset\_config.toml"),
            Some(PathBuf::from(r".changeset\_config.toml"))
        );
        assert_eq!(
            contained_windows_path(r"C:\Users\repo", r"c:\users\repository\config.toml"),
            None
        );
    }

    #[test]
    fn verbatim_and_unc_prefixes_strip_to_the_drive_path() {
        assert_eq!(
            normalized_windows_path(Path::new(r"\\?\C:\repo\_config.toml")),
            r"C:\repo\_config.toml"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"\\?\UNC\localhost\C$\repo")),
            r"C:\repo"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"\\localhost\C$\repo\a")),
            r"C:\repo\a"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"\\127.0.0.1\c$\repo")),
            r"C:\repo"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"\\fileserver\C$\repo")),
            r"\\fileserver\C$\repo"
        );
        assert_eq!(
            normalized_windows_path(Path::new("C:/repo/a")),
            r"C:\repo\a"
        );
        assert_eq!(
            normalized_windows_path(Path::new("//localhost/C$/repo")),
            r"C:\repo"
        );
        assert_eq!(
            normalized_windows_path(Path::new("//?/C:/repo/a")),
            r"C:\repo\a"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"\\localhost\C$")),
            r"C:\"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"\\localhost\C$\")),
            r"C:\"
        );
    }

    #[test]
    fn nt_symlink_unc_becomes_win32_unc() {
        assert_eq!(
            win32_from_nt_symlink_target(Path::new(r"UNC\localhost\C$\repo")),
            PathBuf::from(r"\\localhost\C$\repo")
        );
        assert_eq!(
            win32_from_nt_symlink_target(Path::new(r"\??\UNC\localhost\C$\repo")),
            PathBuf::from(r"\\localhost\C$\repo")
        );
        assert_eq!(
            win32_from_nt_symlink_target(Path::new(r"\??\C:\repo")),
            PathBuf::from(r"C:\repo")
        );
        assert_eq!(
            win32_from_nt_symlink_target(Path::new(r"..\outside")),
            PathBuf::from(r"..\outside")
        );
    }

    #[test]
    fn a_loopback_unc_repo_contains_a_canonical_drive_target() {
        let repo = normalized_windows_path(Path::new(r"\\localhost\C$\repo"));
        assert_eq!(repo, r"C:\repo");
        assert_eq!(
            contained_windows_path(&repo, r"c:\repo\.changeset\_config.toml"),
            Some(PathBuf::from(r".changeset\_config.toml"))
        );
        assert_eq!(
            contained_windows_path(
                &repo,
                &normalized_windows_path(Path::new(r"\\?\C:\repo\.changeset\_config.toml"))
            ),
            Some(PathBuf::from(r".changeset\_config.toml"))
        );
        assert_eq!(
            contained_windows_path(&repo, r"D:\repo\.changeset\_config.toml"),
            None
        );
        assert_eq!(
            contained_windows_path(
                &normalized_windows_path(Path::new(r"\\fileserver\C$\repo")),
                r"C:\repo\.changeset\_config.toml"
            ),
            None
        );
    }

    #[test]
    fn staging_names_are_the_dot_prefixed_marked_ones() {
        use super::is_staging_name;
        assert!(is_staging_name(".one.md.oakum-write.123.456.0"));
        assert!(is_staging_name("._config.toml.oakum-write.1.2.3"));
        assert!(!is_staging_name("one.md"));
        assert!(!is_staging_name("oakum-write.md"));
        assert!(!is_staging_name("one.md.oakum-write.1.2.3"));
        assert!(!is_staging_name(".gitkeep"));
    }
}
