//! What a remote's listed URLs establish about ssh and helpers.

use super::env::BatchSsh;
use super::{CliError, Contact, Direction};

/// What oakum established about a remote's URL set. Three independent facts
/// rather than one verdict: a push contacts every URL listed, so one set can
/// reach ssh and run a helper at once, and collapsing the facts into a single
/// answer is what lost the helper note for exactly that mix.
#[derive(Clone, Debug)]
pub(super) struct Reach {
    pub(super) ssh: bool,
    /// The remote runs a helper command — `<transport>::<address>`, or a URL
    /// scheme git does not implement natively, which falls through to
    /// `git-remote-<scheme>`. Measured: the helper inherits `GIT_SSH_COMMAND`
    /// and applies none of it, so `BatchMode` never reaches whatever it runs.
    pub(super) helper: bool,
    /// The listing could not be read. The note still prints under an
    /// unprotected transport — it is advisory, and withholding it because the
    /// check failed is the quieter wrong answer — and an unread URL is never
    /// evidence of a safe transport.
    pub(super) unread: Option<CliError>,
}

/// `remote -v` lines folded into per-remote, per-direction URL lists. Each
/// line's name half is matched against the configured remote names — a name
/// can itself contain a tab (config-made), so splitting on the first tab
/// could hand one remote's URL to another. The URL is everything up to the
/// trailing direction marker, so a URL containing whitespace survives. A
/// remote with no URL for a direction renders as a bare `name\t` line
/// (measured on a pushurl-only remote) and is simply absent; any other
/// unrecognized shape fails the whole parse, because a listing oakum cannot
/// read is a look that did not happen, not an empty answer.
type RemoteUrlList = Vec<((String, Direction), String)>;

pub(super) fn parse_remote_urls(listing: &str, names: &[&str]) -> Result<RemoteUrlList, CliError> {
    let mut folded: RemoteUrlList = Vec::new();
    for line in listing.lines() {
        let name = names
            .iter()
            .filter(|name| {
                line.strip_prefix(**name)
                    .is_some_and(|rest| rest.starts_with('\t'))
            })
            .max_by_key(|name| name.len());
        let Some(&name) = name else {
            return Err(CliError::new(format!(
                "unparseable `git remote -v` line {line:?}"
            )));
        };
        let rest = &line[name.len() + 1..];
        if rest.trim().is_empty() {
            continue;
        }
        let parsed = if let Some(url) = rest.strip_suffix(" (fetch)") {
            Some((url, Direction::Fetch))
        } else {
            rest.strip_suffix(" (push)")
                .map(|url| (url, Direction::Push))
        };
        let Some((url, direction)) = parsed else {
            return Err(CliError::new(format!(
                "unparseable `git remote -v` line {line:?}"
            )));
        };
        let key = (name.to_owned(), direction);
        match folded.iter_mut().find(|(entry, _)| *entry == key) {
            Some((_, urls)) => {
                urls.push('\n');
                urls.push_str(url);
            }
            None => folded.push((key, url.to_owned())),
        }
    }
    Ok(folded)
}

pub(super) fn classify(urls: Result<&str, &CliError>) -> Reach {
    match urls {
        // Any of them: a push reaches every URL listed, so one over ssh — or
        // one selecting a helper — is enough to make its note apply.
        Ok(urls) => Reach {
            ssh: urls.lines().any(reaches_over_ssh),
            helper: urls.lines().any(runs_a_helper),
            unread: None,
        },
        Err(err) => {
            // The advisory note is not rendering a verification verdict, so
            // the embedded error sheds its outcome token.
            let detail = err.detail();
            Reach {
                ssh: false,
                helper: false,
                unread: Some(CliError::new(format!(
                    "oakum could not read that remote's URL ({detail})"
                ))),
            }
        }
    }
}

