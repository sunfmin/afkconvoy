//! `afkconvoy flows`: what whoever hands the fleet work needs to open an issue
//! the fleet will work — each workflow of the team and what it is for, the
//! labels to keep off an issue, and the line that opens one in the fleet's
//! tracker — with the labels that line names created there where they are
//! missing. In an unbound repo the one label is the ready label.
//!
//! The tracker is the one the checkout's host says on its checkout list, or
//! the one its caller says with `--tracker` where the list holds no entry of
//! it: never guessed, as an issue opened in a tracker no fleet reads is
//! never worked.

use afkconvoy_core::config::{Config, RepoName};
use afkconvoy_core::line::Line;
use afkconvoy_core::team::{DONE, HUMAN, LAND, Pass, Performer, Team, Workflow};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::checkout_list::{List, named_repo, said};
use crate::config;
use crate::gh::{GitHub, Tracker};
use crate::git::Repo;
use crate::intake::open_taken_in;
use crate::team;
use crate::tracker::REPO_LABEL_SAYS;

/// What GitHub shows beside a flow label this command created, and beside a
/// ready label.
const FLOW_LABEL_SAYS: &str = "afkconvoy: the workflow an issue that carries this follows";
pub(crate) const READY_LABEL_SAYS: &str = "afkconvoy: the fleet works an issue that carries this";

/// The line that opens an issue of `tracker` carrying `label` — in a tracker
/// that is another repo, with the repo label too — its title and body left to
/// whoever runs it.
fn line(tracker: &Tracker, label: Option<&str>) -> String {
    let mut line = Line::new().fixed("gh issue create");
    if let Some(named) = tracker.named() {
        line = line.fixed(" --repo ").program(named.as_str());
    }
    if let Some(label) = label {
        line = line.fixed(" --label ").word(label);
    }
    if let Some(repo_label) = tracker.repo_label() {
        line = line.fixed(" --label ").word(&repo_label);
    }
    line.fixed(" --title '<title>' --body-file <file>").own()
}

/// Creates each of `labels` the repo lacks, as one that says `says` → the
/// ones created. GitHub tells labels apart by their letters, not their case.
pub(crate) fn create_missing(gh: &GitHub, labels: &[String], says: &str) -> Result<Vec<String>> {
    let has = gh.labels()?;
    let mut created = vec![];
    for label in labels {
        if !has.iter().any(|had| had.eq_ignore_ascii_case(label)) {
            gh.create_label(label, says)?;
            created.push(label.clone());
        }
    }
    Ok(created)
}

/// One workflow, as whoever opens an issue of `tracker` into it reads it.
fn workflow(tracker: &Tracker, team: &Team, workflow: &Workflow, delivery: Option<&str>) -> Value {
    let first = workflow.first();
    let (by, skill) = match &first.by {
        Performer::Human => (HUMAN, None),
        Performer::Role(work) => (team.role_of(work).name.as_str(), Some(&work.skill)),
    };
    // Every stage is one an issue can come to, so every end a pass leads to
    // is one the workflow has.
    let leads_to =
        |end: fn(&Pass) -> bool| workflow.stages().iter().any(|stage| end(&stage.pass()));
    let mut ends = vec![];
    if leads_to(|pass| matches!(pass, Pass::Land)) {
        ends.push(LAND);
    }
    if leads_to(|pass| matches!(pass, Pass::Done)) {
        ends.push(DONE);
    }
    let flow_label = workflow.flow_label();
    let mut answer = json!({
        "name": workflow.name,
        "flow_label": flow_label,
        "description": workflow.description,
        "first": {"stage": first.name, "by": by, "do": skill},
        "ends": ends,
        "line": line(tracker, delivery.is_none().then_some(flow_label)),
    });
    if let Some(id) = delivery {
        answer["intake_line"] = json!(intake_line(id, Some(&workflow.name)));
    }
    answer
}

