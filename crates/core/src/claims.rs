//! Claims: the lock that makes one issue one fleet instance's, and the
//! heartbeat that says its owner is alive.

use flux_rs::attrs::*;

use crate::layout::Layout;
use crate::lease::{heartbeat_due, is_stale};
use crate::record::{self, Fields};
use crate::tracker::Tracker;

/// The kinds of ref a scan reads: a claim's is named for its issue, a
/// heartbeat's for its instance.
pub const CLAIM_KIND: &str = "claim/";
pub const HEARTBEAT_KIND: &str = "heartbeat/";
/// Claims on human waits, separate from worker and run claims.
pub const WAIT_KIND: &str = "wait/";

/// The claim on a human wait of this tracker. It consumes no worker slot.
pub fn wait_ref(layout: Layout, tracker: &Tracker, number: u64) -> String {
    layout.under(&format!("{WAIT_KIND}{}", tracker.claim_key(number)))
}

/// The ref that is the claim on issue `number` of `tracker`: named for the
/// tracker too where it is another repo (`Tracker::claim_key`), so that two
/// fleets of one repo that work two trackers never claim one ref.
pub fn claim_ref(layout: Layout, tracker: &Tracker, number: u64) -> String {
    layout.under(&format!("{CLAIM_KIND}{}", tracker.claim_key(number)))
}

/// The ref that is the claim on a run — work that is no issue's — keyed `key`:
/// a claim like an issue's. No issue's number reads as a run's key.
pub fn run_ref(layout: Layout, key: &str) -> String {
    layout.under(&format!("{CLAIM_KIND}{key}"))
}

/// The kind of ref that says a run's worker declared it over, named as the
/// run's claim is.
pub const OVER_KIND: &str = "over/";
const OVER: &str = "convoy:run-over";

/// The ref that says the run keyed `key` was declared over.
pub fn over_ref(layout: Layout, key: &str) -> String {
    layout.under(&format!("{OVER_KIND}{key}"))
}

/// The record that the run whose claim is at `claim` is over: it names the
/// claim, so that it says nothing of a later run under the same key.
pub fn write_over(claim: &str) -> String {
    record::write(OVER, &[("claim", claim)])
}

/// A commit subject → the sha of the claim on the run it declares over.
pub fn read_over(subject: &str) -> Option<String> {
    record::read(OVER, subject)?.get("claim").map(str::to_string)
}

/// The ref that carries `instance`'s heartbeat.
pub fn heartbeat_ref(layout: Layout, instance: &InstanceId) -> String {
    layout.under(&format!("{HEARTBEAT_KIND}{instance}"))
}

/// The name a fleet instance claims under: `[a-z0-9][a-z0-9-]{0,39}`. It is a
/// ref's last path segment, so nothing else is ever let in as one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn parse(text: &str) -> Result<InstanceId, String> {
        let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        let grammar = text.len() <= 40
            && text.bytes().next().is_some_and(ok)
            && text.bytes().all(|b| ok(b) || b == b'-');
        if grammar {
            Ok(InstanceId(text.to_string()))
        } else {
            Err(format!("{text:?} is not an instance id: it must match [a-z0-9][a-z0-9-]{{0,39}}"))
        }
    }

    /// A new id for a fleet starting on `host`: the host's name, as far as the
    /// grammar takes it, and `salt` to tell two runs apart.
    pub fn mint(host: &str, salt: u32) -> InstanceId {
        let mut name = String::new();
        for c in host.chars() {
            if name.len() >= 24 {
                break;
            }
            if c.is_ascii_alphanumeric() {
                name.push(c.to_ascii_lowercase());
            } else if !name.is_empty() && !name.ends_with('-') {
                name.push('-');
            }
        }
        let name = name.trim_end_matches('-');
        InstanceId(format!("{}-{salt:08x}", if name.is_empty() { "fleet" } else { name }))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who holds a claim: what its ref's record says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub instance: String,
    pub host: String,
    pub ts: u64,
}