/// Every note the remote's URL set owes, so independent hazards cannot
/// swallow each other: a helper speaks in both transport columns, because
/// `BatchMode` never reaches what a helper runs; ssh and an unread listing
/// speak only when the transport could not be protected.
///
/// Separate from saying them, so the text is assertable without a real
/// child's stderr.
pub(super) fn notes_for(contact: Contact<'_>, reach: &Reach, batch: &BatchSsh) -> Vec<String> {
    let remote = contact.remote;
    let reason = batch.unprotected_reason();
    let mut notes = Vec::new();
    if reach.helper {
        let mut note = format!(
            "the remote {remote:?} runs a helper command oakum cannot inspect; \
             its ssh `BatchMode` does not reach whatever that helper runs, so \
             a prompt can still block."
        );
        if let Some(reason) = reason {
            use std::fmt::Write;
            let _ = write!(
                note,
                " The ssh transport could not be protected either: {reason}."
            );
        }
        notes.push(note);
    }
    if let Some(why) = &reach.unread {
        let mut note = format!(
            "oakum could not establish what transport {remote:?} uses ({why}); \
             if the remote runs a helper, a prompt may still block."
        );
        if let Some(reason) = reason {
            use std::fmt::Write;
            let _ = write!(
                note,
                " The ssh transport could not be protected either: {reason}."
            );
        }
        notes.push(note);
    }
    if let Some(reason) = reason {
        if reach.ssh {
            notes.push(format!(
                "oakum cannot refuse ssh prompts for the transport {remote:?} \
                 uses: {reason}. A prompt can still block."
            ));
        }
    }
    notes
}

/// The note about a transport that cannot take `BatchMode` is only about ssh,
/// but the transport resolves from the environment before any remote URL is
/// known. Gating on the transport alone prints it for `https://` and `file://`
/// remotes, where ssh is never invoked and no prompt it describes can occur.
fn reaches_over_ssh(url: &str) -> bool {
    if let Some((scheme, _)) = url.split_once("://") {
        // Matched exactly, as git matches its own table. Measured with
        // `GIT_TRACE=1 GIT_SSH_COMMAND=<marker> git ls-remote`: `git+ssh://` and
        // `ssh+git://` do reach ssh, and `SSH://` does not — it falls through to
        // a `git-remote-SSH` helper.
        return matches!(scheme, "ssh" | "git+ssh" | "ssh+git");
    }
    let Some(at) = scp_separator(url) else {
        return false;
    };
    // `<transport>::<address>` names a remote helper: measured, `git ls-remote
    // -- a::b` reports `remote helper 'a'` and never runs ssh. An empty host is
    // still ssh, though — `:oakum.git` dials one.
    if url[at..].starts_with("::") {
        return false;
    }
    !dos_drive(&url[..at])
}

/// Whether a remote runs a helper command: `<transport>::<address>`, or a URL
/// scheme git does not implement natively, which selects `git-remote-<scheme>`
/// exactly as the `::` form does — measured, `marker://addr` and
/// `marker::addr` run the same helper with the same uninspectable transport.
/// The native list is exact and case-sensitive, as git matches its own table:
/// `SSH://` reaches `git-remote-SSH`, not ssh.
fn runs_a_helper(url: &str) -> bool {
    if names_a_helper(url) {
        return true;
    }
    match url.split_once("://") {
        Some((scheme, _)) => !matches!(
            scheme,
            "ssh" | "git+ssh" | "ssh+git" | "http" | "https" | "git" | "file" | "ftp" | "ftps"
        ),
        None => false,
    }
}

fn names_a_helper(url: &str) -> bool {
    scp_separator(url).is_some_and(|at| url[at..].starts_with("::"))
}

/// The colon separating host from path in an scp-like remote, if there is one.
///
/// A bracketed IPv6 literal carries colons of its own and can sit after
/// userinfo, so `user@[::1]:repo.git` is separated by the colon *after* the
/// bracket — measured, git dials that over ssh. A colon reached after a slash
/// belongs to the path.
fn scp_separator(url: &str) -> Option<usize> {
    let mut bracketed = false;
    for (at, character) in url.char_indices() {
        match character {
            // Only a bracket that closes opens a literal. Unmatched, it is an
            // ordinary character in a hostname: measured, `git ls-remote --
            // 'foo[bar:baz'` dials ssh with host `foo[bar`.
            '[' if url[at..].contains(']') => bracketed = true,
            ']' => bracketed = false,
            ':' if !bracketed => return Some(at),
            '/' => return None,
            _ => {}
        }
    }
    None
}