fn intake_line(id: &str, workflow: Option<&str>) -> String {
    let mut line = Line::new().fixed("afkconvoy intake --issue <n> --delivery ").word(id);
    if let Some(workflow) = workflow {
        line = line.fixed(" --workflow ").word(workflow);
    }
    line.own()
}

/// The labels that keep an issue from being worked, with `escalate_label` the
/// one the fleet swaps in: the team's in a bound repo, the config's in an
/// unbound one.
fn keep_off(config: &Config, escalate_label: &str) -> Value {
    json!({"epic_labels": config.epic_labels, "escalate_label": escalate_label,
        "lock_label": config.lock_label})
}

/// The tracker `flows` answers for in the checkout `repo`, with `asked` what
/// `--tracker` says of it → the tracker, and whether `asked` said it where
/// the checkout list holds no entry of the checkout. The list's entry holds,
/// and an `asked` it does not say is refused, naming both; with no entry,
/// `asked` is the tracker, and with neither the tracker is not said.
fn tracker(repo: &Repo, asked: Option<&RepoName>) -> Result<(Tracker, bool)> {
    // The tracker a checkout names, none where that is its own repo.
    let own = |named: &RepoName| -> Result<Option<RepoName>> {
        let is_own = named.as_str().eq_ignore_ascii_case(named_repo(repo)?.as_str());
        Ok((!is_own).then(|| named.clone()))
    };
    let tracker = |named: Option<RepoName>| -> Result<Tracker> {
        Ok(match named {
            None => Tracker::Own,
            Some(tracker) => Tracker::Other { tracker, repo: named_repo(repo)? },
        })
    };
    let lower = |named: &Option<RepoName>| named.as_ref().map(|n| n.as_str().to_lowercase());
    match (said(&List::of_host()?, repo)?, asked) {
        (Some(listed), None) => Ok((tracker(listed)?, false)),
        (Some(listed), Some(asked)) => {
            if lower(&listed) != lower(&own(asked)?) {
                let listed = listed.map_or_else(|| named_repo(repo), Ok)?;
                bail!(
                    "{} is on this host's checkout list with the tracker {listed}, and \
                     `--tracker` says {asked}: its fleet reads its issues in {listed}. Leave \
                     `--tracker` off, or change what the list says with `afkconvoy run \
                     --tracker {asked}`",
                    repo.dir.display()
                );
            }
            Ok((tracker(listed)?, false))
        }
        (None, Some(asked)) => Ok((tracker(own(asked)?)?, true)),
        (None, None) => bail!(
            "the tracker is not said: {} is on no checkout list of this host, so which repo its \
             fleet reads its issues in is not known here. Say it: `afkconvoy flows --tracker \
             <owner>/<repo>` — the checkout's own repo where that is its tracker — or run it on \
             the fleet's host, where `afkconvoy run --tracker <owner>/<repo>` puts it on the list",
            repo.dir.display()
        ),
    }
}

