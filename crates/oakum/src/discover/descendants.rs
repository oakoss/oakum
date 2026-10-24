//! Killing a child together with what it started.
//!
//! A child past its deadline is often waiting on something it spawned: git on
//! a signing program or a credential helper, a wrapper on the real tool. Killing
//! the child alone leaves those running. The child stays in oakum's process
//! group, because moved into its own a program reading `/dev/tty` is stopped by
//! the terminal (measured: `ps` state `T`) where today the user can answer it,
//! so the kill walks the process tree instead. Anything whose parent had
//! already exited is no longer under the child and is not reached.

use std::process::Child;

/// What a deadline kill reached beyond the child itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Every process a listing found under the child, and a later one still
    /// found there, was stopped, then killed; the names of those still running,
    /// in the order found, each once. Empty when the child had started nothing.
    Killed(Vec<String>),
    /// The tree was killed whole without listing it (Windows `taskkill /T`).
    Unnamed,
    /// Listing or killing failed, so something the child started may still be
    /// running; the reason, for the message.
    Unknown(String),
}

impl Reach {
    /// The kill as a clause: `killed git and what was still running under it
    /// (`gpg`, `sleep`)`.
    #[must_use]
    pub fn describe(&self, child: &str) -> String {
        match self {
            Self::Killed(names) if names.is_empty() => format!("killed {child}"),
            Self::Killed(names) => {
                let names: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
                format!(
                    "killed {child} and what was still running under it ({})",
                    names.join(", ")
                )
            }
            Self::Unnamed => format!("killed {child} and what was still running under it"),
            Self::Unknown(why) => {
                format!("killed {child}, but what it started may still be running ({why})")
            }
        }
    }
}

/// Kills `child` and everything under it, then reaps `child`.
#[cfg(unix)]
#[must_use]
pub fn kill_tree(child: &mut Child) -> Reach {
    kill_tree_with(child, table, |pid| {
        signal(pid, rustix::process::Signal::STOP)
    })
}

/// Listing more often than this means the tree keeps growing faster than it
/// can be stopped.
#[cfg(unix)]
const ROUNDS: usize = 5;

