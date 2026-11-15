//! The disk the host builds on, kept by its host runner (sunfmin/control#1126):
//! the effect half of `afkconvoy_core::disk`. Every `LOOK_EVERY` the runner
//! reads the disk under the checkouts on its list against the floor their
//! builds need kept free. Under it, the `target/` of each worktree no one
//! builds in — its worker ended, or nothing wrote it for `IDLE_AFTER` — is
//! reclaimed first; the exclusive queue grants no new heavy command while the
//! disk is still under it after that. A disk under its floor, or falling
//! faster than `FALLING`, is an episode, which `doctor` reports as a host
//! finding with the worktrees that build and their `target/` sizes.

use std::collections::VecDeque;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use afkconvoy_core::admission::{Disk, SHARES};
use afkconvoy_core::disk::{
    BUILDING_WITHIN, FALL_OVER, FALLING, IDLE_AFTER, Target, Why, fall, floor, reserve, to_reclaim,
};
use afkconvoy_core::exclusive::{Low, Ticket};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::doctor::{Finding, Kind, finding};
use crate::host_settings::Settings;
use crate::hot::Proc;
use crate::measure;

/// How often the host runner reads the disk.
pub const LOOK_EVERY: Duration = Duration::from_secs(10);

/// How many reclaimed `target/` dirs an episode keeps the names of.
const RECLAIMED_KEPT: usize = 50;

/// The directory cargo builds into, in a worktree.
const TARGET: &str = "target";

/// One worktree of a checkout, as git keeps it: its files and its own git dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    git_dir: PathBuf,
}

impl Worktree {
    fn target(&self) -> PathBuf {
        self.path.join(TARGET)
    }

    /// Whether a worker was started in it: its brief is there.
    fn a_workers(&self) -> bool {
        self.git_dir.join(crate::orca::BRIEF_FILE).exists()
    }

    /// Its fresh build on record (`measure::fresh`), in bytes.
    fn fresh(&self) -> Option<u64> {
        std::fs::read_to_string(self.git_dir.join(measure::FRESH_NOTE)).ok()?.trim().parse().ok()
    }
}

