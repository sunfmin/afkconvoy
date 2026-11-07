//! A checkout's harness: the one its workers start on, on this host. It is
//! the checkout's, said on its entry on the checkout list, set by
//! `run --harness`, and read as each tick opens the fleet. Until the checkout
//! names one, the host kit's first eligible harness holds. `--harness default`
//! clears the name. The host kit stays the home of worker launch commands,
//! models and research. A repo config names no harness.

use std::path::Path;

use afkconvoy_core::harness::Harness;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::checkout_list::{self, List, asked};
use crate::git::Repo;
use crate::host_settings::To;

/// `--harness` as said: a harness's key, or `default`.
pub fn harness_or_default(said: &str) -> Result<To<Harness>, String> {
    To::parse(said, |said| Harness::read(said).ok_or_else(|| Harness::unknown(said)))
}

/// The harness the checkout `repo` names on this host, where it names one.
pub fn named(repo: &Repo) -> Result<Option<Harness>> {
    named_at(&List::of_host()?, &repo.dir)
}

/// The harness the checkout at `root` names on the host of `list`: the one
/// with its entry, or, where the list holds no entry of it yet, what its
/// `run` asked in this process.
pub fn named_at(list: &List, root: &Path) -> Result<Option<Harness>> {
    let asked = asked();
    let listed = list.read()?;
    Ok(match listed.checkouts.iter().find(|entry| entry.checkout == root) {
        Some(entry) => entry.harness,
        None => asked.get(root).and_then(|asked| asked.harness),
    })
}

/// Has the checkout `repo` name `to` on the host of `list`. A harness the
/// host kit does not hold is an error and writes nothing. `Default` clears
/// the name. The key the last converge landed is left as it is: the next
/// tick lands workers on the harness now named.
pub fn say(list: &List, repo: &Repo, to: To<Harness>) -> Result<Value> {
    if let To::Value(harness) = to {
        let kit = crate::kit::load()?;
        if !kit.harnesses.iter().any(|held| held.harness == harness) {
            bail!(
                "`{}` is not a harness this host kit holds: `afkconvoy kit` lists the ones it \
                 does, and nothing was written",
                harness.key()
            );
        }
    }
    let was = json_of(repo);
    list.change(|listed| match listed.checkouts.iter_mut().find(|e| e.checkout == repo.dir) {
        Some(entry) => entry.harness = to.kept(),
        None => ask(&repo.dir, to),
    })?;
    let now = json_of(repo);
    Ok(if was == now { Value::Null } else { json!({"from": was, "to": now}) })
}

/// Keeps `to` as what the `run` of the checkout at `root` asked, in this
/// process alone. What a checkout on the list says with its entry holds
/// over it.
pub fn ask(root: &Path, to: To<Harness>) {
    asked().entry(root.to_path_buf()).or_default().harness = to.kept();
}