/// Whether a one-letter prefix is a drive rather than a hostname, which is true
/// only on Windows: measured here, `x:r.git` and `C:\repos\oakum` both reach git
/// over ssh, so a single-letter prefix is a legitimate host off Windows. The
/// Windows side follows git's DOS-drive handling.
fn dos_drive(host: &str) -> bool {
    cfg!(windows) && host.len() == 1 && host.starts_with(|first: char| first.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::super::Direction;
    use super::*;

    /// Which remotes the ssh-prompt note applies to: not `https://` or
    /// `file://`, where ssh is never invoked and the prompt it warns about
    /// cannot happen.
    ///
    /// Every case was measured rather than reasoned about, with `GIT_TRACE=1
    /// GIT_SSH_COMMAND=<marker> git ls-remote <url>` against git 2.55.
    #[test]
    fn only_an_ssh_remote_can_stop_at_an_ssh_prompt() {
        for url in [
            "ssh://git@github.com/oakoss/oakum.git",
            // Aliases git accepts for the same transport.
            "git+ssh://git@github.com/oakoss/oakum.git",
            "ssh+git://git@github.com/oakoss/oakum.git",
            "git@github.com:oakoss/oakum.git",
            "github.com:oakoss/oakum.git",
        ] {
            assert!(reaches_over_ssh(url), "{url} reaches git over ssh");
        }
        #[cfg(not(windows))]
        assert!(
            reaches_over_ssh("x:oakum.git"),
            "a single-letter host is a host off Windows"
        );
        #[cfg(windows)]
        assert!(
            !reaches_over_ssh("x:oakum.git"),
            "a single-letter prefix is a drive on Windows"
        );
        for url in [
            // Git's scheme table is case-sensitive: this reaches a
            // `git-remote-SSH` helper, not ssh.
            "SSH://git@github.com/oakoss/oakum.git",
            "https://github.com/oakoss/oakum.git",
            "http://github.com/oakoss/oakum.git",
            "git://github.com/oakoss/oakum.git",
            "file:///srv/mirrors/oakum.git",
            "/srv/mirrors/oakum.git",
            "../sibling.git",
            // A colon after a slash belongs to the path.
            "./odd:name.git",
        ] {
            assert!(!reaches_over_ssh(url), "{url} does not use ssh");
        }
    }

    /// Remote-helper syntax and an empty host, both measured against git 2.55:
    /// `git ls-remote -- a::b` reports `remote helper 'a'` and never runs ssh,
    /// while `:oakum.git` does dial one, so an empty host is a remote that can
    /// block.
    #[test]
    fn helper_syntax_is_not_ssh_but_an_empty_host_is() {
        for url in [
            ":oakum.git",
            "[::1]:demo.git",
            // An unmatched bracket is part of the host, not a literal.
            "foo[bar:baz",
            "a[b:c",
            // The literal can sit after userinfo, where a bracket check anchored
            // at the start does not see it.
            "user@[::1]:repo.git",
            "git@[2001:db8::1]:oakum.git",
        ] {
            assert!(reaches_over_ssh(url), "{url} reaches git over ssh");
        }
        for url in ["a::b", "::a"] {
            assert!(!reaches_over_ssh(url), "{url} names a remote helper");
        }
    }

    /// A `<helper>::<address>` remote runs a command oakum cannot inspect, and
    /// an `ext::` one can invoke ssh itself without `BatchMode` — measured, one
    /// did, because a helper inherits `GIT_SSH_COMMAND` and applies none of it.
    /// Not ssh, but not established either, so it must not read as safe.
    #[test]
    fn a_helper_remote_is_unestablished_rather_than_not_ssh() {
        for url in ["a::b", "::a", "ext::ssh -p 22 git@h git-upload-pack r.git"] {
            assert!(names_a_helper(url), "{url} names a helper");
            assert!(!reaches_over_ssh(url), "{url} is not itself an ssh URL");
        }
        for url in [
            "git@github.com:oakoss/oakum.git",
            "https://github.com/oakoss/oakum.git",
            "ssh://git@github.com/oakoss/oakum.git",
            "/srv/mirrors/oakum.git",
        ] {
            assert!(!names_a_helper(url), "{url} names no helper");
        }
    }

    /// A drive letter is a path on Windows and a hostname everywhere else.
    /// Measured here: `git ls-remote 'C:\repos\oakum'` invokes ssh.
    #[cfg(not(windows))]
    #[test]
    fn a_drive_letter_is_a_hostname_off_windows() {
        assert!(reaches_over_ssh(r"C:\repos\oakum"));
        assert!(reaches_over_ssh("C:/repos/oakum"));
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_letter_is_a_path_on_windows() {
        assert!(!reaches_over_ssh(r"C:\repos\oakum"));
        assert!(!reaches_over_ssh("C:/repos/oakum"));
        assert!(!reaches_over_ssh("x:oakum.git"));
    }

    /// A remote can fetch over https and push over ssh, so the direction picks
    /// its URLs from the listing. This is the only guard on that: the note's
    /// text is direction-agnostic, so no process-level test can tell a fetch
    /// note from a push one, and swapping the directions passes every
    /// integration suite.
    #[test]
    fn the_listing_keeps_fetch_and_push_urls_apart() {
        let names = ["origin", "s", "pushonly"];
        let listing = "origin\thttps://host/r.git (fetch)\n\
                       origin\tgit@host:r.git (push)\n\
                       origin\thelper::two (push)";
        let parsed = parse_remote_urls(listing, &names).expect("parses");
        let urls = |direction: Direction| {
            parsed
                .iter()
                .find(|((name, at), _)| name == "origin" && *at == direction)
                .map(|(_, urls)| urls.as_str())
                .expect("listed")
        };
        assert!(!classify(Ok(urls(Direction::Fetch))).ssh);
        assert!(classify(Ok(urls(Direction::Push))).ssh);

        let err = parse_remote_urls("origin\thttps://host/r.git (fetched)", &names)
            .expect_err("an unknown marker fails the parse");
        assert!(err.to_string().contains("unparseable"), "{err}");
        let err = parse_remote_urls("stranger\thttps://host/r.git (fetch)", &names)
            .expect_err("a name outside the configured set fails the parse");
        assert!(err.to_string().contains("unparseable"), "{err}");

        let spaced = parse_remote_urls("s\text::sh -c \"echo hi\" (fetch)", &names)
            .expect("whitespace in a URL survives");
        assert_eq!(spaced[0].1, "ext::sh -c \"echo hi\"");

        // A pushurl-only remote renders `name\t` for its missing direction
        // (measured); that direction is absent, and the other remotes' reach
        // must not be poisoned by it.
        let partial = parse_remote_urls(
            "pushonly\t\npushonly\tgit@host:r.git (push)\norigin\thttps://host/r.git (fetch)",
            &names,
        )
        .expect("a bare direction line is absence, not failure");
        assert_eq!(partial.len(), 2, "{partial:?}");
    }

    /// A read that failed is unread, never safe, and a helper is a fact of
    /// its own — whichever spelling selects it, and even alongside ssh: the
    /// facts are independent, so neither can swallow the other.
    #[test]
    fn an_unread_url_is_never_classed_as_safe() {
        let failed = CliError::new("git exited 128");
        let unread = classify(Err(&failed));
        assert!(unread.unread.is_some() && !unread.ssh && !unread.helper);

        let helper = classify(Ok("ext::my-helper"));
        assert!(helper.helper && !helper.ssh);
        // Same helper, same hazard, both spellings — `marker://addr` runs
        // `git-remote-marker` exactly as `marker::addr` does, and git matches
        // its native table case-sensitively, so `SSH://` is a helper too.
        assert!(classify(Ok("marker://addr")).helper);
        assert!(classify(Ok("SSH://host/r.git")).helper);

        let ssh = classify(Ok("git@host:r.git"));
        assert!(ssh.ssh && !ssh.helper);
        let plain = classify(Ok("https://host/r.git"));
        assert!(!plain.ssh && !plain.helper && plain.unread.is_none());
        assert!(!classify(Ok("ftps://host/r.git")).helper);

        // A push reaches every URL listed, so a set that is both ssh and
        // helper keeps both facts (measured: `url` + an added helper `url`
        // both receive the push).
        let mixed = classify(Ok("ssh://host/a.git\nmarker://b"));
        assert!(mixed.ssh && mixed.helper);
    }

    /// The whole policy, one row at a time, and the rows are independent: a
    /// helper speaks in both transport columns, ssh and an unread listing
    /// speak only when the transport could not be protected, and a set
    /// carrying two hazards says both.
    #[test]
    fn the_note_table_covers_every_reach_and_both_transport_columns() {
        let origin = Contact {
            remote: "origin",
            direction: Direction::Fetch,
        };
        let composed = BatchSsh::Composed(String::from("ssh -o BatchMode=yes"));
        let unprotected = BatchSsh::Unprotected(String::from("ssh.variant is opaque"));
        let plain = Reach {
            ssh: false,
            helper: false,
            unread: None,
        };
        let ssh_only = Reach {
            ssh: true,
            ..plain.clone()
        };
        let helper_only = Reach {
            helper: true,
            ..plain.clone()
        };
        let unread = Reach {
            unread: Some(CliError::new("could not read that remote's URL")),
            ..plain.clone()
        };

        assert!(notes_for(origin, &plain, &composed).is_empty());
        assert!(notes_for(origin, &plain, &unprotected).is_empty());
        assert!(notes_for(origin, &ssh_only, &composed).is_empty());

        // An unread listing speaks in both columns: oakum cannot rule a
        // helper out, and the helper hazard is transport-independent.
        let unread_composed = notes_for(origin, &unread, &composed);
        assert_eq!(unread_composed.len(), 1, "{unread_composed:?}");
        assert!(
            unread_composed[0].contains("could not establish what transport"),
            "{unread_composed:?}"
        );
        assert!(
            unread_composed[0].contains("may still block"),
            "{unread_composed:?}"
        );
        assert!(
            !unread_composed[0].contains("could not be protected"),
            "a composed transport owes no transport rider: {unread_composed:?}"
        );

        let helper_composed = notes_for(origin, &helper_only, &composed);
        assert_eq!(helper_composed.len(), 1, "{helper_composed:?}");
        assert!(
            helper_composed[0].contains("\"origin\""),
            "{helper_composed:?}"
        );
        assert!(helper_composed[0].contains("helper"), "{helper_composed:?}");
        assert!(
            helper_composed[0].contains("can still block"),
            "{helper_composed:?}"
        );
        let helper_unprotected = notes_for(origin, &helper_only, &unprotected);
        assert_eq!(helper_unprotected.len(), 1, "{helper_unprotected:?}");
        assert!(
            helper_unprotected[0].contains("ssh.variant is opaque"),
            "the transport reason must not be dropped: {helper_unprotected:?}"
        );

        let ssh = notes_for(origin, &ssh_only, &unprotected);
        assert_eq!(ssh.len(), 1, "{ssh:?}");
        assert!(ssh[0].contains("\"origin\""), "{}", ssh[0]);
        assert!(ssh[0].contains("ssh.variant is opaque"), "{}", ssh[0]);
        // The claim, not only the nouns: deleting the warning sentence left
        // all 27 test targets green.
        assert!(ssh[0].contains("cannot refuse ssh prompts"), "{}", ssh[0]);
        assert!(ssh[0].contains("can still block"), "{}", ssh[0]);

        let hedged = notes_for(origin, &unread, &unprotected);
        assert_eq!(hedged.len(), 1, "{hedged:?}");
        assert!(hedged[0].contains("may still block"), "{}", hedged[0]);
        assert!(
            hedged[0].contains("could not read that remote's URL"),
            "the note must say why it could not tell: {}",
            hedged[0]
        );
        assert!(
            hedged[0].contains("could not be protected either"),
            "an unprotected transport adds its reason: {}",
            hedged[0]
        );
        assert_ne!(ssh[0], hedged[0], "a guess must not read as a finding");

        // Two hazards, two notes: a push URL set that is ssh and helper at
        // once owes both, under an unprotected transport.
        let both = Reach {
            ssh: true,
            helper: true,
            unread: None,
        };
        let notes = notes_for(origin, &both, &unprotected);
        assert_eq!(notes.len(), 2, "{notes:?}");
        // ...and the helper note survives a composed transport alone.
        let notes = notes_for(origin, &both, &composed);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("helper"), "{notes:?}");
    }
}