/// `afkconvoy flows`, in the checkout `repo`, for the tracker `tracker`
/// finds with `asked`. Everything is read before a label is created, so a
/// team with a fault, or a tracker not said, writes nothing.
///
///   {"team", "workflows": [{"name", "flow_label", "description", "first", "ends", "line"}],
///    "keep_off", "intake": {"open", "limit"}, "created"}
///   {"team": null, "ready_label", "line", "keep_off", "created"}
pub fn flows(repo: &Repo, delivery: Option<&str>, asked: Option<&RepoName>) -> Result<Value> {
    let config = config::load(&repo.dir.join(config::FILE))?;
    let (tracker, off_the_list) = tracker(repo, asked)?;
    let gh = GitHub::new(repo.dir.clone(), tracker);
    if let Some(id) = delivery {
        crate::delivery::reference(repo, id)?;
    }
    // A tracker the list says was given the repo label by the `run` that
    // said it; one `--tracker` says off the list may lack it.
    let repo_label = gh.tracker.repo_label().filter(|_| off_the_list);
    let labelled = |gh: &GitHub, mut created: Vec<String>| -> Result<Vec<String>> {
        if let Some(label) = &repo_label {
            created.extend(create_missing(gh, std::slice::from_ref(label), REPO_LABEL_SAYS)?);
        }
        Ok(created)
    };
    let Some(bound) = &config.bound else {
        let created =
            create_missing(&gh, std::slice::from_ref(&config.ready_label), READY_LABEL_SAYS)?;
        let created = labelled(&gh, created)?;
        let mut answer = json!({"team": null, "ready_label": config.ready_label,
            "line": line(&gh.tracker, delivery.is_none().then_some(config.ready_label.as_str())),
            "keep_off": keep_off(&config, &config.escalate_label), "created": created});
        if let Some(id) = delivery {
            answer["delivery"] = json!(id);
            answer["intake_line"] = json!(intake_line(id, None));
        }
        return Ok(answer);
    };
    let team = team::load(team::From::Config, Some(repo))?.team;
    let open = open_taken_in(&team, &gh.open_issues()?);
    let flow_labels: Vec<String> =
        team.workflows.iter().map(|workflow| workflow.flow_label().to_string()).collect();
    let created = labelled(&gh, create_missing(&gh, &flow_labels, FLOW_LABEL_SAYS)?)?;
    let mut answer = json!({
        "team": bound.team,
        "workflows":
            team.workflows.iter().map(|w| workflow(&gh.tracker, &team, w, delivery)).collect::<Vec<_>>(),
        "keep_off": keep_off(&config, &team.escalate_label),
        "intake": {"open": open, "limit": bound.intake_limit},
        "created": created,
    });
    if let Some(id) = delivery {
        answer["delivery"] = json!(id);
    }
    Ok(answer)
}

/// What `afkconvoy flows --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Before opening an issue the fleet is to work, and again whenever the team gains a workflow: it says which workflow takes which kind of issue, what label puts an issue in it, and which labels to keep off — and it creates the labels that takes, so the line it gives works. Run it once for all the issues you open, not once per issue.

```
afkconvoy flows
```

## Who runs it

Whoever hands the fleet work: a human, or the agent a human is talking to, with work decided on and issues to open for it. In a checkout of the repo the issues go to, bound or unbound. A worker opening a blocker for another repo of the host names that repo's checkout: `afkconvoy flows <target-checkout>`. It takes the issue in there with `afkconvoy intake`.

## What it reads and writes

It reads the config of the checkout and the labels its tracker has. The tracker is the one this host's checkout list says for the checkout, or for the checkout it is a worktree of — the checkout's own repo, until its host says another (`afkconvoy run --tracker`). Where the list holds no entry of it — another account on a shared machine, a fresh clone — the tracker is not said, and it is refused: name it with `--tracker <owner>/<repo>`, the checkout's own repo where that is its tracker. On an entry, a `--tracker` the entry does not say is refused, naming both. In a bound repo it also reads the team as `afkconvoy check` does — the team's files and every role's repo, from GitHub at the head of each default branch — and, to count, the comments of every open issue that carries a flow label.

It writes one thing: each label its lines name that the tracker lacks. In a bound repo that is the flow label of every workflow of the team — `flow:<workflow>`, or the one its file's `label` names; in an unbound one, the ready label; and, where `--tracker` names another repo off the list, the repo label. It puts no label on any issue and opens none. Everything is read first: a team with a fault creates nothing.

With `--delivery <identity>`, it first validates an existing same-repo Jira delivery. Its `line` keeps the tracker and repo label but omits the runnable flow or ready label. Run that line, then its `intake_line` with the returned issue number: intake attaches the delivery before adding the selected workflow's label (or the unbound repo's ready label). Both steps run in this code repo's checkout. The answer includes `delivery`; each workflow, or the unbound answer, includes `intake_line`. An invalid or cross-repo delivery refuses before creating labels. Without --delivery, today's ordinary creation line is unchanged.

