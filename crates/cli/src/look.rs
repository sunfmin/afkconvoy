//! A look begins here (docs/adr/0043).
//!
//! A look reads each source once. The mechanics are this module's: a
//! [`Reading`] is read through one shared cell per key, a second ask waits on
//! the first read instead of observing again, and a reading is forgotten when
//! the next look begins or when a write it names is made.
//!
//! A source declares, beside its read, what it reads, the key it reads by,
//! its [`Lifetime`], and which of its own writes forget it. A declared write
//! is reported once, by the source's own call ([`wrote`]), never at the
//! sites that make it.
//!
//! A tick and a fleet-instance start each call [`begin`] once. That forgets
//! every reading whose lifetime is [`Lifetime::Look`], so the next read
//! observes again: a further reading needs no registration and neither call
//! site gains a line. `doctor` is one look for its process and does not call
//! [`begin`]. Host readings stay behind `admission::Readings`. The login is a
//! [`Lifetime::Process`] reading and is not forgotten here.

use std::any::Any;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock};

/// How long one read of a reading answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// Until the next look begins.
    Look,
    /// For the life of the process: no look forgets it.
    Process,
}

/// One source's observation: what it reads, by which key, for how long, and
/// which of its writes forget it. Declared as a `static` beside the read.
pub struct Reading<V> {
    /// What it reads, unique among readings: its cells are kept by it.
    name: &'static str,
    lifetime: Lifetime,
    /// Each write as the command words it starts with, the tool first:
    /// `["orca", "worktree", "create"]`.
    forgotten_by: &'static [&'static [&'static str]],
    value: PhantomData<fn() -> V>,
}

impl<V> Reading<V> {
    /// A reading forgotten when the next look begins.
    pub const fn per_look(name: &'static str) -> Self {
        Self { name, lifetime: Lifetime::Look, forgotten_by: &[], value: PhantomData }
    }

    /// A reading that stands for the life of the process.
    pub const fn per_process(name: &'static str) -> Self {
        Self { name, lifetime: Lifetime::Process, forgotten_by: &[], value: PhantomData }
    }

    /// The source's writes that forget this reading, whatever its lifetime.
    pub const fn forgotten_by(self, writes: &'static [&'static [&'static str]]) -> Self {
        Self { forgotten_by: writes, ..self }
    }

    pub fn lifetime(&self) -> Lifetime {
        self.lifetime
    }
}

impl<V: Clone + Send + Sync + 'static> Reading<V> {
    /// The reading for `key` in the current look. The first ask runs
    /// `observe`; every later ask answers from it, waiting on it while it runs.
    pub fn read(&'static self, key: &str, observe: impl FnOnce() -> V) -> V {
        current(|look| look.read(self, key, observe))
    }

    /// Observes `key` again now, as one deliberately asked anew, and answers
    /// later asks in the look from that.
    pub fn again(&'static self, key: &str, observe: impl FnOnce() -> V) -> V {
        current(|look| {
            look.forget(self, key);
            look.read(self, key, observe)
        })
    }
}

/// Which reading a cell belongs to: its name. Two statics are not promised
/// two addresses, so a reading is not told by where it is kept.
type Id = &'static str;

fn id<V>(reading: &'static Reading<V>) -> Id {
    reading.name
}

/// One cell of a look and what forgets it.
struct Held {
    lifetime: Lifetime,
    forgotten_by: &'static [&'static [&'static str]],
    cell: Arc<dyn Any + Send + Sync>,
}

/// What one look has read. A process holds one; a test is handed its own.
pub struct Look {
    held: Mutex<BTreeMap<(Id, String), Held>>,
}

impl Default for Look {
    fn default() -> Self {
        Self::new()
    }
}

impl Look {
    pub const fn new() -> Self {
        Self { held: Mutex::new(BTreeMap::new()) }
    }

    fn cells(&self) -> std::sync::MutexGuard<'_, BTreeMap<(Id, String), Held>> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The cell of `reading` at `key`. The lock is not held across a read.
    fn cell<V: Send + Sync + 'static>(
        &self,
        reading: &'static Reading<V>,
        key: &str,
    ) -> Arc<OnceLock<V>> {
        let held = self
            .cells()
            .entry((id(reading), key.to_string()))
            .or_insert_with(|| Held {
                lifetime: reading.lifetime,
                forgotten_by: reading.forgotten_by,
                cell: Arc::new(OnceLock::<V>::new()),
            })
            .cell
            .clone();
        held.downcast().expect("a reading's cell holds its own value")
    }

    fn read<V: Clone + Send + Sync + 'static>(
        &self,
        reading: &'static Reading<V>,
        key: &str,
        observe: impl FnOnce() -> V,
    ) -> V {
        self.cell(reading, key).get_or_init(observe).clone()
    }