/// Stops the child, then each process under it, listing again until a round
/// finds nothing new: a stopped process can neither fork nor exit, so nothing
/// escapes between the listing and the kill.
///
/// Only a process a later listing still finds under the child is killed, and
/// the reach is `Unknown` whenever the kill cannot vouch for the tree: a
/// process that exited before its stop ran unwatched and may have started
/// something; one that would not stop; one alive under another parent
/// (orphaned by an exit before its parent's stop, or its pid reused), which is
/// continued rather than killed; and everything stopped after the last listing
/// that held, continued the same way.
#[cfg(unix)]
fn kill_tree_with(
    child: &mut Child,
    list: impl Fn() -> Result<Vec<Proc>, String>,
    stop: impl Fn(u32) -> Option<rustix::io::Result<()>>,
) -> Reach {
    use rustix::process::Signal;

    let root = child.id();
    signal(root, Signal::STOP);
    let mut frozen: Vec<Proc> = Vec::new();
    let mut confirmed = 0;
    let mut released: Vec<Proc> = Vec::new();
    let mut exited: Vec<Proc> = Vec::new();
    let mut unstopped: Vec<String> = Vec::new();
    let settled = (|| {
        for _ in 0..ROUNDS {
            let table = list()?;
            if !table
                .iter()
                .any(|proc| proc.pid == root && proc.ppid == std::process::id())
            {
                return Err(String::from("the listing did not include it"));
            }
            let tree = under(&table, root);
            frozen.retain(|old| {
                if tree.iter().any(|now| now.pid == old.pid && !now.zombie) {
                    return true;
                }
                if table.iter().any(|now| now.pid == old.pid && !now.zombie) {
                    released.push(old.clone());
                } else {
                    exited.push(old.clone());
                }
                false
            });
            confirmed = frozen.len();
            let new: Vec<Proc> = tree
                .iter()
                .filter(|proc| !proc.zombie && !frozen.iter().any(|seen| seen.pid == proc.pid))
                .cloned()
                .collect();
            if new.is_empty() {
                return Ok(());
            }
            for proc in new {
                match stop(proc.pid) {
                    Some(Ok(())) => {}
                    // A pid past `i32::MAX` names no process, as a gone one does.
                    Some(Err(rustix::io::Errno::SRCH)) | None => exited.push(proc.clone()),
                    Some(Err(err)) => {
                        unstopped.push(format!(
                            "could not stop `{}` ({}): {err}",
                            proc.name, proc.pid
                        ));
                    }
                }
                frozen.push(proc);
            }
        }
        Err(format!(
            "it was still starting processes after {ROUNDS} listings"
        ))
    })();
    released.extend(frozen.drain(confirmed..));
    // Exited ones too: a listing that misreads a live process as gone must
    // not leave it stopped.
    for proc in released.iter().chain(&exited) {
        signal(proc.pid, Signal::CONT);
    }
    // Leaves before parents and the child last: a stopped process left in a
    // process group its parent's death orphans is continued by the kernel,
    // and could fork before its own kill.
    let killed = kill_each(&frozen);
    let _ = child.kill();
    let _ = child.wait();
    let mut why: Vec<String> = Vec::new();
    if let Err(err) = settled {
        why.push(format!("could not list them: {err}"));
    }
    if !exited.is_empty() {
        why.push(format!(
            "{} exited before oakum could stop it, so what it started then may still be running",
            named(&exited)
        ));
    }
    why.extend(unstopped);
    if !released.is_empty() {
        why.push(format!(
            "oakum could not confirm {} was still under it",
            named(&released)
        ));
    }
    match killed {
        Reach::Unknown(also) => why.push(also),
        killed if why.is_empty() => return killed,
        _ => {}
    }
    Reach::Unknown(why.join("; "))
}

/// `sh`, `sleep` once each, or `a process` when none has a name.
#[cfg(unix)]
fn named(procs: &[Proc]) -> String {
    let mut names: Vec<String> = Vec::new();
    for proc in procs {
        let name = format!("`{}`", proc.name);
        if !proc.name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        String::from("a process")
    } else {
        names.join(", ")
    }
}

#[cfg(unix)]
fn signal(pid: u32, sig: rustix::process::Signal) -> Option<rustix::io::Result<()>> {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .map(|pid| rustix::process::kill_process(pid, sig))
}

/// Kills `child` and everything still under it, then reaps `child`.
#[cfg(windows)]
#[must_use]
pub fn kill_tree(child: &mut Child) -> Reach {
    let reach = taskkill(child.id());
    let _ = child.kill();
    let _ = child.wait();
    reach
}

