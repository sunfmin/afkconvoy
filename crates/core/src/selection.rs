//! One worker selection (sunfmin/control#225): what a worker runs — its
//! harness and its model — decided once, from facts the fleet gathers once.
//! The role's range (docs/adr/0020), the runnable host kit, the harness a
//! checkout names (docs/adr/0045), the harnesses paused on the host, the
//! issue's model label (docs/adr/0022, docs/adr/0030) and the harness user
//! default are each a fact here. Dispatch, switch, patrol, plan and the
//! panel all ask `select`, and its answer says how the model goes on the
//! start line, whether or not the harness keeps a session of its own.

use serde::Serialize;

use crate::casting::{self, Cast, Held, Why};
use crate::harness::Harness;
use crate::team::Role;

/// The harness a checkout names on its entry on the checkout list, with the
/// checkout, as a human is told it.
#[derive(Debug, Clone, Copy)]
pub struct Named<'a> {
    pub harness: Harness,
    pub checkout: &'a str,
}

/// A harness's user config, read once, that its user default is read from
/// (`Harness::automatic_model`): its text, the file it was read from, and
/// the ids the host kit lists for that harness.
#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    pub harness: Option<Harness>,
    pub text: String,
    pub file: String,
    pub listed: Vec<String>,
}

/// Everything a worker selection is decided from, gathered once.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    /// The host kit whole, in its order.
    pub kit: &'a [Held],
    /// What of it this host can start now for this worker: less what is
    /// paused, faulted, refused by the issue or cannot start.
    pub runnable: &'a [Held],
    /// Each harness left out of `runnable`, with why, as a human is told it.
    pub left_out: &'a [(Harness, String)],
    /// The harness the checkout names. `None`: the host kit's order chooses.
    pub named: Option<Named<'a>>,
    /// Each harness paused on the host, with its pause as a human is told it.
    pub paused: &'a [(Harness, String)],
    /// The worker's role. `None`: an unbound repo's worker, or one of no
    /// role, which ranges over the host kit whole.
    pub role: Option<&'a Role>,
    /// The issue's labels: none for a patrol, a watch or a cut.
    pub labels: &'a [String],
    /// Whether the worker is an issue's: a user default outside its role's
    /// range is said of the issue, else of the role.
    pub of_issue: bool,
    /// Each user config a harness reads its user default from.
    pub user: &'a [UserConfig],
}

/// What a worker runs: its harness and model, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Selection {
    /// The harness, and what the model label decided (`casting::preferred`).
    pub cast: Cast,
    /// The model the worker starts on: the followed label's id, or the
    /// harness user default where its harness reads one. `None`: the
    /// harness's own default, with no model on the start line.
    pub model: Option<String>,
    /// Where that model came from, in the harness's words; `None` of a
    /// harness that reads no user default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
}

impl Selection {
    pub fn harness(&self) -> Harness {
        self.cast.harness
    }

    /// Whether a model label named the model: it takes precedence over the
    /// user's own preference.
    pub fn explicit(&self) -> bool {
        self.cast.why == Why::ModelLabel
    }

    /// `launch`, the worker launch command of its harness, with the model on
    /// it in that harness's way — once, and as written where it names none.
    /// Every harness's start line is made so, session or not.
    pub fn start_line(&self, launch: &str) -> String {
        self.harness().ensure_model(launch, self.model.as_deref())
    }
}

/// Why no worker can be started now, worded as the user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsafe {
    /// Held while a harness it would run is paused.
    Paused(String),
    /// No harness is safe for it: its role's range, the named harness, or
    /// the runnable host kit holds none.
    NothingSafe(String),
    /// The harness was selected, but its user default refuses the start.
    Refused { cast: Cast, why: String },
}

impl Unsafe {
    pub fn said(&self) -> &str {
        match self {
            Unsafe::Paused(why) | Unsafe::NothingSafe(why) | Unsafe::Refused { why, .. } => why,
        }
    }
}

impl std::fmt::Display for Unsafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.said())
    }
}

impl std::error::Error for Unsafe {}

/// The one worker selection. The role's range is resolved against the host
/// kit whole before anything unavailable is left out, so a pause or a fault
/// never broadens it. A checkout that names a harness runs that harness or
/// nothing: no other harness of the range stands in for it. Otherwise the
/// first harness of the range the host can start, in kit order. One model
/// label names a model of it where the range holds that model; anything
/// else is its user default.
pub fn select(facts: &Facts) -> Result<Selection, Unsafe> {
    let allowed = match facts.role {
        Some(role) => {
            if let Some(why) = casting::nothing_safe(role, facts.kit) {
                return Err(Unsafe::NothingSafe(why));
            }
            casting::range(role, facts.kit)
        }
        None => facts.kit.to_vec(),
    };
    if let Some(named) = facts.named {
        named_allowed(named, facts, &allowed)?;
    }
    let range = runnable_range(facts, &allowed);
    let Some(first) = range.first() else { return Err(nothing_runnable(facts)) };
    if let Some((_, said)) = facts.paused.iter().find(|(harness, _)| *harness == first.harness) {
        return Err(Unsafe::Paused(format!("held while its harness is paused: {said}")));
    }
    let kit = if facts.kit.is_empty() { range.as_slice() } else { facts.kit };
    let cast = casting::preferred(facts.labels, &range, kit).map_err(Unsafe::NothingSafe)?;
    started_on(facts, &allowed, cast)
}

