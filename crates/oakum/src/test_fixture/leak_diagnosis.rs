//! What a failed fixture reclaim left behind and, on Linux, which processes
//! still hold a path inside it. One file for both guards: the integration
//! copy includes it with `#[path]`, since it names no process type and so
//! sits within `git_boundary.rs`'s rule.

use std::fmt::Write as _;
use std::path::Path;

/// Entries and holders a diagnosis names; the rest are counted.
const LISTED: usize = 20;

/// Entries walked before stopping, so a huge or still-growing tree cannot
/// delay the leak report it belongs to.
const WALKED: usize = 1_000;

/// Room for the program and its leading arguments.
#[cfg(target_os = "linux")]
const CMDLINE: usize = 200;

/// One line: the ledger reader splits entries on newlines and fields on tabs.
pub(crate) fn leak_diagnosis(container: &Path) -> String {
    // Processes first: the writer may exit while the tree is walked.
    let holders = holders(container);
    let left = leftover(container);
    one_line(&format!("{left}; {holders}"))
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn leftover(container: &Path) -> String {
    let mut found = Vec::new();
    let mut stopped = false;
    let mut pending = vec![container.to_path_buf()];
    'walk: while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                found.push(format!(
                    "{}/ (unreadable: {err})",
                    relative(container, &dir)
                ));
                continue;
            }
        };
        for entry in entries {
            if found.len() >= WALKED {
                stopped = true;
                break 'walk;
            }
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(err) => {
                    found.push(format!(
                        "{}/? (unreadable: {err})",
                        relative(container, &dir)
                    ));
                    continue;
                }
            };
            let name = relative(container, &path);
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    found.push(format!("{name}/"));
                    pending.push(path);
                }
                Ok(meta) if meta.is_file() => found.push(format!("{name} ({} B)", meta.len())),
                Ok(meta) if meta.is_symlink() => found.push(format!("{name} (symlink)")),
                Ok(_) => found.push(format!("{name} (special)")),
                Err(err) => found.push(format!("{name} (unreadable: {err})")),
            }
        }
    }
    if found.is_empty() {
        return String::from("left: nothing (the container is empty)");
    }
    found.sort();
    let count = if stopped {
        format!("at least {}, walk stopped", found.len())
    } else {
        found.len().to_string()
    };
    format!("left ({count}): {}", capped(found, ", "))
}

fn capped(items: Vec<String>, separator: &str) -> String {
    let total = items.len();
    let mut listing = items
        .into_iter()
        .take(LISTED)
        .collect::<Vec<_>>()
        .join(separator);
    if total > LISTED {
        let _ = write!(listing, "{separator}… {} more", total - LISTED);
    }
    listing
}

/// `/`-joined on every platform, so a diagnosis reads the same in each CI log.
fn relative(container: &Path, path: &Path) -> String {
    path.strip_prefix(container)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every pid whose cwd or open fd resolves inside the container, this process
/// included: a test thread or a leaked handle writes as this pid. A deleted
/// path reads back as `… (deleted)`, which only renames its last component,
/// so a prefix match still finds it.
#[cfg(target_os = "linux")]
fn holders(container: &Path) -> String {
    let target = match container.canonicalize() {
        Ok(target) => target,
        Err(err) => return format!("process scan: container did not resolve ({err})"),
    };
    let procs = match std::fs::read_dir("/proc") {
        Ok(procs) => procs,
        Err(err) => return format!("process scan: /proc unreadable ({err})"),
    };
    let (mut scanned, mut unreadable) = (0_usize, 0_usize);
    let mut held = Vec::new();
    for entry in procs.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        scanned += 1;
        let dir = entry.path();
        let mut hits = Vec::new();
        // A listable fd directory can still deny reading its links, so a
        // refused link marks the process unreadable rather than clean.
        let denied = std::cell::Cell::new(false);
        let inside = |link: &Path| match std::fs::read_link(link) {
            Ok(resolved) => resolved
                .starts_with(&target)
                .then(|| relative(&target, &resolved)),
            // Closed, or the process exited, mid-scan.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                denied.set(true);
                None
            }
        };
        if let Some(path) = inside(&dir.join("cwd")) {
            hits.push(format!("cwd {path}"));
        }
        match std::fs::read_dir(dir.join("fd")) {
            Ok(fds) => {
                for fd in fds.flatten() {
                    if let Some(path) = inside(&fd.path()) {
                        hits.push(format!("fd {} {path}", fd.file_name().to_string_lossy()));
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => denied.set(true),
        }
        if denied.get() {
            unreadable += 1;
        }
        if hits.is_empty() {
            continue;
        }
        let cmdline = std::fs::read(dir.join("cmdline")).map_or_else(
            |err| format!("<cmdline unreadable: {err}>"),
            |raw| {
                let joined = String::from_utf8_lossy(&raw)
                    .split('\0')
                    .filter(|arg| !arg.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                match joined.char_indices().nth(CMDLINE) {
                    Some((cut, _)) => format!("{}…", &joined[..cut]),
                    None => joined,
                }
            },
        );
        let parent = std::fs::read_to_string(dir.join("stat"))
            .ok()
            .and_then(|stat| parent_pid(&stat))
            .map_or_else(|| String::from("?"), |parent| parent.to_string());
        held.push(format!(
            "pid {pid} (ppid {parent}) `{cmdline}` holds {}",
            capped(hits, ", ")
        ));
    }
    if held.is_empty() {
        return format!(
            "process scan: none of {scanned} processes ({unreadable} unreadable) holds a path in the container"
        );
    }
    format!("process scan: {}", capped(held, "; "))
}

/// `comm` may hold spaces and parentheses, so fields count from the last `)`.
#[cfg(target_os = "linux")]
fn parent_pid(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn holders(_container: &Path) -> String {
    String::from("process scan: not supported on this platform")
}
