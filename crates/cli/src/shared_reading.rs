//! A shared reading: a GitHub reading every process on the host waiting on the
//! same thing asks for, read by one of them at most once an interval and
//! kept for the rest (sunfmin/control#1111). A `land` waiting for its round
//! or its predecessor asks whether a fleet lock stands about once a second;
//! 49 of them on one host spent the login's REST core quota within the hour.
//!
//! The reading is kept per login and host, beside the quota note
//! (`gh_quota::Scope::shared`), under a file lock that is held while GitHub
//! is read: a second waiter waits on the first one's read and answers from
//! it. A failed read is kept as well, so that a waiter does not ask again
//! within the interval either. Past the host's GitHub share of the hour's
//! quota (`--github-share`), the kept reading answers until the quota
//! resets: the shared readings leave the rest of the hour to the workers' own calls.

use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::gh::GitHub;
use crate::gh_quota::Observation;

/// The seconds between two reads of one shared reading on a host.
pub const EVERY: u64 = 30;

/// The share of the hour's quota the host's shared readings stay under where its
/// settings say none, a whole percent (`--github-share`).
pub const GITHUB_SHARE: u64 = 60;

/// The hour's REST core quota where GitHub has not said it.
const LIMIT: u64 = 5000;

/// One shared reading as it is kept: when it was read and what it said.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    at: u64,
    body: Option<String>,
    error: Option<String>,
}

/// How a shared reading is paced: every `every` seconds, and not again before
/// the quota's reset once its login has used `share` percent of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pace<'a> {
    pub(crate) every: u64,
    pub(crate) share: u64,
    pub(crate) quota: Option<&'a Observation>,
}

impl Pace<'_> {
    /// Whether the login has used its share of the hour's quota at `now`.
    fn spent(&self, now: u64) -> Option<u64> {
        let quota = self.quota?;
        let remaining = quota.remaining?;
        if quota.reset <= now {
            return None;
        }
        let limit = quota.limit.unwrap_or(LIMIT).max(1);
        let used = limit.saturating_sub(remaining);
        (used.saturating_mul(100) >= self.share.saturating_mul(limit)).then_some(quota.reset)
    }

    /// Whether what was read at `at` still answers at `now`.
    fn answers(&self, at: u64, now: u64) -> bool {
        now < at.saturating_add(self.every) || self.spent(now).is_some_and(|reset| at < reset)
    }
}

/// The shared reading `key` in `dir` at `now`: what was kept, while `pace` says
/// it still answers, else what `read` says now, kept for the others.
pub(crate) fn shared(
    dir: &Path,
    key: &str,
    now: impl Fn() -> u64,
    pace: Pace<'_>,
    read: impl FnOnce() -> Result<String>,
) -> Result<String> {
    std::fs::create_dir_all(dir)?;
    let name: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    let name = if name.len() > 200 { format!("{:016x}", fnv(key)) } else { name };
    let file = dir.join(format!("{name}.json"));
    let lock = std::fs::File::create(dir.join(format!("{name}.lock")))?;
    lock.lock()?;
    let kept: Option<Kept> =
        std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if let Some(kept) = kept
        && pace.answers(kept.at, now())
    {
        return said(kept);
    }
    let at = now();
    let made = read();
    let kept = match &made {
        Ok(body) => Kept { at, body: Some(body.clone()), error: None },
        Err(error) => Kept { at, body: None, error: Some(format!("{error:#}")) },
    };
    let temp = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temp, serde_json::to_vec(&kept)?)?;
    std::fs::rename(temp, &file)?;
    made
}

fn said(kept: Kept) -> Result<String> {
    match (kept.body, kept.error) {
        (Some(body), _) => Ok(body),
        (None, error) => Err(anyhow!(
            "{} (a shared reading of this host, read again after {EVERY} seconds)",
            error.unwrap_or_default()
        )),
    }
}

/// A stable name for a key too long to be one.
fn fnv(key: &str) -> u64 {
    key.bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The REST read `args` of `gh`'s checkout as the host's waiting processes
/// share it: `what` names it among the shared readings of the checkout's repo. Where
/// the login or the repo cannot be told, it is read as it is, unshared.
pub(crate) fn rest(gh: &GitHub, what: &str, args: &[&str]) -> Result<String> {
    let Ok(scope) = crate::gh_quota::Scope::of(gh) else { return gh.read(args) };
    let repo = crate::git::Repo::at(&gh.dir).and_then(|r| crate::checkout_list::named_repo(&r));
    let Ok(repo) = repo else { return gh.read(args) };
    let share = crate::host_settings::of_host().map_or(GITHUB_SHARE, |s| s.github_share());
    let core = scope.read("core").ok();
    let pace = Pace { every: EVERY, share, quota: core.as_ref() };
    shared(&scope.shared(), &format!("{repo} {what}"), crate::gh_quota::now, pace, || gh.read(args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const PACE: Pace<'static> = Pace { every: EVERY, share: GITHUB_SHARE, quota: None };

    #[test]
    fn forty_waiting_lands_make_one_call_an_interval_between_them() {
        let dir = tempfile::tempdir().unwrap();
        let calls = AtomicU64::new(0);
        let clock = AtomicU64::new(1_000);
        let wait = |n: u64| {
            shared(
                dir.path(),
                "acme/app locks",
                || clock.load(Ordering::SeqCst),
                PACE,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    Ok(format!("read by {n}"))
                },
            )
            .unwrap()
        };
        let lands = |round: u64| {
            std::thread::scope(|scope| {
                let waiting: Vec<_> = (0..40).map(|n| scope.spawn(move || wait(n))).collect();
                let said: Vec<String> = waiting.into_iter().map(|w| w.join().unwrap()).collect();
                assert!(said.windows(2).all(|two| two[0] == two[1]), "round {round}: {said:?}");
            });
        };
        lands(0);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one call for the 40 of them");
        clock.fetch_add(EVERY - 1, Ordering::SeqCst);
        lands(1);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "none more within the interval");
        clock.fetch_add(1, Ordering::SeqCst);
        lands(2);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one more for the next interval");
    }

    #[test]
    fn a_failed_read_is_not_asked_again_within_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let calls = AtomicU64::new(0);
        let fail = |now: u64| {
            shared(
                dir.path(),
                "acme/app locks",
                || now,
                PACE,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow!("API rate limit exceeded"))
                },
            )
        };
        for _ in 0..40 {
            assert!(fail(1_000).unwrap_err().to_string().contains("rate limit"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(fail(1_000 + EVERY).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn past_its_share_of_the_hour_the_host_reads_again_only_after_the_reset() {
        let dir = tempfile::tempdir().unwrap();
        let calls = AtomicU64::new(0);
        let reset = 10_000;
        let ask = |now: u64, remaining: u64| {
            let quota = Observation { remaining: Some(remaining), reset, limit: Some(5000) };
            let pace = Pace { every: EVERY, share: 60, quota: Some(&quota) };
            shared(
                dir.path(),
                "acme/app locks",
                || now,
                pace,
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(String::new())
                },
            )
            .unwrap();
        };
        ask(1_000, 2_001);
        ask(1_000 + EVERY, 2_001);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "under 60% used, it reads each interval");
        ask(1_000 + 2 * EVERY, 2_000);
        ask(9_999, 2_000);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "at 60% used, the kept reading answers");
        ask(reset, 2_000);
        assert_eq!(calls.load(Ordering::SeqCst), 3, "the reset starts the hour again");
    }
}