const CLAIM: &str = "convoy:claim";
const HEARTBEAT: &str = "convoy:heartbeat";

impl Owner {
    /// The record of a claim created in this owner's name.
    pub fn write(&self) -> String {
        self.record("", "")
    }

    /// The record of the claim `taken` once it is this owner's: it is still
    /// the claim it was created as.
    pub fn write_over(&self, taken: &Claim) -> String {
        let since = taken.since.map(|since| since.to_string()).unwrap_or_default();
        self.record(&taken.id, &since)
    }

    fn record(&self, first: &str, since: &str) -> String {
        let ts = self.ts.to_string();
        let (instance, host) = (self.instance.as_str(), self.host.as_str());
        record::write(
            CLAIM,
            &[
                ("instance", instance),
                ("host", host),
                ("ts", &ts),
                ("first", first),
                ("since", since),
            ],
        )
    }

    /// A commit subject → the owner it names; `None` when it names nobody.
    pub fn read(subject: &str) -> Option<Owner> {
        let fields: Fields = record::read(CLAIM, subject)?;
        Some(Owner {
            instance: fields.get("instance")?.to_string(),
            host: fields.get("host").unwrap_or("").to_string(),
            ts: fields.number("ts").unwrap_or(0),
        })
    }
}

/// One heartbeat ref as the remote has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beat {
    pub ts: u64,
    /// The host its instance runs on; empty where its record names none.
    pub host: String,
    /// The commit its ref points at now: what a delete of it is leased to.
    pub sha: String,
}

/// The record of the heartbeat `instance`, running on `host`, stamps at `ts`.
pub fn write_heartbeat(instance: &InstanceId, host: &str, ts: u64) -> String {
    let ts = ts.to_string();
    record::write(HEARTBEAT, &[("instance", instance.as_str()), ("host", host), ("ts", &ts)])
}

/// The heartbeat whose ref points at `sha`, a commit with the subject
/// `subject`; `None` when it carries no time.
pub fn read_heartbeat(subject: &str, sha: String) -> Option<Beat> {
    let fields: Fields = record::read(HEARTBEAT, subject)?;
    let host = fields.get("host").unwrap_or("").to_string();
    Some(Beat { ts: fields.number("ts")?, host, sha })
}

/// One claim ref as the remote has it. The ref is the lock, so a ref whose
/// record cannot be read is still a claim: one that names nobody, which is
/// never mine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub number: u64,
    pub owner: Option<Owner>,
    /// The commit its ref points at now: what a takeover or a delete of it is
    /// leased to. Every takeover moves it.
    pub sha: String,
    /// The commit it was created at, which every takeover carries on: what is
    /// declared or kept under a claim names this, so it is found whoever
    /// holds the claim now.
    pub id: String,
    /// When it was created, where its record says.
    pub since: Option<u64>,
}

/// The claim on a run: work a fleet instance does for no issue — a patrol's
/// run or a cut — as the remote has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunClaim {
    /// What follows `CLAIM_KIND` in its ref's name: whoever claims runs of
    /// one kind says how theirs are keyed.
    pub key: String,
    /// `None`: the claim's record names nobody.
    pub owner: Option<Owner>,
    /// What a delete of the claim is leased to.
    pub sha: String,
    /// Its worker declared the run over (`over_ref`).
    pub over: bool,
}

/// Whose a run still claimed is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    Mine,
    /// A peer whose heartbeat is within the lease.
    Peer,
    /// A peer presumed dead, or nobody: the claim is to be deleted.
    Dead,
}