/// Refuses a named harness the host kit does not hold, or the role's range
/// does not hold.
fn named_allowed(named: Named, facts: &Facts, allowed: &[Held]) -> Result<(), Unsafe> {
    let key = named.harness.key();
    let refused = |why: String| {
        Err(Unsafe::NothingSafe(format!(
            "checkout {} names harness {key}: no eligible harness: {why}",
            named.checkout
        )))
    };
    if !facts.kit.iter().any(|held| held.harness == named.harness) {
        return refused(format!("`{key}` is not a harness this host kit holds"));
    }
    if let Some(role) = facts.role {
        if let Some(why) = crate::team::unlimited(role, named.harness) {
            return refused(format!("the role `{}` holds no limits for `{key}`: {why}", role.name));
        }
        if !allowed.iter().any(|held| held.harness == named.harness) {
            return refused(format!("`{key}` is outside the roster range of role `{}`", role.name));
        }
    }
    Ok(())
}

/// `allowed` as the host can start it now, in the runnable kit's order: each
/// harness with the models of it both hold — one the runnable kit holds no
/// model of is its default's. A checkout that names a harness has that alone.
fn runnable_range(facts: &Facts, allowed: &[Held]) -> Vec<Held> {
    let mut range = vec![];
    for available in facts.runnable {
        if facts.named.is_some_and(|named| named.harness != available.harness) {
            continue;
        }
        if let Some(held) = allowed.iter().find(|held| held.harness == available.harness) {
            let mut held = held.clone();
            held.models.retain(|model| available.models.iter().any(|m| m.id == model.id));
            if !held.models.is_empty() || available.models.is_empty() {
                range.push(held);
            }
        }
    }
    range
}

/// Why nothing the range holds can start now.
fn nothing_runnable(facts: &Facts) -> Unsafe {
    if let Some(named) = facts.named {
        let key = named.harness.key();
        let why = facts
            .left_out
            .iter()
            .find(|(harness, _)| *harness == named.harness)
            .map(|(_, why)| why.clone())
            .unwrap_or_else(|| format!("`{key}` is not a harness this host kit holds"));
        return Unsafe::NothingSafe(format!(
            "checkout {} names harness {key}: no eligible harness: {why}",
            named.checkout
        ));
    }
    if let Some(role) = facts.role {
        return Unsafe::NothingSafe(format!(
            "the role `{}` has no currently eligible harness in its range; waiting for recovery",
            role.name
        ));
    }
    if !facts.paused.is_empty() {
        let each: Vec<&str> = facts.paused.iter().map(|(_, said)| said.as_str()).collect();
        return Unsafe::Paused(format!(
            "held while every harness of the host kit is paused: {}",
            each.join("; ")
        ));
    }
    let whys: Vec<&str> = facts.left_out.iter().map(|(_, why)| why.as_str()).collect();
    let whys = if whys.is_empty() { String::new() } else { format!(": {}", whys.join("; ")) };
    Unsafe::NothingSafe(format!("no eligible harness in the host kit{whys}: run `afkconvoy kit`"))
}