/// `taskkill` by full path: the child may have changed `PATH`, and a
/// repository-local `taskkill.exe` must not run.
#[cfg(windows)]
fn taskkill(pid: u32) -> Reach {
    let Some(root) = std::env::var_os("SystemRoot") else {
        return Reach::Unknown(String::from(
            "SystemRoot is not set, so taskkill could not be found",
        ));
    };
    let program = std::path::Path::new(&root)
        .join("System32")
        .join("taskkill.exe");
    match std::process::Command::new(&program)
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => Reach::Unnamed,
        Ok(output) => Reach::Unknown(format!(
            "taskkill exited {}: {}",
            output.status,
            super::child::complaint(&output)
        )),
        Err(err) => Reach::Unknown(format!("could not run taskkill: {err}")),
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Proc {
    pid: u32,
    ppid: u32,
    zombie: bool,
    name: String,
}

/// The processes under `root`, parents before children.
#[cfg(unix)]
fn under(table: &[Proc], root: u32) -> Vec<Proc> {
    let mut found: Vec<Proc> = Vec::new();
    let mut frontier = Vec::from([root]);
    while let Some(parent) = frontier.pop() {
        for proc in table
            .iter()
            .filter(|proc| proc.ppid == parent && proc.pid != root)
        {
            if !found.iter().any(|seen| seen.pid == proc.pid) {
                frontier.push(proc.pid);
                found.push(proc.clone());
            }
        }
    }
    found
}

/// Kills `procs` last to first, so children go before the parents listed ahead
/// of them, and names them in listed order. A process gone by the time of its
/// kill was not running, so it goes unnamed.
#[cfg(unix)]
fn kill_each(procs: &[Proc]) -> Reach {
    let mut taken: Vec<&Proc> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for proc in procs.iter().rev() {
        match signal(proc.pid, rustix::process::Signal::KILL) {
            Some(Ok(())) => taken.push(proc),
            Some(Err(rustix::io::Errno::SRCH)) | None => {}
            Some(Err(err)) => failed.push(format!("`{}` ({}): {err}", proc.name, proc.pid)),
        }
    }
    let mut names: Vec<String> = Vec::new();
    for proc in taken.into_iter().rev() {
        if !proc.name.is_empty() && !names.contains(&proc.name) {
            names.push(proc.name.clone());
        }
    }
    if failed.is_empty() {
        Reach::Killed(names)
    } else {
        Reach::Unknown(format!("could not kill {}", failed.join(", ")))
    }
}

#[cfg(unix)]
fn table() -> Result<Vec<Proc>, String> {
    #[cfg(target_os = "linux")]
    if let Some(table) = proc_table() {
        return table;
    }
    ps_table()
}

/// Linux without spawning anything. `None` when `/proc` cannot be read at all;
/// a process that exits mid-read is skipped, and any other unreadable entry
/// fails the listing rather than hiding a process.
#[cfg(target_os = "linux")]
fn proc_table() -> Option<Result<Vec<Proc>, String>> {
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut table = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => return Some(Err(format!("could not read /proc: {err}"))),
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        match std::fs::read(entry.path().join("stat")) {
            Ok(stat) => {
                if let Some(proc) = parse_stat(pid, &String::from_utf8_lossy(&stat)) {
                    table.push(proc);
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    || err.raw_os_error() == Some(libc::ESRCH) => {}
            Err(err) => return Some(Err(format!("could not read /proc/{pid}/stat: {err}"))),
        }
    }
    Some(Ok(table))
}

/// `pid (comm) state ppid ...`. The name can hold spaces and parentheses, so
/// it runs to the last `)`.
#[cfg(any(target_os = "linux", all(test, unix)))]
fn parse_stat(pid: u32, stat: &str) -> Option<Proc> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let mut fields = stat.get(close + 1..)?.split_whitespace();
    let state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    Some(Proc {
        pid,
        ppid: parent,
        zombie: state == "Z",
        name,
    })
}

/// A `ps` that never answers would leave the deadline unbounded.
#[cfg(unix)]
const PS_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// `ps` by full path, for the reason `taskkill` is: one earlier on `PATH`
/// could hide a process from the kill.
#[cfg(unix)]
fn ps_table() -> Result<Vec<Proc>, String> {
    use std::io::Read;

    let program = ["/bin/ps", "/usr/bin/ps"]
        .into_iter()
        .map(std::path::Path::new)
        .find(|path| path.is_file())
        .ok_or_else(|| String::from("no ps at /bin/ps or /usr/bin/ps"))?;
    let mut ps = std::process::Command::new(program)
        .args([
            "-A", "-o", "pid=", "-o", "ppid=", "-o", "stat=", "-o", "comm=",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("could not run ps: {err}"))?;
    let mut stdout = ps.stdout.take().expect("stdout was piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    // A panic here would abort with the tree still stopped.
    let reader = std::thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });
    if let Err(err) = reader {
        let _ = ps.kill();
        let _ = ps.wait();
        return Err(format!("could not read ps: {err}"));
    }
    let read = receiver.recv_timeout(PS_LIMIT);
    let Ok(Ok(bytes)) = read else {
        let _ = ps.kill();
        let _ = ps.wait();
        return Err(match read {
            Ok(Err(err)) => format!("could not read ps: {err}"),
            _ => format!("ps gave no answer within {}s", PS_LIMIT.as_secs()),
        });
    };
    match ps.wait() {
        Ok(status) if status.success() => Ok(parse_ps(&String::from_utf8_lossy(&bytes))),
        Ok(status) => Err(format!("ps exited {status}")),
        Err(err) => Err(format!("could not wait on ps: {err}")),
    }
}

/// `pid ppid stat comm` per line. macOS prints `comm` as the executable's path,
/// so the name is its last component; a path can hold spaces, so `comm` is the
/// rest of the line.
#[cfg(unix)]
fn parse_ps(listing: &str) -> Vec<Proc> {
    listing
        .lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut field = || {
                let (head, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                rest = tail.trim_start();
                head
            };
            let own = field().parse().ok()?;
            let parent = field().parse().ok()?;
            let stat = field();
            let comm = rest.trim();
            Some(Proc {
                pid: own,
                ppid: parent,
                zombie: stat.starts_with('Z'),
                name: comm.rsplit('/').next().unwrap_or(comm).to_owned(),
            })
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Stdio};

    use super::{kill_tree_with, parse_ps, parse_stat, under, Proc, Reach};

    fn proc(own: u32, parent: u32, name: &str) -> Proc {
        Proc {
            pid: own,
            ppid: parent,
            zombie: false,
            name: name.to_owned(),
        }
    }

    #[test]
    fn under_walks_the_whole_tree_and_nothing_beside_it() {
        let table = [
            proc(1, 0, "init"),
            proc(10, 1, "oakum"),
            proc(11, 10, "pnpm"),
            proc(12, 11, "gpg"),
            proc(13, 12, "pinentry"),
            proc(14, 10, "other"),
            proc(15, 1, "orphan"),
        ];
        let names: Vec<String> = under(&table, 11)
            .into_iter()
            .map(|proc| proc.name)
            .collect();
        assert_eq!(names, ["gpg", "pinentry"]);
        assert_eq!(under(&table, 13), []);
    }

    #[test]
    fn a_stat_name_runs_to_the_last_paren() {
        assert_eq!(
            parse_stat(42, "42 (a (b) c) S 7 42 42 0 -1"),
            Some(proc(42, 7, "a (b) c"))
        );
        assert_eq!(
            parse_stat(43, "43 (sh) Z 7 43").map(|proc| proc.zombie),
            Some(true)
        );
        assert_eq!(parse_stat(42, "42 (cut"), None);
    }

    #[test]
    fn a_ps_line_keeps_a_path_with_spaces_and_names_its_last_part() {
        let listing = "  101     1 Ss   /bin/sh\n  102   101 S+   /Applications/My Tool.app/x/gpg\n bad line\n  103   102 Z    <defunct>\n";
        let mut zombie = proc(103, 102, "<defunct>");
        zombie.zombie = true;
        assert_eq!(
            parse_ps(listing),
            [proc(101, 1, "sh"), proc(102, 101, "gpg"), zombie]
        );
    }

    #[test]
    fn describe_names_what_the_kill_reached() {
        assert_eq!(Reach::Killed(Vec::new()).describe("pnpm"), "killed pnpm");
        assert_eq!(
            Reach::Killed(Vec::from([String::from("sh"), String::from("sleep")])).describe("pnpm"),
            "killed pnpm and what was still running under it (`sh`, `sleep`)"
        );
        assert_eq!(
            Reach::Unnamed.describe("pnpm"),
            "killed pnpm and what was still running under it"
        );
        assert_eq!(
            Reach::Unknown(String::from("could not list them: no ps")).describe("it"),
            "killed it, but what it started may still be running (could not list them: no ps)"
        );
    }

    fn sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .expect("sleep")
    }

    fn stop(pid: u32) -> Option<rustix::io::Result<()>> {
        super::signal(pid, rustix::process::Signal::STOP)
    }

    /// A process that refuses its stop is not vouched for, even when it is
    /// killed afterwards.
    #[test]
    fn a_process_that_will_not_stop_is_unknown() {
        let mut child = sleeper();
        let mut other = sleeper();
        let root = child.id();
        let refused = other.id();
        let reach = kill_tree_with(
            &mut child,
            || {
                Ok(Vec::from([
                    proc(root, std::process::id(), "sleep"),
                    proc(refused, root, "sudo"),
                ]))
            },
            |pid| {
                if pid == refused {
                    Some(Err(rustix::io::Errno::PERM))
                } else {
                    stop(pid)
                }
            },
        );
        let _ = other.kill();
        let _ = other.wait();
        assert_eq!(
            reach,
            Reach::Unknown(format!(
                "could not stop `sudo` ({refused}): {}",
                rustix::io::Errno::PERM
            ))
        );
    }

    /// A listing that cannot be read, or that does not hold the child, says
    /// nothing about what the child started.
    #[test]
    fn a_listing_that_cannot_vouch_for_the_tree_is_unknown() {
        let mut child = sleeper();
        let reach = kill_tree_with(&mut child, || Err(String::from("no ps")), stop);
        assert_eq!(
            reach,
            Reach::Unknown(String::from("could not list them: no ps"))
        );

        let mut child = sleeper();
        let reach = kill_tree_with(&mut child, || Ok(Vec::from([proc(1, 0, "init")])), stop);
        assert_eq!(
            reach,
            Reach::Unknown(String::from(
                "could not list them: the listing did not include it"
            ))
        );
    }

    /// A tree that grows on every listing is never claimed killed whole.
    #[test]
    fn a_tree_that_keeps_growing_is_unknown() {
        let mut child = sleeper();
        let root = child.id();
        // Past `i32::MAX`, so no signal is ever sent to a real process.
        let next = std::cell::Cell::new(3_000_000_000_u32);
        let reach = kill_tree_with(
            &mut child,
            || {
                next.set(next.get() + 1);
                Ok(Vec::from([
                    proc(root, std::process::id(), "sleep"),
                    proc(next.get(), root, "fork"),
                ]))
            },
            stop,
        );
        // Each fake names no process, so reads as exited, and the last is never
        // confirmed.
        assert_eq!(
            reach,
            Reach::Unknown(String::from(
                "could not list them: it was still starting processes after 5 listings; \
                 `fork` exited before oakum could stop it, so what it started then may \
                 still be running; oakum could not confirm `fork` was still under it"
            ))
        );
    }

    /// A stopped process a later listing finds under another parent (orphaned,
    /// or its pid reused) or reads as gone is continued, not killed, and
    /// reported. A real process stands in, so a missing continue shows as `T`.
    #[test]
    fn a_pid_that_leaves_the_tree_is_continued_and_reported() {
        for (zombie, expected) in [
            (false, "oakum could not confirm `sh` was still under it"),
            (
                true,
                "`sh` exited before oakum could stop it, so what it started then may still be running",
            ),
        ] {
            let mut child = sleeper();
            let mut other = sleeper();
            let root = child.id();
            let left_pid = other.id();
            let rounds = std::cell::Cell::new(0);
            let reach = kill_tree_with(
                &mut child,
                || {
                    rounds.set(rounds.get() + 1);
                    let later = rounds.get() > 1;
                    let mut left = proc(left_pid, root, "sh");
                    if later && zombie {
                        left.zombie = true;
                    } else if later {
                        left.ppid = 1;
                    }
                    Ok(Vec::from([proc(root, std::process::id(), "sleep"), left]))
                },
                stop,
            );
            let stat = Command::new("ps")
                .args(["-o", "stat=", "-p", &left_pid.to_string()])
                .output()
                .expect("ps");
            let _ = other.kill();
            let _ = other.wait();
            assert_eq!(reach, Reach::Unknown(String::from(expected)), "zombie {zombie}");
            let stat = String::from_utf8_lossy(&stat.stdout);
            assert!(!stat.trim().starts_with('T'), "left stopped: {stat}");
            assert!(!stat.trim().is_empty(), "killed rather than let go");
        }
    }

    /// The pids still running `sleep <marker>`, killed so a red run leaves
    /// nothing behind. `pgrep` exits 1 for no match; anything else is no answer.
    fn survivors(marker: &str) -> String {
        let pattern = format!("sleep {marker}");
        let found = Command::new("pgrep").args(["-f", &pattern]).output();
        // The forking shells carry the pattern too; killed before any assert.
        let _ = Command::new("pkill").args(["-9", "-f", &pattern]).status();
        let found = found.expect("pgrep");
        assert!(
            matches!(found.status.code(), Some(0 | 1)),
            "pgrep gave no answer: {found:?}"
        );
        String::from_utf8_lossy(&found.stdout).trim().to_owned()
    }

    /// Unique to this run, so a concurrent run's sleepers are neither counted
    /// nor killed, and about 25 seconds long, so one the kill misses still
    /// ends on its own.
    fn marker(lead: u32) -> String {
        format!("2{lead}.{:05}", std::process::id() % 100_000)
    }

    /// A forker that stops by itself after about three seconds: one the kill
    /// misses must not fork until the process table is full.
    fn forker(marker: &str) -> String {
        format!("i=0; while [ $i -lt 300 ]; do sleep {marker} & sleep 0.01; i=$((i+1)); done")
    }

    /// Killed, or unconfirmed only because a short-lived process exited before
    /// its stop: anything else means the kill could not vouch for the tree.
    fn vouched(reach: &Reach) -> bool {
        match reach {
            Reach::Killed(_) => true,
            Reach::Unknown(why) => why
                .split("; ")
                .all(|part| part.contains("exited before oakum could stop it")),
            Reach::Unnamed => false,
        }
    }

    /// A stopped process leading its own group is continued by the kernel when
    /// its parent's death orphans the group; killed leaves first, it cannot
    /// fork in between.
    #[test]
    fn an_orphaned_group_is_killed_before_it_is_continued() {
        let marker = marker(4);
        let script = format!(
            "i=0; while [ $i -lt 100 ]; do sleep 5 & i=$((i+1)); done; set -m; \
             sh -c 'trap \"\" HUP; {}' & wait",
            forker(&marker)
        );
        let mut reaches = Vec::new();
        for _ in 0..6 {
            let mut child = Command::new("sh")
                .args(["-c", &script])
                .stdin(Stdio::null())
                .spawn()
                .expect("sh");
            std::thread::sleep(std::time::Duration::from_millis(300));
            reaches.push(super::kill_tree(&mut child));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let left = survivors(&marker);
        assert!(reaches.iter().all(vouched), "{reaches:?}");
        assert_eq!(left, "", "{reaches:?}");
    }

    /// A grandchild that forks every 10ms leaves nothing behind, because each
    /// listed process is stopped before the next listing.
    #[test]
    fn a_forking_tree_is_stopped_and_killed_whole() {
        let marker = marker(3);
        let mut child = Command::new("sh")
            .args(["-c", &format!("sh -c '{}' & wait", forker(&marker))])
            .stdin(Stdio::null())
            .spawn()
            .expect("sh");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let reach = super::kill_tree(&mut child);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let left = survivors(&marker);
        assert!(vouched(&reach), "{reach:?}");
        assert_eq!(left, "", "{reach:?}");
    }
}
