//! `afkconvoy show <issue>` in an unbound repo: where the issue stands on the
//! one path every issue there takes — ready, held back, claimed, landing,
//! waiting on a human, or closed — read off the facts a pass acts on.

use afkconvoy_core::plan::{Stands, closing_pr};
use afkconvoy_core::worker::recorded_branches;
use anyhow::Result;
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::git::Repo;

/// Where issue `number` of the unbound repo of `repo` stands at `now`:
///
///   {"issue", "workflow": null, "stands": {"is", …}, "claim", "pr"}
///
/// `stands.is` is `ready`, `held` with its `reason`, `claimed`, `landing`,
/// `waiting_on_human` with its `why`, `url` and `answer`, or `closed` with
/// GitHub's `state_reason` and `closed_by`. `claim` is {"instance", "live"}
/// while one is held; `pr` the open PR that closes the issue,
/// {"number", "branch", "fleet"}, `fleet` whether its branch is one the
/// fleet recorded on the issue: only such a PR waits to land.
pub fn show(repo: &Repo, now: u64, number: u64) -> Result<Value> {
    let (fleet, open) = Fleet::read(repo, now)?;
    let (set, ..) = fleet.rebuild(&open, None)?;
    let bodies = open.issues.comments(number).iter().map(|comment| comment.body.as_str());
    let recorded = recorded_branches(bodies);
    let pr = closing_pr(&open.prs, number);
    let fleets = pr.is_some_and(|pr| recorded.contains(&pr.head_branch));
    let pr = pr.map(|pr| json!({"number": pr.number, "branch": pr.head_branch, "fleet": fleets}));
    let mut claim = Value::Null;
    let stands = match set.stands(number) {
        Some(Stands::Ready) => json!({"is": "ready"}),
        Some(Stands::Held(reason)) => json!({"is": "held", "reason": reason}),
        Some(Stands::Waiting(waiting)) => json!({"is": "waiting_on_human", "why": waiting.why,
            "url": waiting.url, "answer": waiting.answer}),
        Some(Stands::Claimed { claim: held, live }) => {
            claim = json!({"instance": held.instance, "live": live});
            json!({"is": if fleets { "landing" } else { "claimed" }})
        }
        None => {
            let read = fleet.gh.issue(number)?;
            match read.is_closed() {
                true => json!({"is": "closed", "state_reason": read.state_reason,
                    "closed_by": read.closed_by}),
                false => json!({"is": "held",
                    "reason": "not among the open issues this fleet's tracker lists for its repo"}),
            }
        }
    };
    Ok(json!({"issue": number, "workflow": null, "stands": stands, "claim": claim, "pr": pr}))
}
