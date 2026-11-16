//! A ed worker (docs/adr/0088): one whose build waits forever on
//! another's build-directory locks. A process stuck on a lock takes no CPU,
//! so no limit of a hot worker finds it (sunfmin/control#1249); this does.
//!
//! The sampler keeps, for every process of every worker, since when it has
//! taken no CPU and had no children (`Idle`). One idle for `STALL_HELD` is
//! asked of `lsof` for the build-directory locks it holds open, and each of
//! those locks for every process that holds it: where every holder is idle
//! too — for `STALL_HELD`, or, being no worker's, at no CPU with no children
//! as `ps` says it — nothing will ever release them, and its worker is
//! stalled. A build waiting on one that runs waits on a holder that takes
//! CPU or runs a compiler, and is not stalled.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Command;

use afkconvoy_core::hot::STALL_HELD;

use crate::hot::Proc;

/// The names a build-directory lock file has: cargo's, each unit's `.lock`
/// and the build and artifact directories' own.
const LOCKS: [&str; 4] = [".lock", ".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"];

/// Since when each process of the workers has taken no CPU and had no
/// children, with the CPU it had taken then.
#[derive(Debug, Default)]
pub(crate) struct Idle {
    since: HashMap<u32, (u64, u64)>,
}

impl Idle {
    /// Takes in `procs`, the processes of every worker at `at`, with `all`
    /// the host's: one that took CPU or has a child is idle from `at` on,
    /// and one no longer listed is forgotten.
    pub(crate) fn sampled(&mut self, procs: &[&Proc], all: &[Proc], at: u64) {
        let parents: BTreeSet<u32> = all.iter().map(|proc| proc.ppid).collect();
        let mut since = HashMap::with_capacity(procs.len());
        for proc in procs {
            let kept = self.since.get(&proc.pid).filter(|(cpu, _)| *cpu == proc.cpu_ms);
            let from = match kept {
                Some((_, from)) if !parents.contains(&proc.pid) => *from,
                _ => at,
            };
            since.insert(proc.pid, (proc.cpu_ms, from));
        }
        self.since = since;
    }

    /// Whether `pid` has been idle for `STALL_HELD` at `at`.
    pub(crate) fn held(&self, pid: u32, at: u64) -> bool {
        self.since.get(&pid).is_some_and(|(_, from)| at.saturating_sub(*from) >= STALL_HELD)
    }
}

/// A process holding a build-directory lock, as the stall names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holder {
    pub(crate) pid: u32,
    /// Its command, its first words.
    pub(crate) command: String,
    /// The worker it is of, `#7`; `None` for no worker's.
    pub(crate) worker: Option<String>,
    /// How many of the locks it holds open.
    pub(crate) locks: usize,
}

/// A worker's stall: its processes that wait, and every holder of the locks
/// they hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stall {
    pub(crate) waiting: Vec<u32>,
    pub(crate) locks: usize,
    pub(crate) holders: Vec<Holder>,
}

impl Stall {
    /// The stall in words, its lock holders named.
    pub(crate) fn said(&self) -> String {
        let waiting: Vec<String> = self.waiting.iter().map(u32::to_string).collect();
        let holders: Vec<String> = self
            .holders
            .iter()
            .map(|holder| {
                let of = holder.worker.as_deref().map_or_else(String::new, |w| format!("{w}, "));
                format!("{} ({of}`{}`, {} locks)", holder.pid, holder.command, holder.locks)
            })
            .collect();
        format!(
            "pid {} at no CPU with no children for {}s or more, holding {} build-directory locks; \
             their holders, every one idle: {}",
            waiting.join(", "),
            STALL_HELD,
            self.locks,
            holders.join("; ")
        )
    }
}