/// The worktrees of the checkout at `checkout` but the checkout itself, as
/// its git dir lists them: each `worktrees/<name>/gitdir` names the `.git`
/// of one. One whose files are gone is left out.
pub fn worktrees(checkout: &Path) -> Vec<Worktree> {
    let Ok(entries) = std::fs::read_dir(checkout.join(".git").join("worktrees")) else {
        return vec![];
    };
    let mut found = vec![];
    for entry in entries.map_while(Result::ok) {
        let git_dir = entry.path();
        let Ok(named) = std::fs::read_to_string(git_dir.join("gitdir")) else { continue };
        let Some(path) = Path::new(named.trim_end()).parent() else { continue };
        if path.is_dir() {
            found.push(Worktree { path: path.to_path_buf(), git_dir });
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The bytes the builds that may run at once on this host take, measured:
/// the largest fresh build on record in the worktrees of `checkouts`, times
/// the queue's `lanes` and the worktrees a worker was started in, which
/// build beside them. `None`: none is on record, or the host said its own
/// disk share (`admit_disk`), which then stands alone.
pub fn reserve_of(settings: &Settings, checkouts: &[PathBuf]) -> Option<u64> {
    if settings.admit_disk.is_some() {
        return None;
    }
    let worktrees: Vec<Worktree> = checkouts.iter().flat_map(|dir| worktrees(dir)).collect();
    let build = worktrees.iter().filter_map(Worktree::fresh).max()?;
    let lanes = settings.lanes();
    let workers = worktrees.iter().filter(|worktree| worktree.a_workers()).count() as u64;
    let at_once = lanes.worker.saturating_add(lanes.fast).saturating_add(lanes.slow) + workers;
    Some(reserve(build, at_once))
}

/// The checkouts on this host's checkout list.
pub fn listed() -> Vec<PathBuf> {
    crate::checkout_list::List::of_host()
        .and_then(|list| list.read())
        .map(|listed| listed.checkouts.into_iter().map(|entry| entry.checkout).collect())
        .unwrap_or_default()
}

/// The floor the disk `disk` is held to under `settings`, with the measured
/// `reserve` (`reserve_of`).
pub fn floor_of(settings: &Settings, disk: Disk, reserve: Option<u64>) -> u64 {
    floor(disk.size, settings.admit_disk.unwrap_or(SHARES.disk), reserve)
}

/// When anything under `dir` was last written, in epoch seconds, by the
/// directories down to `depth` under it: cargo makes files in
/// `target/<profile>/deps`, `.fingerprint` and `incremental` as it builds,
/// which changes theirs.
fn written(dir: &Path, depth: u32) -> Option<u64> {
    let meta = std::fs::symlink_metadata(dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let mut newest = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    if depth > 0
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.map_while(Result::ok) {
            if let Some(at) = written(&entry.path(), depth - 1) {
                newest = newest.max(at);
            }
        }
    }
    Some(newest)
}

/// The `target/` of `worktree`, as `afkconvoy_core::disk` judges it, with
/// `running` the queue's tickets and `procs` the process table; `None`: it
/// has none.
fn target_of(worktree: &Worktree, running: &[Ticket], procs: &[Proc]) -> Option<Target> {
    let target = worktree.target();
    if !target.is_dir() {
        return None;
    }
    let queued = running
        .iter()
        .any(|ticket| ticket.running() && Path::new(&ticket.dir).starts_with(&worktree.path));
    Some(Target {
        worktree: worktree.path.display().to_string(),
        written: written(&target, 3),
        queued,
        worker: !crate::hot::tree(procs, &worktree.path).is_empty(),
    })
}

/// One `target/` reclaimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reclaimed {
    pub worktree: PathBuf,
    pub why: Why,
    pub at: u64,
}

/// Reclaims the `target/` of each of `worktrees` no one builds in, at `now`
/// (`afkconvoy_core::disk::to_reclaim`): each is set aside into the trash
/// beside its worktree, on its filesystem, and deleted by a detached child.
/// `running` is the exclusive queue, `procs` the process table. One that
/// cannot be set aside is left as it is.
pub(crate) fn reclaim(
    worktrees: &[Worktree],
    running: &[Ticket],
    procs: &[Proc],
    now: u64,
) -> Vec<Reclaimed> {
    let judged: Vec<(&Worktree, Target)> = worktrees
        .iter()
        .filter_map(|worktree| Some((worktree, target_of(worktree, running, procs)?)))
        .collect();
    let targets: Vec<Target> = judged.iter().map(|(_, target)| target.clone()).collect();
    let mut reclaimed = vec![];
    for (at, why) in to_reclaim(&targets, now, IDLE_AFTER) {
        let worktree = judged[at].0;
        let Some(beside) = worktree.path.parent() else { continue };
        if crate::trash::discard_in(&worktree.target(), beside).is_ok() {
            reclaimed.push(Reclaimed { worktree: worktree.path.clone(), why, at: now });
        }
    }
    reclaimed
}

/// What the host runner last saw of the disk its checkouts build on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    /// A checkout on the volume, which `df` was asked of.
    pub dir: PathBuf,
    pub free: u64,
    pub size: u64,
    /// The bytes it is held to keep free (`floor_of`).
    pub floor: u64,
    /// How fast its free space fell over the last `FALL_OVER` seconds, in
    /// bytes a minute; `None`: too few looks yet.
    pub fall: Option<u64>,
    /// Whether the queue grants nothing new: under its floor still, after
    /// reclaiming what it could.
    pub held: bool,
    /// Since when it has been under its floor or falling past `FALLING`, in
    /// epoch seconds; `None`: it is neither.
    pub since: Option<u64>,
    /// The `target/` dirs reclaimed in this episode, the latest `RECLAIMED_KEPT`.
    #[serde(default)]
    pub reclaimed: Vec<Reclaimed>,
}

impl Seen {
    /// What the queue is told: the disk under its floor, where it holds.
    pub fn low(&self) -> Option<Low> {
        self.held.then_some(Low { free: self.free, floor: self.floor })
    }
}

/// The host runner's look at the disk, kept across looks: its recent free
/// space, and the episode it is in.
#[derive(Default)]
pub struct Keeper {
    volume: Option<u64>,
    samples: VecDeque<(u64, u64)>,
    seen: Option<Seen>,
}

impl Keeper {
    /// Looks at the disk under `checkouts` at `now`, with the exclusive queue
    /// `queue`: reclaims first where it is under its floor, and says whether
    /// the queue holds → what it saw. `None`: no checkout's disk could be read.
    pub fn look(&mut self, checkouts: &[PathBuf], queue: &[Ticket], now: u64) -> Option<Seen> {
        let settings = crate::host_settings::of_host().unwrap_or_default();
        let disk = |dir: &Path| crate::admission::disk(dir).ok();
        self.look_with(&settings, checkouts, queue, now, disk, crate::hot::listed)
    }

    /// `look`, under `settings`, with the disk read by `read` and the process
    /// table by `procs`, which only a reclaim reads.
    fn look_with(
        &mut self,
        settings: &Settings,
        checkouts: &[PathBuf],
        queue: &[Ticket],
        now: u64,
        read: impl Fn(&Path) -> Option<Disk>,
        procs: impl FnOnce() -> Vec<Proc>,
    ) -> Option<Seen> {
        let reserve = reserve_of(settings, checkouts);
        // The volume closest to its floor is the one that holds.
        let mut tightest: Option<(PathBuf, u64, Disk, u64)> = None;
        for dir in checkouts {
            let Some(disk) = read(dir) else { continue };
            let volume = std::fs::metadata(dir).map(|meta| meta.dev()).unwrap_or(0);
            let floor = floor_of(settings, disk, reserve);
            let margin = disk.free as i128 - floor as i128;
            let tighter = match &tightest {
                Some((_, _, held, held_floor)) => margin < held.free as i128 - *held_floor as i128,
                None => true,
            };
            if tighter {
                tightest = Some((dir.clone(), volume, disk, floor));
            }
        }
        let (dir, volume, mut disk, floor) = tightest?;
        let mut reclaimed = vec![];
        if disk.free < floor {
            let on_volume: Vec<Worktree> = checkouts
                .iter()
                .filter(|dir| std::fs::metadata(dir).is_ok_and(|meta| meta.dev() == volume))
                .flat_map(|dir| worktrees(dir))
                .collect();
            reclaimed = reclaim(&on_volume, queue, &procs(), now);
            if !reclaimed.is_empty()
                && let Some(again) = read(&dir)
            {
                disk = again;
            }
        }
        Some(self.saw(dir, volume, disk, floor, reclaimed, now))
    }

    /// Notes one look: `disk` of `volume`, under `dir`, held to `floor`, after
    /// `reclaimed` → what it saw.
    fn saw(
        &mut self,
        dir: PathBuf,
        volume: u64,
        disk: Disk,
        floor: u64,
        reclaimed: Vec<Reclaimed>,
        now: u64,
    ) -> Seen {
        if self.volume != Some(volume) {
            self.volume = Some(volume);
            self.samples.clear();
            self.seen = None;
        }
        self.samples.push_back((now, disk.free));
        while self.samples.front().is_some_and(|(at, _)| now.saturating_sub(*at) > FALL_OVER) {
            self.samples.pop_front();
        }
        let samples: Vec<(u64, u64)> = self.samples.iter().copied().collect();
        let fall = fall(&samples, FALL_OVER);
        let held = disk.free < floor;
        let episode = held || fall.is_some_and(|fall| fall >= FALLING);
        let before = self.seen.take().filter(|seen| seen.since.is_some());
        let since = episode.then(|| before.as_ref().and_then(|seen| seen.since).unwrap_or(now));
        let mut kept = match (&before, episode) {
            (Some(before), true) => before.reclaimed.clone(),
            _ => vec![],
        };
        kept.extend(reclaimed);
        let drop = kept.len().saturating_sub(RECLAIMED_KEPT);
        kept.drain(..drop);
        let seen = Seen {
            dir,
            free: disk.free,
            size: disk.size,
            floor,
            fall,
            held,
            since,
            reclaimed: kept,
        };
        self.seen = Some(seen.clone());
        seen
    }
}

/// A worktree that builds now, and the bytes its `target/` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Building {
    pub worktree: PathBuf,
    pub target_bytes: u64,
}

