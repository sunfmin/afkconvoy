//! A checkout's concurrency: how many claims its fleet instance holds at
//! work at once on this host. It is the checkout's, said on its host and
//! never pushed: kept with its entry on the checkout list, set by a `run`
//! that names it, and read as each tick opens the fleet — as the host
//! settings are read as they are used — so a change holds at the next tick
//! and no fleet instance is started again for it. Until its checkout says
//! one, the repo config's `concurrency` holds, or the default.
//!
//! A checkout the list holds no entry of yet — its first tick has not ended,
//! or a `run --foreground` holds it — goes by what its `run` asked, kept in
//! that process alone until its first tick puts it on the list with it.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use afkconvoy_core::config::Config;
use anyhow::Result;
use serde_json::{Value, json};

use crate::checkout_list::List;
use crate::config;
use crate::git::Repo;
use crate::host_settings::To;

/// What the `run` of each checkout the list holds no entry of asked, in this
/// process, by the checkout's root. Whoever reads it beside the list holds
/// it while they read the list, and its first tick forgets it only once the
/// list holds it (`forget`): a reader finds it in one or the other.
static ASKED: Mutex<BTreeMap<PathBuf, NonZeroUsize>> = Mutex::new(BTreeMap::new());

fn asked() -> MutexGuard<'static, BTreeMap<PathBuf, NonZeroUsize>> {
    ASKED.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `--concurrency` as said: a whole number, at least 1, or `default`.
pub fn claims_or_default(said: &str) -> Result<To<NonZeroUsize>, String> {
    To::parse(said, |said| {
        said.parse()
            .map_err(|_| format!("`{said}` is not a whole number of at least 1, nor `default`"))
    })
}

/// The repo config of the checkout `repo` as its fleet instance goes by it
/// on this host: with the checkout's own concurrency laid over the repo's
/// (`own`). A config or a checkout list that cannot be read is an error
/// naming it.
pub fn config_of(repo: &Repo) -> Result<Config> {
    let config = config::load(&config::file_of(repo))?;
    Ok(match own(&List::of_host()?, &repo.dir)? {
        Some(own) => config.laid(own.get()),
        None => config,
    })
}

/// The concurrency the checkout at `root` says on the host of `list`, where
/// it says one: the one with its entry; where the list holds no entry of it,
/// what its `run` asked in this process.
pub fn own(list: &List, root: &Path) -> Result<Option<NonZeroUsize>> {
    let asked = asked();
    let listed = list.read()?;
    Ok(match listed.checkouts.iter().find(|entry| entry.checkout == root) {
        Some(entry) => entry.concurrency,
        None => asked.get(root).copied(),
    })
}

/// Has the checkout at `root` say `to` of its concurrency on the host of
/// `list`: with its entry, where the list holds one; where it holds none
/// yet, as what its `run` asked, in this process, which its first tick puts
/// on the list with it.
pub fn say(list: &List, root: &Path, to: To<NonZeroUsize>) -> Result<()> {
    list.change(|listed| match listed.checkouts.iter_mut().find(|entry| entry.checkout == root) {
        Some(entry) => entry.concurrency = to.kept(),
        None => ask(root, to),
    })
}

/// Keeps `to` as what the `run` of the checkout at `root` asked, in this
/// process alone: `Default` asks none. What a checkout on the list says
/// with its entry holds over it.
pub fn ask(root: &Path, to: To<NonZeroUsize>) {
    match to.kept() {
        Some(own) => asked().insert(root.to_path_buf(), own),
        None => asked().remove(root),
    };
}

/// What the `run` of the checkout at `root` asked in this process, where it
/// asked one: what its first tick puts on the list with it.
pub fn asked_of(root: &Path) -> Option<NonZeroUsize> {
    asked().get(root).copied()
}

/// Forgets what the `run` of the checkout at `root` asked in this process,
/// once its entry on the list holds it.
pub fn forget(root: &Path) {
    asked().remove(root);
}

/// The concurrency `config` goes by, as `plan`, `doctor` and `checkouts` say
/// it: {in_force, said_by} — said by the checkout, the repo's `config`, or
/// nobody, the `default`.
pub fn json(config: &Config) -> Value {
    json!({"in_force": config.concurrency(), "said_by": config.concurrency_said_by()})
}

/// The concurrency the fleet instance of the checkout `repo` goes by on this
/// host, as `json` says it; {error} where its config or the checkout list
/// cannot be read.
pub fn json_of(repo: &Repo) -> Value {
    match config_of(repo) {
        Ok(config) => json(&config),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkout_list::Entry;
    use crate::instance::Named;

    fn of(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    #[test]
    fn a_concurrency_is_a_whole_number_of_at_least_one_or_default() {
        assert_eq!(claims_or_default("6"), Ok(To::Value(of(6))));
        assert_eq!(claims_or_default("default"), Ok(To::Default));
        for wrong in ["0", "-1", "1.5", "lots", ""] {
            let error = claims_or_default(wrong).unwrap_err();
            assert!(error.contains("not a whole number of at least 1, nor `default`"), "{error}");
            assert!(error.contains(&format!("`{wrong}`")), "{wrong:?}: {error}");
        }
    }

    #[test]
    fn the_checkout_on_the_list_says_its_own_and_one_not_on_it_yet_what_its_run_asked() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join("checkouts.json"));
        let (listed, starting) = (dir.path().join("listed"), dir.path().join("starting"));
        let entry = Entry { checkout: listed.clone(), named: Named::default(), concurrency: None };
        list.change(|kept| kept.checkouts.push(entry)).unwrap();
        assert_eq!(own(&list, &listed).unwrap(), None, "none said: the repo's holds");

        // the one on the list says it with its entry, which holds over what
        // a `run` asked in this process
        say(&list, &listed, To::Value(of(6))).unwrap();
        ask(&listed, To::Value(of(2)));
        assert_eq!(own(&list, &listed).unwrap(), Some(of(6)));
        assert_eq!(list.read().unwrap().checkouts[0].concurrency, Some(of(6)));
        say(&list, &listed, To::Default).unwrap();
        assert_eq!(own(&list, &listed).unwrap(), None);
        forget(&listed);

        // one not on it yet says what its `run` asked, and nothing is written
        say(&list, &starting, To::Value(of(4))).unwrap();
        assert_eq!(own(&list, &starting).unwrap(), Some(of(4)));
        assert_eq!(asked_of(&starting), Some(of(4)));
        assert_eq!(list.read().unwrap().checkouts.len(), 1);
        // once its entry holds it, its entry says it
        list.change(|kept| {
            let concurrency = asked_of(&starting);
            kept.checkouts.push(Entry {
                checkout: starting.clone(),
                named: Named::default(),
                concurrency,
            });
        })
        .unwrap();
        forget(&starting);
        assert_eq!(own(&list, &starting).unwrap(), Some(of(4)));
        ask(&starting, To::Default);
        assert_eq!(asked_of(&starting), None);
    }
}