/// The stall of the worker whose processes idle for `STALL_HELD` are
/// `candidates`, where they are stalled; `idle` is every worker's, `all` the
/// host's processes, `worker_of` the worker each process of a worker is of.
/// `None` where they hold no lock, where `lsof` cannot be read, or where a
/// holder of their locks is busy.
pub(crate) fn of(
    candidates: &[u32],
    idle: &Idle,
    all: &[Proc],
    worker_of: &HashMap<u32, String>,
    at: u64,
) -> Option<Stall> {
    if candidates.is_empty() {
        return None;
    }
    let pids: Vec<String> = candidates.iter().map(u32::to_string).collect();
    let held = locks(&lsof(&["-p", &pids.join(",")])?);
    let paths: BTreeSet<&PathBuf> = held.iter().map(|(_, path)| path).collect();
    if paths.is_empty() {
        return None;
    }
    let mut args = vec!["--".to_string()];
    args.extend(paths.iter().map(|path| path.display().to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let holding = locks(&lsof(&args)?);
    let parents: BTreeSet<u32> = all.iter().map(|proc| proc.ppid).collect();
    let mut by_holder: BTreeMap<u32, BTreeSet<&PathBuf>> = BTreeMap::new();
    for (pid, path) in &holding {
        by_holder.entry(*pid).or_default().insert(path);
    }
    let mut holders = vec![];
    for (pid, locks) in by_holder {
        let proc = all.iter().find(|proc| proc.pid == pid);
        let idle_now = match worker_of.get(&pid) {
            Some(_) => idle.held(pid, at),
            None => proc.is_some_and(|proc| proc.cpu_share == 0) && !parents.contains(&pid),
        };
        if !idle_now {
            return None;
        }
        holders.push(Holder {
            pid,
            command: proc.map_or_else(String::new, |proc| command(&proc.line)),
            worker: worker_of.get(&pid).cloned(),
            locks: locks.len(),
        });
    }
    let waiting: BTreeSet<u32> = held.iter().map(|(pid, _)| *pid).collect();
    Some(Stall { waiting: waiting.into_iter().collect(), locks: paths.len(), holders })
}

/// What `lsof -n -P -F pn` says with `args`, or `None` where it could not
/// run. It exits 1 where some file is held by none: that is no failure.
fn lsof(args: &[&str]) -> Option<String> {
    let out =
        crate::process::output(Command::new("lsof").args(["-n", "-P", "-F", "pn"]).args(args))
            .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The build-directory locks `lsof -F pn` lists, each with its process.
fn locks(listed: &str) -> Vec<(u32, PathBuf)> {
    let mut pid = None;
    let mut found = vec![];
    for line in listed.lines() {
        if let Some(number) = line.strip_prefix('p') {
            pid = number.parse().ok();
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
            let path = PathBuf::from(name);
            let lock = path.file_name().is_some_and(|file| LOCKS.iter().any(|lock| file == *lock));
            if lock && !found.iter().any(|(p, f)| *p == pid && *f == path) {
                found.push((pid, path));
            }
        }
    }
    found
}

/// The first words of a `ps` line: its command, without its environment.
fn command(line: &str) -> String {
    let words: Vec<&str> = line.split(' ').take_while(|word| !word.contains('=')).take(4).collect();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: u32, cpu_ms: u64) -> Proc {
        Proc {
            pid,
            ppid,
            resident: 0,
            cpu_ms,
            cpu_share: 0,
            written: 0,
            line: format!("cargo test TMPDIR=/t/{pid}"),
            cwd: None,
        }
    }

    #[test]
    fn a_process_is_idle_from_its_last_cpu_or_child_on() {
        let mut idle = Idle::default();
        let (a, b) = (proc(1, 0, 50), proc(2, 0, 50));
        let child = proc(3, 2, 0);
        let all = [a.clone(), b.clone(), child.clone()];
        idle.sampled(&[&a, &b, &child], &all, 1000);
        let at = 1000 + STALL_HELD;
        idle.sampled(&[&a, &b, &child], &all, at);
        assert!(idle.held(1, at), "no CPU and no child since 1000");
        assert!(!idle.held(2, at), "it has a child");
        let busy = proc(1, 0, 51);
        idle.sampled(&[&busy], &[busy.clone()], at + 10);
        assert!(!idle.held(1, at + 10), "it took CPU");
        assert!(!idle.held(3, at + 10), "gone, it is forgotten");
    }

    #[test]
    fn the_locks_lsof_lists_are_the_build_directorys_by_their_names() {
        let listed = "p10\nf3\nn/b/debug/build/x/1/.lock\nn/b/debug/.cargo-build-lock\n\
                      n/b/debug/deps/libx.rlib\np11\nn/b/debug/build/x/1/.lock\n";
        let found = locks(listed);
        assert_eq!(
            found,
            [
                (10, PathBuf::from("/b/debug/build/x/1/.lock")),
                (10, PathBuf::from("/b/debug/.cargo-build-lock")),
                (11, PathBuf::from("/b/debug/build/x/1/.lock")),
            ]
        );
    }

    #[test]
    fn a_command_is_said_without_its_environment() {
        assert_eq!(command("cargo test -p a TMPDIR=/t HOME=/u"), "cargo test -p a");
    }

    /// A stand-in for a stuck cargo: `sleep`, with `lock` open, at no CPU
    /// and with no children; or, `busy`, a shell with `sleep` under it.
    fn stand_in(lock: &std::path::Path, busy: bool) -> std::process::Child {
        let script =
            if busy { "exec 3>>\"$1\"; sleep 600; :" } else { "exec 3>>\"$1\"; exec sleep 600" };
        Command::new("sh").args(["-c", script, "sh"]).arg(lock).spawn().unwrap()
    }

    /// Lists the host's processes until `pid` holds `lock` open as `lsof`
    /// says it.
    fn holding(pid: u32, lock: &std::path::Path) -> Vec<Proc> {
        afkconvoy_patience::until("the stand-in holds its lock", || {
            let open = lsof(&["-p", &pid.to_string()]).unwrap_or_default();
            locks(&open).iter().any(|(_, path)| path == lock).then(crate::hot::listed)
        })
    }

    #[test]
    fn a_worker_stuck_on_locks_only_idle_processes_hold_is_stalled_and_one_a_build_holds_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().canonicalize().unwrap().join("debug/build/x/1");
        std::fs::create_dir_all(&unit).unwrap();
        let lock = unit.join(".lock");
        let mut stuck = stand_in(&lock, false);
        let all = holding(stuck.id(), &lock);
        let mine: Vec<&Proc> = all.iter().filter(|proc| proc.pid == stuck.id()).collect();
        let worker_of = HashMap::from([(stuck.id(), "#7".to_string())]);
        let mut idle = Idle::default();
        idle.sampled(&mine, &all, 1000);
        let at = 1000 + STALL_HELD;
        idle.sampled(&mine, &all, at);
        let stall = of(&[stuck.id()], &idle, &all, &worker_of, at);
        let mut building = stand_in(&lock, true);
        let all = holding(building.id(), &lock);
        let waited_on = of(&[stuck.id()], &idle, &all, &worker_of, at);
        for child in [&mut stuck, &mut building] {
            let _ = Command::new("pkill").args(["-P", &child.id().to_string()]).status();
            let _ = child.kill();
            let _ = child.wait();
        }
        let stall = stall.expect("stalled");
        assert_eq!((stall.waiting.clone(), stall.locks), (vec![stuck.id()], 1));
        assert_eq!(stall.holders.len(), 1, "{stall:?}");
        assert_eq!(stall.holders[0].worker.as_deref(), Some("#7"));
        assert!(stall.said().contains(&format!("{} (#7, `sleep 600`", stuck.id())), "{stall:?}");
        assert_eq!(waited_on, None, "a holder runs a child: the build moves");
    }
}