## What it answers

In a bound repo:

{"team", "workflows", "keep_off", "intake", "created"}

With --delivery: also `delivery`, and each workflow's `intake_line` to run after its non-runnable creation line. In an unbound repo, `delivery` and `intake_line` are top-level keys.

- `team`: the team's repo
- `workflows`: every workflow of the team, each {"name", "flow_label", "description", "first", "ends", "line"}:

  - `description`: what the workflow is for and what an issue of it holds: the prose of its file, workflows/<name>.md, above the yaml block. Empty when the file says nothing there — `afkconvoy check` warns of that. Choose the workflow by it
  - `first`: the stage an issue enters at, {"stage", "by", "do"}: the role that performs it and the skill — `by` is `human`, and `do` null, where a human's decision is waited for
  - `ends`: where its passes end: `land`, a landing turn — its issues end in a PR landed on their target branch — `done`, nothing more to do, or both, by different stages
  - `line`: the line that opens an issue into it, in the tracker. Give it a title and a file holding the body
- `keep_off`: the labels that keep an issue from being worked as one of a workflow, {"epic_labels", "escalate_label", "lock_label"}: an issue carrying an epic label is never dispatched; the escalate label is the fleet's to put on, when it hands an issue to a human; and an open issue carrying the lock label is a fleet lock, which stops all other work (`afkconvoy lock --help`)
- `intake`: {"open", "limit"}: the open issues workers took in, against the repo's `intake_limit`. An issue a human opens with a flow label is outside both: nothing refuses it
- `created`: the flow labels this run created on the repo; empty when it had them all

In an unbound repo, which has no workflow:

{"team": null, "ready_label", "line", "keep_off", "created"}

- `ready_label`: the label an issue is worked under
- `line`: the line that opens an issue carrying it
- `keep_off`: as above; the escalate label is the config's
- `created`: the ready label, when this run created it; else empty

Errors, as {"error": …} with a non-zero exit: "the tracker is not said" — the checkout is on no checkout list of this host, and no `--tracker` names it; a `--tracker` its entry does not say; the team has a fault — {"error", "faults"}, as `check` answers it; the team's repo cannot be read; the config cannot be read; "is not in a git checkout", or git's own words for a checkout it refuses; GitHub refused to list or create a label.

## What trips you up

- `gh issue create --label` refuses a label the repo lacks, and nothing but this command creates a flow label ahead of the first issue. Run it before the line.
- Opening an issue with `gh`, or labelling one, wakes no fleet: the fleet takes it up at its next tick — within {const:TICK_BUSY_SECONDS} seconds while it has work, within {const:TICK_IDLE_MINUTES} minutes once it has been idle {const:TICK_IDLE_AFTER} ticks in a row. `afkconvoy wake <checkout>`, on the fleet's host, starts that tick now: run it once, after the last issue you open.
- An issue follows one workflow: put one flow label on it. With several it follows none, and no stage of any is performed on it.
- The flow label is what hands the issue over. Leave it off an issue that is not ready to be worked, and put it on later: `gh issue edit <n> --add-label <label>`.
- Where the tracker is another repo, the line opens the issue there, `--repo <owner>/<tracker>`, with this repo's repo label, `repo:<owner>/<repo>`: an issue there without it is no issue of this repo, and its fleet never works it. Name the tracker in every `gh issue` command (`--repo`): run in the checkout, `gh` acts on the checkout's own repo.
- A workflow whose `first` is `by: human` starts by waiting for a human: the fleet does nothing on its issue until one declares a verdict (`afkconvoy verdict --help`).
- It answers the team as pushed to its default branch. A workflow added in a checkout of the team is not answered, and its label not created, until it is pushed.
- A label of a workflow the team no longer has is left on the repo: this command creates labels and removes none."#;