impl RunClaim {
    /// Whose the run is for the fleet instance `me` (`None`: nobody's claims
    /// are its own) at `now`, with each instance's heartbeat in `heartbeats`.
    pub fn holder(
        &self,
        me: Option<&InstanceId>,
        heartbeats: &[(String, u64)],
        now: u64,
    ) -> Holder {
        let owner = self.owner.as_ref().map(|owner| owner.instance.as_str());
        let beat = owner.and_then(|instance| {
            heartbeats.iter().find(|(name, _)| name == instance).map(|(_, ts)| *ts)
        });
        if me.is_some_and(|me| owner == Some(me.as_str())) {
            Holder::Mine
        } else if is_stale(beat, now) {
            Holder::Dead
        } else {
            Holder::Peer
        }
    }
}

impl Claim {
    /// The claim on issue `number` whose ref points at `sha`, a commit with
    /// the subject `subject`.
    pub fn read(number: u64, subject: &str, sha: String) -> Claim {
        let owner = Owner::read(subject);
        let fields = record::read(CLAIM, subject).unwrap_or_default();
        let id = fields.get("first").map_or_else(|| sha.clone(), str::to_string);
        let since = fields.number("since").or(owner.as_ref().map(|owner| owner.ts));
        Claim { number, owner, sha, id, since }
    }

    pub fn is_held_by(&self, instance: &InstanceId) -> bool {
        self.owner.as_ref().is_some_and(|owner| owner.instance == instance.as_str())
    }
}

/// One push of putting a claim in an instance's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStep {
    /// Refresh the instance's heartbeat.
    Beat,
    /// Push the claim ref itself.
    Push,
}

/// The pushes of one claim, in order, as they are decided. `beaten`: the
/// owner's heartbeat on the remote is fresh.
#[refined_by(beaten: bool)]
struct ClaimOrder {
    #[field(bool[beaten])]
    beaten: bool,
    steps: Vec<ClaimStep>,
}

impl ClaimOrder {
    #[spec(fn(due: bool) -> ClaimOrder[!due])]
    fn new(due: bool) -> ClaimOrder {
        ClaimOrder { beaten: !due, steps: Vec::new() }
    }

    #[spec(fn(self: &mut Self) ensures self: Self[true])]
    fn beat(&mut self) {
        self.steps.push(ClaimStep::Beat);
        self.beaten = true;
    }

    /// A claim is pushed only behind a fresh heartbeat of its owner.
    #[spec(fn(self: &mut Self[@o]) requires o.beaten)]
    fn push(&mut self) {
        self.steps.push(ClaimStep::Push);
    }
}

/// The pushes that put a claim in an instance's name — a fresh claim, or one
/// taken from somebody — given the instance's heartbeat on the remote. The
/// heartbeat goes first when it is due: a peer scanning the instant the claim
/// lands would otherwise read it as stale and take it from a live fleet.
pub fn claim_steps(last_beat: Option<u64>, now: u64) -> Vec<ClaimStep> {
    let mut order = ClaimOrder::new(heartbeat_due(last_beat, now));
    if !order.beaten {
        order.beat();
    }
    order.push();
    order.steps
}

/// Why a claim is not the caller's to settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotMine {
    pub number: u64,
    /// The instance that holds it; empty when its record names nobody.
    pub holder: String,
}

impl std::fmt::Display for NotMine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "issue #{} is not this fleet's claim (it is held by {:?}); nothing was changed",
            self.number, self.holder
        )
    }
}

impl std::error::Error for NotMine {}

