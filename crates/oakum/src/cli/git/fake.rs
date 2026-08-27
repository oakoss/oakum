//! An in-process stand-in for the git child.
//!
//! Two things need it. The rules in [`super`] classify answers a real repository
//! is awkward to produce on demand — a look that warns while printing nothing, a
//! wrapper that exits 1 with a diagnostic, a child a signal killed — and the
//! shell shims that produced them in `tests/check.rs` run on unix only. And a
//! caller that gates one read on another is only shown to gate by observing that
//! the second child never ran, which a real repository cannot report.

use std::sync::Mutex;

use super::Reply;

/// An answer and the operation that earns it, named by [`super::Op::name`].
struct Scripted {
    op: &'static str,
    reply: Reply,
}

/// Answers keyed by operation rather than by position.
///
/// Keying matters twice over. A positional queue answers whatever is asked, so
/// a caller that spawns the wrong child still gets a reply and the test still
/// passes. And position makes every answer an obligation: a test cannot script
/// the child a gate is meant to prevent, because the gate holding would shift
/// every later answer onto the wrong read. Keyed, an unclaimed answer is one
/// the caller was right not to ask for — which is why nothing here complains
/// about leftovers. What a caller did ask for is [`Fake::asked`], which a test
/// asserts on directly.
///
/// The key is the operation's name, not its argv. An argv key would put git's
/// flags in every caller's test, where a changed flag breaks a test that has
/// nothing to do with it, and matching one would need rules about how much of a
/// command line counts. The names are distinct, stable, and already the phrase
/// a failure quotes, so a script reads as the diagnostics it stands for.
///
/// `Mutex` rather than `RefCell` so a scripted [`super::Git`] stays `Sync` like
/// the shipping one, instead of changing shape under `cfg(test)`.
pub(super) struct Fake {
    scripted: Mutex<Vec<Option<Scripted>>>,
    asked: Mutex<Vec<String>>,
}

impl Fake {
    pub(super) fn answering(replies: impl IntoIterator<Item = (&'static str, Reply)>) -> Self {
        Self {
            scripted: Mutex::new(
                replies
                    .into_iter()
                    .map(|(op, reply)| Some(Scripted { op, reply }))
                    .collect(),
            ),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// The first unclaimed answer scripted for this operation. Repeating one
    /// scripts successive answers for it, first written answered first.
    ///
    /// # Panics
    ///
    /// When nothing scripted matches. A caller asking for a child the test never
    /// named is the finding, so it is reported rather than defaulted away.
    pub(super) fn answer(&self, op: &'static str) -> Reply {
        self.asked
            .lock()
            .expect("the fake is not shared across threads")
            .push(String::from(op));
        let mut scripted = self
            .scripted
            .lock()
            .expect("the fake is not shared across threads");
        let found = scripted
            .iter_mut()
            .find(|entry| entry.as_ref().is_some_and(|entry| entry.op == op))
            .and_then(Option::take);
        // Released before the panic below: unwinding while it is held poisons
        // the mutex, and every later `lock` then reports that instead of this.
        drop(scripted);
        match found {
            Some(entry) => entry.reply,
            None => panic!("nothing scripted answers `git {op}`"),
        }
    }

    /// Each operation the caller asked for, in order.
    pub(super) fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .expect("the fake is not shared across threads")
            .clone()
    }
}
