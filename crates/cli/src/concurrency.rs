//! What is left of a checkout's concurrency: nothing bounds a fleet's workers
//! any more, and admission alone decides each start (docs/adr/0072). A `run`
//! still accepts `--concurrency` for one release, and keeps what it says with
//! the checkout's entry on the checkout list, where it changes nothing:
//! `doctor` says it is ignored, and `--concurrency default` drops it. After
//! that release the flag goes.
//!
//! A checkout the list holds no entry of yet — its first tick has not ended,
//! or a `run --foreground` holds it — keeps what its `run` asked in that
//! process alone until its first tick puts it on the list with it.

use std::num::NonZeroUsize;
use std::path::Path;

use anyhow::Result;

use crate::checkout_list::{List, asked};
use crate::host_settings::To;

/// Why a concurrency is ignored, as `run` and `doctor` say it.
pub const IGNORED: &str = "no number bounds a fleet's workers: admission alone decides each start, by the host's load, memory and disk, the usage windows of the accounts its workers start on, and the host's exclusive queue";

/// `--concurrency` as said: a whole number, at least 1, or `default`.
pub fn claims_or_default(said: &str) -> Result<To<NonZeroUsize>, String> {
    To::parse(said, |said| {
        said.parse()
            .map_err(|_| format!("`{said}` is not a whole number of at least 1, nor `default`"))
    })
}

/// The concurrency the checkout at `root` still says on the host of `list`,
/// which is ignored, where it says one: the one with its entry; where the list holds no entry of it,
/// what its `run` asked in this process.
pub fn own(list: &List, root: &Path) -> Result<Option<NonZeroUsize>> {
    let asked = asked();
    let listed = list.read()?;
    Ok(match listed.checkouts.iter().find(|entry| entry.checkout == root) {
        Some(entry) => entry.concurrency,
        None => asked.get(root).and_then(|asked| asked.concurrency),
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
    asked().entry(root.to_path_buf()).or_default().concurrency = to.kept();
}

/// What the `run` of the checkout at `root` asked in this process, where it
/// asked one: what its first tick puts on the list with it.
pub fn asked_of(root: &Path) -> Option<NonZeroUsize> {
    crate::checkout_list::asked_of(root).concurrency
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkout_list::{Entry, forget};
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
        let entry = Entry {
            checkout: listed.clone(),
            named: Named::default(),
            concurrency: None,
            tracker: None,
            harness: None,
            harness_applied: None,
        };
        list.change(|kept| kept.checkouts.push(entry)).unwrap();
        assert_eq!(own(&list, &listed).unwrap(), None, "none said");

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
                tracker: None,
                harness: None,
                harness_applied: None,
            });
        })
        .unwrap();
        forget(&starting);
        assert_eq!(own(&list, &starting).unwrap(), Some(of(4)));
        ask(&starting, To::Default);
        assert_eq!(asked_of(&starting), None);
    }
}
