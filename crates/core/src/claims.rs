//! Claims: the lock that makes one issue one fleet instance's, and the
//! heartbeat that says its owner is alive.

use flux_rs::attrs::*;

use crate::lease::heartbeat_due;
use crate::record::{self, Fields};

/// Under every ref the fleet keeps on the remote.
pub const NAMESPACE: &str = "refs/convoy";

/// The ref that is the claim on issue `number`.
pub fn claim_ref(number: u64) -> String {
    format!("{NAMESPACE}/claim/{number}")
}

/// The ref that carries `instance`'s heartbeat.
pub fn heartbeat_ref(instance: &InstanceId) -> String {
    format!("{NAMESPACE}/heartbeat/{instance}")
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
    pub fn write(&self) -> String {
        let ts = self.ts.to_string();
        record::write(CLAIM, &[("instance", &self.instance), ("host", &self.host), ("ts", &ts)])
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

pub fn write_heartbeat(instance: &InstanceId, ts: u64) -> String {
    record::write(HEARTBEAT, &[("instance", instance.as_str()), ("ts", &ts.to_string())])
}

/// A commit subject → the time of the heartbeat it carries.
pub fn read_heartbeat(subject: &str) -> Option<u64> {
    record::read(HEARTBEAT, subject)?.number("ts")
}

/// One claim ref as the remote has it. The ref is the lock, so a ref whose
/// record cannot be read is still a claim: one that names nobody, which is
/// never mine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub number: u64,
    pub owner: Option<Owner>,
    pub sha: String,
}

impl Claim {
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
    #[spec(fn(self: &mut Self[@o]) requires o.beaten ensures self: Self[o.beaten])]
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
            "issue #{} is not this fleet's claim ({} is held by {:?}); nothing was changed",
            self.number,
            claim_ref(self.number),
            self.holder
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
        assert_eq!(Owner::read(&owner.write()), Some(owner));
        assert_eq!(Owner::read("convoy:claim host=x"), None, "names nobody");
        assert_eq!(Owner::read("initial commit"), None);
        assert_eq!(read_heartbeat(&write_heartbeat(&id("a"), 5)), Some(5));
        assert_eq!(read_heartbeat("convoy:heartbeat instance=a"), None);
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
        let mine = Claim {
            number: 7,
            owner: Some(Owner { instance: "me".into(), host: "h".into(), ts: 1 }),
            sha: "abc".into(),
        };
        let nobodys = Claim { number: 8, owner: None, sha: "def".into() };
        assert_eq!(releasable(Some(&mine), &id("me")), Ok(Some(&mine)));
        assert_eq!(releasable(None, &id("me")), Ok(None));
        assert_eq!(releasable(Some(&mine), &id("peer")).unwrap_err().holder, "me");
        assert_eq!(releasable(Some(&nobodys), &id("me")).unwrap_err().holder, "");
    }
}