    fn forget<V>(&self, reading: &'static Reading<V>, key: &str) {
        self.cells().remove(&(id(reading), key.to_string()));
    }

    /// Forgets every reading of this look, keeping those of the process.
    pub fn begin(&self) {
        self.cells().retain(|_, held| held.lifetime == Lifetime::Process);
    }

    /// Forgets every reading `command` is a declared write of.
    pub fn wrote(&self, command: &[&str]) {
        self.cells()
            .retain(|_, held| !held.forgotten_by.iter().any(|write| command.starts_with(write)));
    }

    /// Seeds `reading` at `key`, as though this look had read `value`.
    #[cfg(test)]
    pub fn seed<V: Clone + Send + Sync + 'static>(
        &self,
        reading: &'static Reading<V>,
        key: &str,
        value: V,
    ) -> &Self {
        let _ = self.cell(reading, key).set(value);
        self
    }

    /// What this look has read of `reading` at `key`, without reading it.
    #[cfg(test)]
    pub fn held<V: Clone + Send + Sync + 'static>(
        &self,
        reading: &'static Reading<V>,
        key: &str,
    ) -> Option<V> {
        let cell = self.cells().get(&(id(reading), key.to_string()))?.cell.clone();
        cell.downcast::<OnceLock<V>>().ok()?.get().cloned()
    }

    /// Runs `f` on this thread with this look as the current one.
    pub fn enter<T>(self: &Arc<Self>, f: impl FnOnce() -> T) -> T {
        struct Restore(Option<Arc<Look>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                ENTERED.with(|entered| *entered.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(ENTERED.with(|entered| entered.replace(Some(self.clone()))));
        f()
    }
}

/// The process's look. A process is one look until [`begin`] starts the next.
static PROCESS: Look = Look::new();

thread_local! {
    /// A scoped look on this thread, in place of the process's. A report
    /// transition rereads under its tracker claim without resetting peers.
    static ENTERED: std::cell::RefCell<Option<Arc<Look>>> =
        const { std::cell::RefCell::new(None) };
}

fn current<T>(f: impl FnOnce(&Look) -> T) -> T {
    if let Some(look) = ENTERED.with(|entered| entered.borrow().clone()) {
        return f(&look);
    }
    f(&PROCESS)
}

/// Begins a look. The next read of each reading a look reads once observes
/// again.
pub fn begin() {
    current(Look::begin);
}

/// Reports a write a source made, as the command words it ran with the tool
/// first. Each reading that names it as one of its writes is forgotten.
pub fn wrote(command: &[&str]) {
    current(|look| look.wrote(command));
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    static FURTHER: Reading<usize> = Reading::per_look("test further");
    static STANDING: Reading<usize> = Reading::per_process("test standing");
    static WRITTEN: Reading<usize> =
        Reading::per_process("test written").forgotten_by(&[&["tool", "make"], &["tool", "drop"]]);

    #[test]
    fn a_tick_and_a_start_begin_a_look_through_one_call() {
        for (name, source) in
            [("tick", include_str!("tick.rs")), ("start", include_str!("instance.rs"))]
        {
            assert_eq!(
                source.matches("crate::look::begin()").count(),
                1,
                "{name} begins a look through one call"
            );
            assert!(!source.contains("look::wrote"), "{name} does not forget a reading itself");
        }
        assert!(
            !include_str!("doctor.rs").contains("crate::look::begin()"),
            "doctor is one look and does not reset between reads"
        );
    }

    #[test]
    fn begin_forgets_a_further_reading_with_no_registration() {
        let look = Arc::new(Look::new());
        look.seed(&FURTHER, "", 1).seed(&STANDING, "", 1);
        look.enter(begin);
        assert_eq!(look.held(&FURTHER, ""), None, "a look's reading is read again");
        assert_eq!(look.held(&STANDING, ""), Some(1), "a process's reading stands");
    }

    #[test]
    fn a_second_ask_answers_from_the_first_read() {
        let look = Arc::new(Look::new());
        let reads = AtomicUsize::new(0);
        let ask = || FURTHER.read("k", || reads.fetch_add(1, Ordering::SeqCst) + 7);
        look.enter(|| {
            assert_eq!(ask(), 7);
            assert_eq!(ask(), 7);
            assert_eq!(FURTHER.read("other", || 9), 9, "each key is its own read");
            assert_eq!(FURTHER.again("k", || 8), 8, "asked anew, it observes again");
            assert_eq!(ask(), 8);
        });
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_declared_write_forgets_its_reading_whatever_its_lifetime() {
        let look = Arc::new(Look::new());
        look.seed(&WRITTEN, "", 1).seed(&STANDING, "", 1);
        look.enter(|| wrote(&["tool", "list"]));
        assert_eq!(look.held(&WRITTEN, ""), Some(1), "a read is no write");
        look.enter(|| wrote(&["tool", "make", "--name", "w"]));
        assert_eq!(look.held(&WRITTEN, ""), None, "its write forgets it");
        assert_eq!(look.held(&STANDING, ""), Some(1), "another reading stays");
    }

    #[test]
    fn a_look_handed_to_a_test_is_its_own() {
        let mine = Arc::new(Look::new());
        mine.seed(&FURTHER, "", 3);
        assert_eq!(FURTHER.read("", || 4), 4, "the process's look is not the test's");
        assert_eq!(mine.enter(|| FURTHER.read("", || 5)), 3);
    }

    #[test]
    fn one_look_answers_from_its_read_and_the_next_observes_again() {
        let look = Arc::new(Look::new());
        let dir = PathBuf::from("one-look");
        let dir = dir.to_string_lossy();
        look.seed(&crate::orca::REPOS, "", Ok(json!({"repos": [{"id": "r1"}]})))
            .seed(&crate::orca::RUNNING, "", true)
            .seed(&crate::orca::WORKTREES, "", Ok(json!({"worktrees": []})))
            .seed(&crate::gh_quota::NOTE, &dir, ())
            .seed(&crate::gh_quota::RATES, "github.com", Ok("{}".to_string()))
            .seed(&crate::gh::HEADS, "acme/dev", Ok("abc".to_string()));
        look.enter(|| {
            assert!(crate::orca::running(), "a later question answers from the read");
        });

        look.enter(begin);

        assert!(look.held(&crate::orca::REPOS, "").is_none(), "the repo list is read again");
        assert!(look.held(&crate::orca::RUNNING, "").is_none(), "orca is asked again");
        assert!(look.held(&crate::orca::WORKTREES, "").is_none(), "worktrees are listed again");
        assert!(look.held(&crate::gh_quota::NOTE, &dir).is_none(), "the quota note again");
        assert!(look.held(&crate::gh_quota::RATES, "github.com").is_none(), "rate_limit again");
        assert!(look.held(&crate::gh::HEADS, "acme/dev").is_none(), "HEAD again");
    }

    #[test]
    fn the_sources_read_through_the_look_with_their_lifetimes() {
        use crate::{gh, gh_quota, launch, orca, tools};
        for (name, lifetime) in [
            ("login shell", launch::ANSWERED.lifetime()),
            ("repos", orca::REPOS.lifetime()),
            ("running", orca::RUNNING.lifetime()),
            ("worktrees", orca::WORKTREES.lifetime()),
            ("heads", gh::HEADS.lifetime()),
            ("note", gh_quota::NOTE.lifetime()),
            ("rates", gh_quota::RATES.lifetime()),
        ] {
            assert_eq!(lifetime, Lifetime::Look, "{name} is read once per look");
        }
        assert_eq!(tools::GH_LOGIN.lifetime(), Lifetime::Process, "the login stands");
        assert_eq!(gh_quota::SCOPES.lifetime(), Lifetime::Process, "the login scope stands");
        for (name, source) in [
            ("orca", include_str!("orca.rs")),
            ("gh", include_str!("gh.rs")),
            ("gh_quota", include_str!("gh_quota.rs")),
            ("tools", include_str!("tools.rs")),
        ] {
            for own in ["fn forget_", "fn hold_", "_cached()", "_held("] {
                assert!(!source.contains(own), "{name} keeps no read-once cell of its own: {own}");
            }
        }
    }

    #[test]
    fn creating_or_removing_a_worktree_forgets_the_worktree_list() {
        for write in [["orca", "worktree", "create"], ["orca", "worktree", "rm"]] {
            let look = Arc::new(Look::new());
            look.seed(&crate::orca::WORKTREES, "", Ok(json!({"worktrees": []}))).seed(
                &crate::orca::REPOS,
                "",
                Ok(json!({"repos": []})),
            );
            look.enter(|| wrote(&write));
            assert!(look.held(&crate::orca::WORKTREES, "").is_none(), "{write:?}");
            assert!(look.held(&crate::orca::REPOS, "").is_some(), "{write:?} keeps the repo list");
        }
        let look = Arc::new(Look::new());
        look.seed(&crate::orca::REPOS, "", Ok(json!({"repos": []})));
        look.enter(|| wrote(&["orca", "repo", "add", "--path", "/x"]));
        assert!(look.held(&crate::orca::REPOS, "").is_none(), "adding a repo forgets the list");
        let source = include_str!("orca.rs");
        assert!(
            !source.contains("forget_worktrees") && !source.contains("forget_listing"),
            "no call site forgets a reading by hand"
        );
    }
}