/// The claim `me` may release on an issue: its own, or `None` when the issue
/// has no claim — already gone counts as released. Another's is refused.
pub fn releasable<'a>(
    claim: Option<&'a Claim>,
    me: &InstanceId,
) -> Result<Option<&'a Claim>, NotMine> {
    match claim {
        Some(claim) if !claim.is_held_by(me) => Err(NotMine {
            number: claim.number,
            holder: claim.owner.as_ref().map(|o| o.instance.clone()).unwrap_or_default(),
        }),
        other => Ok(other),
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A claim pushed without a look at its owner's heartbeat.
    #[should_fail]
    fn a_claim_pushed_ahead_of_its_heartbeat(due: bool) {
        let mut order = ClaimOrder::new(due);
        order.push();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::BEAT_EVERY;

    fn id(text: &str) -> InstanceId {
        InstanceId::parse(text).unwrap()
    }

    #[test]
    fn an_instance_id_has_one_grammar() {
        for good in ["a", "fleet-1", "0abc", &"a".repeat(40)] {
            assert!(InstanceId::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "-a", "A", "a_b", "a/b", "a b", &"a".repeat(41), "é"] {
            assert!(InstanceId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_minted_id_is_in_the_grammar() {
        for host in ["Felix's MacBook-Pro.local", "", "---", "主机", &"x".repeat(80)] {
            let minted = InstanceId::mint(host, 0xdead_beef);
            assert_eq!(InstanceId::parse(minted.as_str()), Ok(minted.clone()), "{host:?}");
        }
        assert_eq!(InstanceId::mint("My Mac.local", 255).as_str(), "my-mac-local-000000ff");
    }

    #[test]
    fn a_claim_record_reads_back() {
        let owner = Owner { instance: "fleet-1".into(), host: "my mac.local".into(), ts: 99 };
        assert_eq!(Owner::read(&owner.write()), Some(owner.clone()));
        assert_eq!(Owner::read("convoy:claim host=x"), None, "names nobody");
        assert_eq!(Owner::read("initial commit"), None);
        let claim = Claim::read(7, &owner.write(), "abc".into());
        assert_eq!((claim.id.as_str(), claim.since), ("abc", Some(99)));
        assert_eq!(claim.owner, Some(owner));
        // Taken over twice, it is still the claim it was created as.
        let mut taken = claim;
        for (sha, ts) in [("def", 500), ("ghi", 900)] {
            let peer = Owner { instance: "fleet-2".into(), host: "h".into(), ts };
            taken = Claim::read(7, &peer.write_over(&taken), sha.into());
            assert_eq!(
                (taken.sha.as_str(), taken.id.as_str(), taken.since),
                (sha, "abc", Some(99))
            );
            assert_eq!(taken.owner, Some(peer));
        }
        let nobodys = Claim::read(8, "initial commit", "def".into());
        assert_eq!((nobodys.owner, nobodys.id.as_str(), nobodys.since), (None, "def", None));
        let beat = read_heartbeat(&write_heartbeat(&id("a"), "my mac.local", 5), "abc".into());
        assert_eq!(beat, Some(Beat { ts: 5, host: "my mac.local".into(), sha: "abc".into() }));
        assert_eq!(read_heartbeat("convoy:heartbeat instance=a", "abc".into()), None);
        // One stamped before a heartbeat named its host is nobody's host's.
        let hostless = read_heartbeat("convoy:heartbeat instance=a ts=5", "abc".into());
        assert_eq!(hostless.map(|beat| beat.host), Some(String::new()));
    }

    #[test]
    fn the_heartbeat_goes_first_when_it_is_due() {
        use ClaimStep::*;
        assert_eq!(claim_steps(None, 1000), [Beat, Push]);
        assert_eq!(claim_steps(Some(1000), 1000 + BEAT_EVERY), [Push]);
        assert_eq!(claim_steps(Some(1000), 1001 + BEAT_EVERY), [Beat, Push]);
    }

    #[test]
    fn only_my_own_claim_is_releasable() {
        let owner = Owner { instance: "me".into(), host: "h".into(), ts: 1 };
        let mine = Claim::read(7, &owner.write(), "abc".into());
        let nobodys = Claim::read(8, "", "def".into());
        assert_eq!(releasable(Some(&mine), &id("me")), Ok(Some(&mine)));
        assert_eq!(releasable(None, &id("me")), Ok(None));
        assert_eq!(releasable(Some(&mine), &id("peer")).unwrap_err().holder, "me");
        assert_eq!(releasable(Some(&nobodys), &id("me")).unwrap_err().holder, "");
    }
}