/// The model `cast`'s harness starts on: the followed label's, else its user
/// default where it reads one. A user default the host kit does not list, or
/// the role's range does not hold, refuses the start.
fn started_on(facts: &Facts, allowed: &[Held], cast: Cast) -> Result<Selection, Unsafe> {
    let harness = cast.harness;
    if let Some(model) = &cast.model {
        let source = harness
            .automatic_model(Some(model), "", "", &[])
            .and_then(Result::ok)
            .map(|(_, source)| source);
        let model = Some(model.clone());
        return Ok(Selection { cast, model, source });
    }
    let empty = UserConfig::default();
    let user = facts.user.iter().find(|user| user.harness == Some(harness)).unwrap_or(&empty);
    let (model, source) = match harness.automatic_model(None, &user.text, &user.file, &user.listed)
    {
        None => return Ok(Selection { cast, model: None, source: None }),
        Some(Err(why)) => return Err(Unsafe::Refused { cast, why }),
        Some(Ok(chosen)) => chosen,
    };
    if let (Some(role), Some(name)) = (facts.role, model.as_deref()) {
        let held = allowed.iter().find(|held| held.harness == harness);
        if !held.is_some_and(|held| held.models.iter().any(|allowed| allowed.id == name)) {
            let why = match facts.of_issue {
                true => harness.outside_issue(name),
                false => harness.outside_role(name, &role.name),
            };
            if let Some(why) = why {
                return Err(Unsafe::Refused { cast, why });
            }
        }
    }
    Ok(Selection { cast, model, source: Some(source) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casting::Model;

    fn held(harness: Harness, models: &[&str]) -> Held {
        Held { harness, models: models.iter().map(|model| Model::from(*model)).collect() }
    }

    fn kit() -> Vec<Held> {
        vec![
            held(Harness::ClaudeCode, &["opus", "haiku"]),
            held(Harness::Codex, &["gpt-x", "gpt-y"]),
            held(Harness::Pi, &[]),
        ]
    }

    fn facts<'a>(kit: &'a [Held], labels: &'a [String]) -> Facts<'a> {
        Facts {
            kit,
            runnable: kit,
            left_out: &[],
            named: None,
            paused: &[],
            role: None,
            labels,
            of_issue: true,
            user: &[],
        }
    }

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn role(limits: &[Harness]) -> Role {
        let team = crate::team::tests::rostered("  dev: { run: [{ harness: pi }] }\n").unwrap();
        let mut role = team.role("dev").unwrap().clone();
        role.roster = None;
        role.limits = limits.to_vec();
        role
    }

    fn codex_config(model: &str) -> UserConfig {
        UserConfig {
            harness: Some(Harness::Codex),
            text: format!("model = '{model}'\n"),
            file: "/home/.codex/config.toml".into(),
            listed: vec!["gpt-x".into(), "gpt-y".into(), "gpt-z".into()],
        }
    }

    #[test]
    fn with_no_name_the_first_runnable_harness_of_the_kit_runs_its_own_default() {
        let kit = kit();
        let selection = select(&facts(&kit, &[])).unwrap();
        assert_eq!(selection.harness(), Harness::ClaudeCode);
        assert_eq!(
            (selection.model.as_deref(), selection.source, selection.explicit()),
            (None, None, false)
        );
        assert_eq!(selection.start_line("claude"), "claude");
    }

    #[test]
    fn a_named_harness_runs_alone_and_nothing_stands_in_for_it() {
        let kit = kit();
        let named = Named { harness: Harness::Codex, checkout: "/c" };
        let chosen = select(&Facts { named: Some(named), ..facts(&kit, &[]) }).unwrap();
        assert_eq!(chosen.harness(), Harness::Codex);
        // Not runnable: no other harness of the kit starts in its place.
        let runnable = [kit[0].clone(), kit[2].clone()];
        let left_out = [(Harness::Codex, "`codex` is paused on this host: down".to_string())];
        let unsafe_ = select(&Facts {
            named: Some(named),
            runnable: &runnable,
            left_out: &left_out,
            ..facts(&kit, &[])
        })
        .unwrap_err();
        assert_eq!(
            unsafe_,
            Unsafe::NothingSafe(
                "checkout /c names harness codex: no eligible harness: `codex` is paused on this \
                 host: down"
                    .into()
            )
        );
        // One the host kit does not hold.
        let dsh = Named { harness: Harness::Dsh, checkout: "/c" };
        let gone = select(&Facts { named: Some(dsh), ..facts(&kit, &[]) }).unwrap_err();
        assert!(gone.said().ends_with("`dsh` is not a harness this host kit holds"), "{gone}");
        // One the role holds no limits for: no worker of the role.
        let role = role(&[Harness::ClaudeCode]);
        let unlimited =
            select(&Facts { named: Some(named), role: Some(&role), ..facts(&kit, &[]) })
                .unwrap_err();
        assert!(unlimited.said().contains("holds no limits for `codex`"), "{unlimited}");
    }

    #[test]
    fn a_paused_harness_holds_a_worker_back_and_every_one_paused_says_so() {
        let kit = kit();
        let paused = [
            (Harness::ClaudeCode, "`claude-code` is paused".to_string()),
            (Harness::Codex, "`codex` is paused".to_string()),
            (Harness::Pi, "`pi` is paused".to_string()),
        ];
        let held = select(&Facts { runnable: &[], paused: &paused, ..facts(&kit, &[]) });
        assert_eq!(
            held,
            Err(Unsafe::Paused(
                "held while every harness of the host kit is paused: `claude-code` is paused; \
                 `codex` is paused; `pi` is paused"
                    .into()
            ))
        );
        // One paused, the next runs.
        let runnable = &kit[1..];
        let chosen = select(&Facts { runnable, paused: &paused[..1], ..facts(&kit, &[]) }).unwrap();
        assert_eq!(chosen.harness(), Harness::Codex);
        // A paused harness the runnable kit still holds is never started.
        let stale = select(&Facts { paused: &paused[..1], ..facts(&kit, &[]) });
        assert_eq!(
            stale,
            Err(Unsafe::Paused("held while its harness is paused: `claude-code` is paused".into()))
        );
    }

    #[test]
    fn a_role_runs_only_within_its_range_and_an_empty_one_is_nothing_safe() {
        let kit = kit();
        let codex = role(&[Harness::Codex]);
        let chosen = select(&Facts { role: Some(&codex), ..facts(&kit, &[]) }).unwrap();
        assert_eq!(chosen.harness(), Harness::Codex);
        let dsh = role(&[Harness::Dsh]);
        let none = select(&Facts { role: Some(&dsh), ..facts(&kit, &[]) }).unwrap_err();
        assert!(
            matches!(&none, Unsafe::NothingSafe(why) if why.contains("has no harness on this host it holds limits for")),
            "{none}"
        );
        // A range whose harnesses cannot start now waits for recovery; a
        // fault never broadens it.
        let runnable = [kit[0].clone()];
        let waiting =
            select(&Facts { role: Some(&codex), runnable: &runnable, ..facts(&kit, &[]) })
                .unwrap_err();
        assert!(waiting.said().ends_with("waiting for recovery"), "{waiting}");
    }

    #[test]
    fn an_unfollowed_label_runs_the_user_default_and_carries_its_notice() {
        let kit = kit();
        let opus = labels(&["model:opus"]);
        let followed = select(&facts(&kit, &opus)).unwrap();
        assert_eq!((followed.model.as_deref(), followed.explicit()), (Some("opus"), true));
        assert_eq!(followed.start_line("claude"), "claude --model 'opus'");
        assert_eq!(followed.start_line("claude --model 'opus'"), "claude --model 'opus'");
        let nothing = labels(&["model:gpt-x"]);
        let unfollowed = select(&facts(&kit, &nothing)).unwrap();
        assert_eq!(unfollowed.harness(), Harness::ClaudeCode);
        assert_eq!((unfollowed.model.as_deref(), unfollowed.explicit()), (None, false));
        assert_eq!(unfollowed.cast.unmatched.as_deref(), Some("`model:gpt-x`"));
        assert!(unfollowed.cast.notice.unwrap().contains("names no model"));
    }

    #[test]
    fn a_harness_user_default_is_its_model_and_one_outside_the_range_refuses() {
        let kit = kit();
        let runnable = &kit[1..2];
        let user = [codex_config("gpt-y")];
        let chosen = select(&Facts { runnable, user: &user, ..facts(&kit, &[]) }).unwrap();
        assert_eq!((chosen.model.as_deref(), chosen.explicit()), (Some("gpt-y"), false));
        assert_eq!(chosen.source, Some("user_selection"));
        assert_eq!(chosen.start_line("codex --yolo"), "codex --yolo --model 'gpt-y'");
        // A label followed wins over the user's preference.
        let label = labels(&["model:gpt-x"]);
        let labelled = select(&Facts { runnable, user: &user, ..facts(&kit, &label) }).unwrap();
        assert_eq!(
            (labelled.model.as_deref(), labelled.source),
            (Some("gpt-x"), Some("model_label"))
        );
        // Outside the role's range: refused, said of the issue or the role.
        let team =
            crate::team::tests::rostered("  dev: { run: [{ harness: codex, model: gpt-x }] }\n")
                .unwrap();
        let rostered = team.role("dev").unwrap().clone();
        let narrow = [codex_config("gpt-y")];
        let refused =
            select(&Facts { runnable, user: &narrow, role: Some(&rostered), ..facts(&kit, &[]) })
                .unwrap_err();
        assert!(refused.said().contains("outside this issue's role range"), "{refused}");
        let patrol = select(&Facts {
            runnable,
            user: &narrow,
            role: Some(&rostered),
            of_issue: false,
            ..facts(&kit, &[])
        })
        .unwrap_err();
        assert!(patrol.said().contains("outside role `dev`'s range"), "{patrol}");
        // One the host kit does not list refuses too.
        let unlisted = [codex_config("gone")];
        let gone = select(&Facts { runnable, user: &unlisted, ..facts(&kit, &[]) }).unwrap_err();
        assert!(matches!(gone, Unsafe::Refused { .. }), "{gone}");
    }

    #[test]
    fn nothing_runnable_in_an_unbound_repo_says_what_left_each_out() {
        let kit = kit();
        let left_out = [(Harness::Pi, "pi cannot start".to_string())];
        let none = select(&Facts { runnable: &[], left_out: &left_out, ..facts(&kit, &[]) });
        assert_eq!(
            none,
            Err(Unsafe::NothingSafe(
                "no eligible harness in the host kit: pi cannot start: run `afkconvoy kit`".into()
            ))
        );
    }
}
