//! The panel as the host runner serves it (docs/adr/0023): one page for the
//! host, served from the host runner's start, and made anew after every tick
//! of any fleet instance it holds — the host as it is read then, and each
//! checkout as the last tick of its fleet instance showed it. A checkout is
//! on the page from its fleet instance's start until it ends; one broken is
//! shown as it last was, and said broken, until it is back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::Result;

use crate::admission::Readings;
use crate::host_runner::Shared;
use crate::panel::{self, Broken, Checkout, Host, Page, Panel, Part};
use crate::serve::{self, Live};

/// The page the host runner serves, and what it is made from.
pub struct HostPanel {
    live: Live,
    /// Each checkout whose fleet instance it holds, by its root. Held while a
    /// page is made: pages are shown in the order they were made.
    checkouts: Mutex<BTreeMap<PathBuf, Checkout>>,
    /// What the host runner holds for its fleets, read as each page is made.
    shared: Shared,
    /// Where it is served, once it is.
    address: Mutex<Option<String>>,
}

impl HostPanel {
    /// The page of a host runner that holds `shared` for its fleets, served
    /// nowhere yet, and with no checkout on it.
    pub fn new(shared: Shared) -> HostPanel {
        let host = read_host(Path::new("/"), &shared);
        let page = Page { host, live: true, checkouts: vec![] };
        HostPanel {
            live: Live::new(panel::render(&page)),
            checkouts: Mutex::default(),
            shared,
            address: Mutex::default(),
        }
    }

    fn checkouts(&self) -> MutexGuard<'_, BTreeMap<PathBuf, Checkout>> {
        self.checkouts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Serves the page on `port` of this host, or on any free one where that
    /// is taken → its address, and whether `port` was taken; an error where
    /// it cannot be served at all.
    pub fn open(&self, port: u16) -> Result<(String, bool)> {
        let (port, taken) = match self.live.open(port) {
            Ok(port) => (port, false),
            Err(_) => (self.live.open(0)?, true),
        };
        let address = serve::address(port);
        *self.address.lock().unwrap_or_else(PoisonError::into_inner) = Some(address.clone());
        Ok((address, taken))
    }

    /// Where it is served; `None` where it is not.
    pub fn address(&self) -> Option<String> {
        self.address.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The fleet instance of the checkout `root` starts: one new to the page
    /// is shown starting, and one started again after a break keeps what it
    /// showed.
    pub fn starting(&self, root: &Path) {
        let mut checkouts = self.checkouts();
        let starting = Checkout { dir: root.to_path_buf(), part: Part::Starting, broken: None };
        checkouts.entry(root.to_path_buf()).or_insert(starting);
        self.remake(&checkouts);
    }

    /// The checkout `root`'s part after a tick of its fleet instance: it is
    /// broken no more.
    pub fn shown(&self, root: &Path, part: Panel) {
        let mut checkouts = self.checkouts();
        let part = Part::Shown(Box::new(part));
        checkouts
            .insert(root.to_path_buf(), Checkout { dir: root.to_path_buf(), part, broken: None });
        self.remake(&checkouts);
    }

    /// The fleet instance of the checkout `root` broke, as `broken` says.
    pub fn broke(&self, root: &Path, broken: Broken) {
        let mut checkouts = self.checkouts();
        let starting = || Checkout { dir: root.to_path_buf(), part: Part::Starting, broken: None };
        checkouts.entry(root.to_path_buf()).or_insert_with(starting).broken = Some(broken);
        self.remake(&checkouts);
    }

    /// The fleet instance of the checkout `root` ended: its checkout leaves
    /// the page.
    pub fn ended(&self, root: &Path) {
        let mut checkouts = self.checkouts();
        if checkouts.remove(root).is_some() {
            self.remake(&checkouts);
        }
    }

    /// Serves the page no more, and lets its port go: the host runner stops.
    pub fn close(&self) {
        self.live.close();
    }

    /// Shows the page made of `checkouts` and the host as it is now.
    fn remake(&self, checkouts: &BTreeMap<PathBuf, Checkout>) {
        let dir = checkouts.keys().next().map_or(Path::new("/"), PathBuf::as_path);
        let host = read_host(dir, &self.shared);
        let page = Page { host, live: true, checkouts: checkouts.values().cloned().collect() };
        self.live.show(panel::render(&page));
    }
}

/// The panel runs inside the host runner, including before its request
/// server starts. Its queue is already here; asking itself would time out
/// during startup and once more for every checkout on the list.
fn read_host(dir: &Path, shared: &Shared) -> Host {
    let snapshot = shared.now();
    let readings = Readings::with_queue(&snapshot.exclusive, &snapshot.exclusive_ran);
    Host::read(dir, crate::clock(), Ok(Some(snapshot)), &readings)
}
