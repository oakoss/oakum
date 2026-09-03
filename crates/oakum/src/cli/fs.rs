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
            ".{file_name}.oakum-write.{}.{nanos}.{attempt}",
            std::process::id()
        ));
        match dir.open_with(&candidate, OpenOptions::new().create_new(true).write(true)) {
            Ok(file) => break (candidate, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempt < 16 => {
                attempt += 1;
            }
            Err(err) => {
                return Err(Box::new(CliError::new(format!(
                    "failed to stage `{file_name}`: {err}"
                ))));
            }
        }
    };
    staged.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!("failed to stage `{file_name}`: {err}"))
    })?;
    drop(staged);
    dir.rename(&tmp, dir, target).map_err(|err| {
        let _ = dir.remove_file(&tmp);
        CliError::new(format!("failed to replace `{file_name}`: {err}"))
    })?;
    Ok(())
}

/// `create_new` so a file that appears between the check and the write is not replaced.
pub(super) fn write_file_exclusive(dir: &Dir, target: &Path, body: &str) -> io::Result<()> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "write target has no file name")
        })?;
    let mut file = dir
        .open_with(target, OpenOptions::new().create_new(true).write(true))
        .map_err(|err| {
            io::Error::new(err.kind(), format!("failed to create `{file_name}`: {err}"))
        })?;
    file.write_all(body.as_bytes()).map_err(|err| {
        let _ = dir.remove_file(target);
        io::Error::new(err.kind(), format!("failed to write `{file_name}`: {err}"))
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
                        path.display()
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
                        path.display()
                    )));
                }
                let target = match dir.read_link_contents(&candidate) {
                    Ok(target) => target,
                    Err(err) => {
                        #[cfg(not(windows))]
                        {
                            return Err(path_error(format!(
                                "failed to resolve `{}` within the repository: {err}",
                                path.display()
                            )));
                        }
                        #[cfg(windows)]
                        {
                            read_symlink_via_ambient(repo_path, &candidate).map_err(|_| {
                                path_error(format!(
                                    "failed to resolve `{}` within the repository: {err}",
                                    path.display()
                                ))
                            })?
                        }
                    }
                };
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
        path.display()
    ))
}

fn path_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(CliError::new(message))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{contained_windows_path, normalized_windows_path};

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
}