/// The harness in force for `repo`, as {in_force, said_by}. `said_by` is
/// `checkout` where the checkout names one — `in_force` is that harness even
/// when it cannot start — and `host_kit` where it names none, `in_force`
/// then the host kit's first that pause and health leave, or null where none
/// remains. A launch command is not asked again here: the selection already
/// asks the login shell, and asking again would read it a second time on a
/// plan that trusts its probe. {error} where the checkout list or, for a
/// checkout that names none, the host kit cannot be read.
pub fn json_of(repo: &Repo) -> Value {
    match standing(repo) {
        Ok(value) => value,
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

fn standing(repo: &Repo) -> Result<Value> {
    match named(repo)? {
        Some(harness) => Ok(json!({"in_force": harness.key(), "said_by": "checkout"})),
        None => {
            let in_force = first_eligible()?.map(|harness| harness.key());
            Ok(json!({"in_force": in_force, "said_by": "host_kit"}))
        }
    }
}

/// The host kit's first harness pause and health leave. Whether its launch
/// command can start is the selection's question (`Fleet::runnable`), asked
/// once there.
fn first_eligible() -> Result<Option<Harness>> {
    let (kit, _) = crate::kit::running()?;
    Ok(kit.first().map(|held| held.harness))
}

/// Why `harness` cannot start a worker of `repo` at `now`. A model fault that
/// still leaves a placed model does not count, and neither does a harness
/// with no placed model and no fault: that harness starts on its default model.
/// While a startup probe that checked the current launch commands stands,
/// this does not ask the harness: the note says what that probe found.
pub fn cannot_start(repo: &Repo, harness: Harness, now: u64) -> Option<String> {
    let Ok(kit) = crate::kit::load() else {
        return Some("the host kit cannot be read".to_string());
    };
    if !kit.harnesses.iter().any(|held| held.harness == harness) {
        return Some(format!("`{}` is not a harness this host kit holds", harness.key()));
    }
    if let Some(pause) = crate::paused::read().into_iter().find(|pause| pause.harness == harness) {
        return Some(format!("`{}` is paused on this host: {}", harness.key(), pause.why));
    }
    let Ok((running, _)) = crate::kit::running() else {
        return Some("the host kit cannot be read".to_string());
    };
    if !running.harnesses.iter().any(|held| held.harness == harness) {
        return Some(
            crate::health::fault_reason(harness).unwrap_or_else(|| {
                format!("`{}` cannot start a worker on this host", harness.key())
            }),
        );
    }
    if crate::probe::stands(repo, now) {
        return noted_start_fault(repo, harness, now);
    }
    let launches = crate::launch::Launches::foreseen(repo);
    match crate::role::startable(&launches, harness, "the harness this checkout names") {
        Ok(()) => None,
        Err(error) => Some(format!("{error:#}")),
    }
}

/// What the standing probe found in the way of starting `harness`. `None`
/// when it found the harness able to start, or the note is from before
/// harness results were recorded: asking again would start the harness.
fn noted_start_fault(repo: &Repo, harness: Harness, now: u64) -> Option<String> {
    let notes = crate::probe::noted_harnesses(repo, now);
    let note = notes.iter().find(|note| note.harness() == harness.key())?;
    if let Some(fault) = note.fault().or(note.flags_fault()) {
        return Some(fault.to_string());
    }
    match note.state() {
        "missing" => Some(format!("`{}` is missing on this host", harness.key())),
        "logged_out" => Some(format!("`{}` is logged out on this host", harness.key())),
        _ => None,
    }
}

/// What `doctor` says of a checkout that names a harness no worker of it
/// starts on: the harness cannot start, or a role holds no limits for it.
pub fn findings(checkout: &Path, now: u64) -> Vec<crate::doctor::Finding> {
    let Ok(repo) = Repo::at(checkout) else { return vec![] };
    let Ok(Some(harness)) = named(&repo) else { return vec![] };
    let mut found = vec![];
    if let Some(why) = cannot_start(&repo, harness, now) {
        found.push(one(&repo, harness, &why, "afkconvoy kit"));
    }
    let Ok(config) = crate::config::load(&crate::config::file_of(&repo)) else {
        return found;
    };
    let Some(bound) = config.bound.as_ref() else { return found };
    let Some(team) = crate::role::current(&repo, bound, &config.labels()) else {
        return found;
    };
    for role in &team.roles {
        let Some(why) = afkconvoy_core::team::unlimited(role, harness) else { continue };
        let reason =
            format!("the role `{}` holds no limits for `{}`: {why}", role.name, harness.key());
        let fix = format!(
            "write the role's limits for `{}` in its {}, or `afkconvoy run {} --harness default`",
            harness.key(),
            harness.limits(),
            repo.dir.display()
        );
        found.push(one(&repo, harness, &reason, &fix));
    }
    found
}

fn one(repo: &Repo, harness: Harness, reason: &str, fix: &str) -> crate::doctor::Finding {
    crate::doctor::finding(
        crate::doctor::Kind::NamedHarness,
        serde_json::json!(["named-harness", harness]),
        format!(
            "names a harness no worker of this checkout starts on: checkout {}, harness {}, {reason}",
            repo.dir.display(),
            harness.key()
        ),
        reason.to_string(),
        fix.to_string(),
    )
}

/// Whether this tick continues workers onto the harness now in force.
/// Absent or already landed on the host kit does nothing. A named harness
/// is retried every tick, so a worker still on another harness is continued
/// again. Clearing lands once, then stops.
fn wants_converge(applied: Option<&str>, desired: &str) -> bool {
    if !desired.is_empty() {
        return true;
    }
    match applied {
        None | Some("") => false,
        Some(_) => true,
    }
}

/// Continues this checkout's workers onto the harness it names, or, where it
/// just cleared that name, onto the host kit's first eligible harness. A
/// harness that cannot start is left for the next tick, and so is a
/// continuation that did not land. The tick is not failed.
pub fn converge(
    fleet: &mut crate::fleet::Fleet,
    found: &mut crate::found::Found,
    me: &crate::claims::Me,
    agent: &crate::dispatch::Agent,
) -> Result<()> {
    let list = List::of_host()?;
    let root = fleet.repo.dir.clone();
    let listed = list.read()?;
    let entry = listed.checkouts.iter().find(|entry| entry.checkout == root);
    let (named_harness, applied) = match entry {
        Some(entry) => (entry.harness, entry.harness_applied.clone()),
        None => (checkout_list::asked_of(&root).harness, None),
    };
    let desired = named_harness.map(|harness| harness.key().to_string()).unwrap_or_default();
    if !wants_converge(applied.as_deref(), &desired) {
        return Ok(());
    }
    // This tick opened its fleet before the name changed. Its selection is
    // the previous harness, so a continuation now would land on that one and
    // a mark would stop the tick that opens on the harness now in force.
    if fleet.pinned != named_harness {
        return Ok(());
    }
    let target = match named_harness {
        Some(harness) => harness,
        None => match fleet.first_harness() {
            Ok(harness) => harness,
            Err(_) => return Ok(()),
        },
    };
    if cannot_start(&fleet.repo, target, fleet.now).is_some() {
        return Ok(());
    }
    let landed = match crate::switch::onto(fleet, found, me, agent, target) {
        Ok(true) => true,
        Ok(false) | Err(_) => false,
    };
    if !landed || entry.is_none() || applied.as_deref() == Some(desired.as_str()) {
        return Ok(());
    }
    list.change(|listed| {
        if let Some(entry) = listed.checkouts.iter_mut().find(|entry| entry.checkout == root) {
            entry.harness_applied = Some(desired);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_is_its_key_or_default() {
        assert_eq!(harness_or_default("kimi"), Ok(To::Value(Harness::Kimi)));
        assert_eq!(harness_or_default("default"), Ok(To::Default));
        let error = harness_or_default("nope").unwrap_err();
        assert!(error.contains("is no harness"), "{error}");
        assert!(error.contains("`nope`"), "{error}");
    }

    #[test]
    fn a_named_harness_is_retried_and_a_cleared_one_lands_once() {
        assert!(!wants_converge(None, ""));
        assert!(!wants_converge(Some(""), ""));
        assert!(wants_converge(Some("kimi"), ""));
        assert!(wants_converge(None, "kimi"));
        assert!(wants_converge(Some("kimi"), "kimi"));
        assert!(wants_converge(Some(""), "kimi"));
    }
}