/// The worktrees of `checkouts` whose `target/` was written within
/// `BUILDING_WITHIN` of `now`, each with its size, the largest first.
pub fn building(checkouts: &[PathBuf], now: u64) -> Vec<Building> {
    let mut found: Vec<Building> = checkouts
        .iter()
        .flat_map(|dir| worktrees(dir))
        .filter(|worktree| {
            written(&worktree.target(), 3)
                .is_some_and(|at| now.saturating_sub(at) < BUILDING_WITHIN)
        })
        .map(|worktree| Building {
            target_bytes: measure::bytes_under(&worktree.target()),
            worktree: worktree.path,
        })
        .collect();
    found.sort_by_key(|build| std::cmp::Reverse(build.target_bytes));
    found
}

/// The host finding of a disk episode the host runner says (`Seen::since`):
/// the disk under its floor, or falling past `FALLING`, with the worktrees
/// of `checkouts` that build and their `target/` sizes. None outside one.
pub fn finding_of(seen: &Seen, checkouts: &[PathBuf], now: u64) -> Option<Finding> {
    let since = seen.since?;
    let said = measure::said;
    let falling = seen.fall.map_or(String::new(), |fall| format!(", falling {}/min", said(fall)));
    let what = format!(
        "the disk the host builds on has {} free of {}{falling}, held to keep {} free{}",
        said(seen.free),
        said(seen.size),
        said(seen.floor),
        if seen.held { ": the queue grants no new heavy command" } else { "" },
    );
    let builds = building(checkouts, now);
    let mut evidence = format!("since {since}, under {}", seen.dir.display());
    if builds.is_empty() {
        evidence.push_str("; no worktree builds now");
    } else {
        let each: Vec<String> = builds
            .iter()
            .map(|build| format!("{} ({})", build.worktree.display(), said(build.target_bytes)))
            .collect();
        evidence.push_str(&format!("; building: {}", each.join(", ")));
    }
    if !seen.reclaimed.is_empty() {
        let each: Vec<String> =
            seen.reclaimed.iter().map(|done| done.worktree.display().to_string()).collect();
        evidence.push_str(&format!("; target/ reclaimed in: {}", each.join(", ")));
    }
    Some(finding(
        Kind::DiskFilling,
        json!(["disk-filling"]),
        what,
        evidence,
        format!("df -h {}", seen.dir.display()),
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::SystemTime;

    use super::*;

    const GIB: u64 = 1 << 30;

    /// A checkout with a worktree for each of `names`, each with a `target/`
    /// holding a build → the checkout, and each worktree's path.
    fn checkout(root: &Path, names: &[&str]) -> (PathBuf, Vec<PathBuf>) {
        let checkout = root.join("checkout");
        let mut paths = vec![];
        for name in names {
            let path = root.join(name);
            let git_dir = checkout.join(".git/worktrees").join(name);
            std::fs::create_dir_all(&git_dir).unwrap();
            std::fs::write(git_dir.join("gitdir"), format!("{}\n", path.join(".git").display()))
                .unwrap();
            std::fs::create_dir_all(path.join("target/debug/deps")).unwrap();
            std::fs::write(path.join("target/debug/deps/libbuilt.rlib"), vec![0; 4096]).unwrap();
            std::fs::write(path.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();
            paths.push(path);
        }
        (checkout, paths)
    }

    /// Makes everything under `path` last written `ago` seconds before `now`.
    fn written_ago(path: &Path, now: u64, ago: u64) {
        let at = UNIX_EPOCH + Duration::from_secs(now - ago);
        for entry in std::fs::read_dir(path).unwrap().map_while(Result::ok) {
            if entry.path().is_dir() {
                written_ago(&entry.path(), now, ago);
            } else {
                std::fs::File::options()
                    .write(true)
                    .open(entry.path())
                    .unwrap()
                    .set_modified(at)
                    .unwrap();
            }
        }
        std::fs::File::open(path).unwrap().set_modified(at).unwrap();
    }

    /// A process of the worker in `worktree`: started with its `TMPDIR`.
    fn worker_in(worktree: &Path, pid: u32) -> Proc {
        Proc {
            pid,
            ppid: 100,
            resident: 0,
            cpu_ms: 0,
            cpu_share: 0,
            written: 0,
            line: format!("claude TMPDIR={}", crate::worker_tmp::of(worktree).display()),
            cwd: None,
        }
    }

    fn running_in(worktree: &Path) -> Ticket {
        Ticket {
            number: 1,
            pid: 2,
            command: "./gate.sh fast".into(),
            dir: worktree.display().to_string(),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            whole_repo: false,
            gate: None,
            asked: 0,
            ran: Some(0),
            note: Default::default(),
        }
    }

    #[test]
    fn idle_and_ended_targets_are_reclaimed_before_a_disk_refusal_and_a_building_one_never() {
        let root = tempfile::tempdir().unwrap();
        let names = ["ended", "idle", "building", "gating", "working"];
        let (checkout, paths) = checkout(root.path(), &names);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let [ended, idle, building, gating, working] = &paths[..] else { unreachable!() };
        written_ago(&ended.join(TARGET), now, 3600); // its worker is gone
        written_ago(&idle.join(TARGET), now, IDLE_AFTER + 60); // its worker waits
        written_ago(&building.join(TARGET), now, 30); // cargo writes it, no worker
        written_ago(&gating.join(TARGET), now, IDLE_AFTER + 60); // a gate runs there
        written_ago(&working.join(TARGET), now, 3600); // its worker works on
        let procs = vec![worker_in(idle, 10), worker_in(gating, 11), worker_in(working, 12)];
        let queue = vec![running_in(gating)];
        let mut settings = Settings::default();
        settings.admit_disk = Some(10);
        let size = 100 * GIB;

        // Under its floor until the reclaim, over it after.
        let reclaimed_yet = Cell::new(false);
        let read = |_: &Path| {
            let free = if reclaimed_yet.get() { 20 * GIB } else { 2 * GIB };
            Some(Disk { free, size })
        };
        let procs_read = || {
            reclaimed_yet.set(true);
            procs.clone()
        };
        let mut keeper = Keeper::default();
        let checkouts = [checkout.clone()];
        let seen = keeper.look_with(&settings, &checkouts, &queue, now, read, procs_read).unwrap();

        let done: Vec<(&Path, Why)> =
            seen.reclaimed.iter().map(|done| (done.worktree.as_path(), done.why)).collect();
        assert_eq!(done, [(ended.as_path(), Why::Ended), (idle.as_path(), Why::Idle)]);
        assert!(!seen.held, "space returned by reclaiming: nothing is refused");
        assert_eq!(seen.low(), None);
        for gone in [ended, idle] {
            assert!(!gone.join(TARGET).exists(), "{} is reclaimed", gone.display());
            assert!(gone.join(".git").exists(), "its worktree stays");
        }
        for kept in [building, gating, working] {
            assert!(kept.join(TARGET).is_dir(), "{} is kept", kept.display());
        }
        assert!(
            !names.iter().any(|name| root.path().join(name).join(crate::trash::TRASH_DIR).exists()),
            "nothing is set aside into a worktree, where git would see it"
        );

        // Still under its floor with nothing left to reclaim: the queue holds,
        // and what builds is never reclaimed for it.
        let low = |_: &Path| Some(Disk { free: 2 * GIB, size });
        let seen = keeper.look_with(&settings, &checkouts, &queue, now + 10, low, || procs.clone());
        let seen = seen.unwrap();
        assert!(seen.held);
        assert_eq!(seen.low(), Some(Low { free: 2 * GIB, floor: 10 * GIB }));
        assert_eq!(seen.since, Some(now + 10), "the episode starts at the look still under it");
        for kept in [building, gating, working] {
            assert!(kept.join(TARGET).is_dir(), "{} is still kept", kept.display());
        }
    }

    #[test]
    fn the_floor_is_measured_from_fresh_builds_times_what_may_build_at_once() {
        let root = tempfile::tempdir().unwrap();
        let (checkout, _) = checkout(root.path(), &["a", "b", "c"]);
        let git = checkout.join(".git/worktrees");
        std::fs::write(git.join("a").join(measure::FRESH_NOTE), format!("{}\n", 4 * GIB)).unwrap();
        std::fs::write(git.join("b").join(measure::FRESH_NOTE), format!("{}\n", 5 * GIB)).unwrap();
        for worker in ["a", "b"] {
            std::fs::write(git.join(worker).join(crate::orca::BRIEF_FILE), "brief").unwrap();
        }
        let settings = Settings::default();
        let lanes = settings.lanes();
        let at_once = lanes.worker + lanes.fast + lanes.slow + 2;
        let checkouts = [checkout];
        assert_eq!(reserve_of(&settings, &checkouts), Some(5 * GIB * at_once));
        let mut said = Settings::default();
        said.admit_disk = Some(15);
        assert_eq!(reserve_of(&said, &checkouts), None, "the host's own share stands alone");
        let disk = Disk { free: 0, size: 460 * GIB };
        assert_eq!(floor_of(&said, disk, None), 460 * GIB / 100 * 15);
    }

    #[test]
    fn an_episode_is_a_finding_naming_what_builds_and_its_target_size() {
        let root = tempfile::tempdir().unwrap();
        let (checkout, paths) = checkout(root.path(), &["hot", "cold"]);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        written_ago(&paths[0].join(TARGET), now, 5);
        written_ago(&paths[1].join(TARGET), now, IDLE_AFTER);
        let seen = Seen {
            dir: checkout.clone(),
            free: 2 * GIB,
            size: 460 * GIB,
            floor: 60 * GIB,
            fall: Some(9 * GIB),
            held: true,
            since: Some(now - 120),
            reclaimed: vec![],
        };
        let found = finding_of(&seen, std::slice::from_ref(&checkout), now).unwrap();
        assert_eq!(found.kind, Kind::DiskFilling);
        assert!(found.what.contains("falling 9.0 GiB/min"), "{}", found.what);
        assert!(found.what.contains("grants no new heavy command"), "{}", found.what);
        let hot = format!("{} (4.0 KiB)", paths[0].display());
        assert!(found.evidence.contains(&hot), "{}", found.evidence);
        assert!(!found.evidence.contains(&paths[1].display().to_string()), "{}", found.evidence);
        let calm = Seen { since: None, held: false, ..seen };
        assert!(finding_of(&calm, &[checkout], now).is_none(), "no episode, no finding");
    }
}
