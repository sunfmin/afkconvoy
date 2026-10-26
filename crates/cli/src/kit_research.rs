//! Sourced research of the models the host kit lists, once an identity is
//! answered for.
use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::kit::{Kit, Seen};

const LIMIT: Duration = Duration::from_secs(15 * 60);
/// The good_at of a model a valid research answer gave no dated source: it is
/// settled, and never started on.
pub(crate) const UNKNOWN: &str = "not researched";
/// The good_at of a model its harness lists that research has not answered
/// for yet: never started on, and researched at the next kit write.
pub(crate) const PENDING: &str = "research pending";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub url: String,
    /// The source's publication or update date, YYYY-MM-DD.
    pub date: String,
}

pub(super) fn identity(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_lowercase()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    harness_order: Vec<String>,
    model_order: Vec<ModelOrder>,
    models: Vec<Finding>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelOrder {
    harness: String,
    models: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Finding {
    harness: String,
    model: String,
    good_at: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    sources: Vec<Source>,
}

/// Runs on the kit's first, on its default model until research places one.
/// Failure is data: writing the kit must still succeed.
pub fn session(kit: &Kit, prompt: &str) -> Result<String> {
    let first = kit.first().context("the host kit holds no harness")?;
    let kept = crate::kept::keep(first.harness, false)?;
    let question = question(kit, &kept.display().to_string(), prompt)?;
    match crate::orca::ask(&question, LIMIT)? {
        Some((true, answer)) => Ok(answer),
        _ => bail!("research failed or gave no answer within 15 minutes"),
    }
}

/// Headless research with live search enabled on harnesses that require it.
fn question(kit: &Kit, kept: &str, prompt: &str) -> Result<String> {
    let first = kit.first().context("no harness answered")?;
    let command = kit.command(first.harness).context("no worker launch command")?;
    let command = if first.harness == afkconvoy_core::harness::Harness::Codex {
        format!("{command} --search")
    } else {
        command.to_string()
    };
    let mut question =
        first.harness.prompt_question(&command, kept, first.model.as_deref(), prompt);
    if first.harness == afkconvoy_core::harness::Harness::ClaudeCode {
        question.push_str(" --allowedTools WebSearch WebFetch");
    }
    Ok(question)
}

/// Researches each identity research has not answered for — a listed model
/// `research pending`, or one named with no dated research — once across all
/// harnesses. A valid answer places the sourced ones and settles the rest as
/// `not researched` in their fixed slots. A failed session or a malformed
/// answer applies nothing, including its order: what was pending stays
/// pending, for the next kit write. Sourced research is never overwritten.
pub fn research(
    before: &Kit,
    kit: &mut Kit,
    now: u64,
    ask: impl FnOnce(&Kit, &str) -> Result<String>,
) {
    let old: BTreeSet<_> =
        before.harnesses.iter().flat_map(|h| &h.models).map(|m| identity(&m.model)).collect();
    let mut new = BTreeSet::new();
    let mut targets = vec![];
    for held in &mut kit.harnesses {
        for model in &mut held.models {
            if !model.awaits_research() {
                continue;
            }
            if model.good_at.as_deref().is_some_and(|good| good != PENDING) {
                eprintln!(
                    "afkconvoy: {} {}: held good_at has no dated research; researching it",
                    held.harness.key(),
                    model.model
                );
                model.good_at = None;
            }
            model.sources.clear();
            model.researched_at = None;
            if new.insert(identity(&model.model)) {
                targets.push(json!({"harness": held.harness.key(), "model": model.model}));
            }
        }
    }
    if new.is_empty() {
        return;
    }
    let prompt = format!(
        "Research new coding-agent models using web search now. Use primary and high-trust \
         sources: vendor model cards and release notes, SWE-bench, Terminal-Bench and Aider \
         leaderboards. Do not rely on training knowledge. \
         Research only these identities (one entry per identity): {}.\n\
         Context: every kept model, under all its names and harness display names: {}.\n\
         Answer with exactly one JSON object, no fences: {{\"harness_order\":[\"pi\",...],\
         \"model_order\":[{{\"harness\":\"pi\",\"models\":[\"kept id\",...]}}],\
         \"models\":[{{\"harness\":\"pi\",\"model\":\"id from targets\",\"good_at\":\
         \"a few words: strengths and weaknesses for long refactors, small fixes, review, speed, cost\",\
         \"names\":[\"aliases the dated sources tie to this listed id\"],\
         \"sources\":[{{\"url\":\"https://...\",\"date\":\"YYYY-MM-DD\"}}]}}]}}. \
         Include every kept harness exactly once in preference order for a coding fleet. \
         Include model_order for every kept harness, each kept id exactly once, using held \
         research for old identities. Include each target exactly once. If evidence is unavailable, \
         omit that model. Existing models need no research. Today is epoch second {now}.",
        json!(targets),
        json!(kit.harnesses),
    );
    let result = ask(kit, &prompt).and_then(|answer| checked(&answer, kit, &targets));
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            eprintln!(
                "afkconvoy: host kit research not applied: {error:#}; keeping fixed order, held \
                 good_at and research pending for the next kit write"
            );
            return;
        }
    };
    let sourced: Vec<_> = answer.models.iter().filter(|finding| valid(finding)).collect();
    if sourced.is_empty() {
        eprintln!(
            "afkconvoy: host kit research has no dated source URL; keeping fixed order and not researched"
        );
        settle(kit, &new);
        return;
    }
    for held in &mut kit.harnesses {
        for model in &mut held.models {
            if !new.contains(&identity(&model.model)) {
                continue;
            }
            if let Some(finding) =
                sourced.iter().find(|f| identity(&f.model) == identity(&model.model))
            {
                model.good_at = Some(finding.good_at.clone());
                model.sources = finding.sources.clone();
                model.researched_at = Some(now);
                for name in &finding.names {
                    if name != &model.model && !model.names.contains(name) {
                        model.names.push(name.clone());
                    }
                }
            } else {
                eprintln!(
                    "afkconvoy: {} {} not researched: missing finding or dated source URL; keeping fixed place",
                    held.harness.key(),
                    model.model
                );
                model.good_at = Some(UNKNOWN.into());
            }
        }
        // An unsourced new model stays in its fixed slot. Old research can be
        // ranked with the new research without paying to research it again.
        let mut ranked: Vec<Seen> = held
            .models
            .iter()
            .filter(|m| old.contains(&identity(&m.model)) || m.researched_at == Some(now))
            .cloned()
            .collect();
        let order = answer
            .model_order
            .iter()
            .find(|o| o.harness == held.harness.key())
            .expect("checked harness order");
        ranked.sort_by_key(|m| order.models.iter().position(|id| id == &m.model));
        let mut ranked = ranked.into_iter();
        for model in &mut held.models {
            if old.contains(&identity(&model.model)) || model.researched_at == Some(now) {
                *model = ranked.next().expect("one replacement per researched slot");
            }
        }
    }
    // Research may rank new discoveries, but established host priority wins.
    crate::kit::retain_model_order(before, kit);
}

/// Settles each model of an identity in `asked` as `not researched`: a valid
/// answer gave it no dated source.
fn settle(kit: &mut Kit, asked: &BTreeSet<String>) {
    for model in kit.harnesses.iter_mut().flat_map(|held| &mut held.models) {
        if asked.contains(&identity(&model.model)) {
            model.good_at = Some(UNKNOWN.into());
        }
    }
}

fn valid(finding: &Finding) -> bool {
    !finding.good_at.trim().is_empty()
        && !finding.sources.is_empty()
        && finding.sources.iter().all(|s| {
            (s.url.starts_with("https://") || s.url.starts_with("http://"))
                && s.url.split_once("://").is_some_and(|(_, tail)| {
                    !tail.is_empty()
                        && !tail.starts_with('/')
                        && !tail.chars().any(char::is_whitespace)
                })
                && s.date.len() == 10
                && s.date
                    .as_bytes()
                    .iter()
                    .enumerate()
                    .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
        })
}

fn checked(text: &str, kit: &Kit, targets: &[serde_json::Value]) -> Result<Answer> {
    let answer: Answer = serde_json::from_str(text).context("research is not one JSON object")?;
    let order: BTreeSet<_> = answer.harness_order.iter().map(String::as_str).collect();
    let held: BTreeSet<_> = kit.harnesses.iter().map(|h| h.harness.key()).collect();
    if order != held || order.len() != answer.harness_order.len() {
        bail!("research harness order must name every kept harness exactly once");
    }
    let mut ordered = BTreeSet::new();
    for order in &answer.model_order {
        if !ordered.insert(order.harness.as_str()) {
            bail!("duplicate harness in model order");
        }
        let h = kit
            .harnesses
            .iter()
            .find(|h| h.harness.key() == order.harness)
            .context("model order names a harness not kept")?;
        let ids: BTreeSet<_> = h.models.iter().map(|m| m.model.as_str()).collect();
        let named: BTreeSet<_> = order.models.iter().map(String::as_str).collect();
        if ids != named || named.len() != order.models.len() {
            bail!("model order must name each kept model exactly once");
        }
    }
    if ordered != held {
        bail!("model order must name each kept harness exactly once");
    }
    let mut seen = BTreeSet::new();
    for model in &answer.models {
        if !targets.iter().any(|t| t["harness"] == model.harness && t["model"] == model.model) {
            bail!("research names a model not requested: {} {}", model.harness, model.model);
        }
        if !seen.insert(identity(&model.model)) {
            bail!("research names a model twice: {}", model.model);
        }
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn research_enables_live_web_search_on_the_first_harness() {
        let mut kit = kit();
        let asked = question(&kit, "/kept", "Research now.").unwrap();
        assert!(asked.contains("--print 'Research now.' --allowedTools WebSearch WebFetch"));
        kit.harnesses[0].harness = afkconvoy_core::harness::Harness::Codex;
        kit.harnesses[0].command = "codex --dangerously-bypass-approvals-and-sandbox".into();
        let asked = question(&kit, "/kept", "Research now.").unwrap();
        assert!(asked.contains("--search exec --skip-git-repo-check --ephemeral 'Research now.'"));
        assert!(!asked.contains("--model"), "research starts on the default: {asked}");
        assert!(asked.starts_with("export CODEX_HOME='/kept'"));
    }
    fn kit() -> Kit {
        serde_json::from_value(json!({"harnesses": [
            {"harness":"claude-code", "command":"claude", "models":[{"model":"fast", "names":["default", "Fast"], "good_at":"research pending"}, {"model":"deep", "good_at":"research pending"}]},
            {"harness":"pi", "command":"pi", "models":[{"model":"other", "good_at":"research pending"}]}
        ]})).unwrap()
    }
    fn answer() -> serde_json::Value {
        let finding = |h, m| json!({"harness":h,"model":m,"good_at":"review; costly", "sources":[{"url":"https://vendor.example/card","date":"2026-10-24"}]});
        json!({"harness_order":["pi","claude-code"],"model_order":[{"harness":"pi","models":["other"]},{"harness":"claude-code","models":["deep","fast"]}],"models":[finding("pi","other"),finding("claude-code","deep"),finding("claude-code","fast")]})
    }
    #[test]
    fn a_sourced_answer_places_the_models_with_their_names_in_its_order() {
        let mut after = kit();
        let mut answer = answer();
        answer["models"][1]["names"] = json!(["claude-opus-5-5", "default", "opus"]);
        research(&Kit::default(), &mut after, 100, |first, prompt| {
            // Nothing is placed yet: research runs on the first's default.
            assert_eq!(first.first().unwrap().model, None);
            assert!(
                prompt.contains("default")
                    && prompt.contains("Fast")
                    && prompt.contains("SWE-bench")
            );
            assert!(!prompt.contains("probe"), "{prompt}");
            Ok(answer.to_string())
        });
        assert_eq!(after.harnesses[0].harness.key(), "claude-code");
        assert_eq!(after.harnesses[0].models[0].model, "deep");
        assert!(after.harnesses[0].models[0].names.contains(&"opus".to_string()));
        assert_eq!(after.range()[0].model("OPUS").unwrap().id, "deep");
        assert_eq!(after.first().unwrap().model.as_deref(), Some("deep"));
        for m in after.harnesses.iter().flat_map(|h| &h.models) {
            assert_eq!(m.good_at.as_deref(), Some("review; costly"));
            assert_eq!(m.researched_at, Some(100));
            assert_eq!(m.sources.len(), 1);
        }
    }
    #[test]
    fn a_failed_or_malformed_answer_leaves_research_pending_for_the_next_write() {
        let mut unknown = answer();
        unknown["models"][0]["model"] = json!("not kept");
        let mut duplicate = answer();
        let repeated = duplicate["models"][0].clone();
        duplicate["models"].as_array_mut().unwrap().push(repeated);
        let failed = |_: &Kit, _: &str| bail!("no answer within 15 minutes");
        for answered in
            [unknown.to_string(), duplicate.to_string(), "invalid JSON".into(), String::new()]
        {
            let mut after = kit();
            if answered.is_empty() {
                research(&Kit::default(), &mut after, 100, failed);
            } else {
                research(&Kit::default(), &mut after, 100, |_, _| Ok(answered));
            }
            assert_eq!(after, kit(), "nothing applied, its order neither");
            // The harness stays on its default: nothing is placed.
            assert_eq!(after.first().unwrap().model, None);
            assert!(after.range().iter().all(|held| held.models.is_empty()));
            // The next kit write researches them again, and places them.
            let before = after.clone();
            research(&before, &mut after, 200, |_, _| Ok(answer().to_string()));
            assert!(
                after
                    .harnesses
                    .iter()
                    .flat_map(|h| &h.models)
                    .all(|m| m.researched_at == Some(200))
            );
            assert_eq!(after.first().unwrap().model.as_deref(), Some("deep"));
        }
    }
    #[test]
    fn omitted_or_unsourced_models_settle_as_not_researched_in_their_fixed_place() {
        for missing in [false, true] {
            let mut value = answer();
            if missing {
                value["models"].as_array_mut().unwrap().pop();
            } else {
                value["models"][2]["sources"] = json!([]);
            }
            let mut after = kit();
            research(&Kit::default(), &mut after, 100, |_, _| Ok(value.to_string()));
            let claude = after.harnesses.iter().find(|h| h.harness.key() == "claude-code").unwrap();
            assert_eq!(claude.models[0].model, "fast");
            assert_eq!(claude.models[0].good_at.as_deref(), Some(UNKNOWN));
            assert_eq!(claude.models[1].researched_at, Some(100));
            assert_eq!(after.first().unwrap().model.as_deref(), Some("deep"));
            assert_eq!(after.range()[0].models.len(), 1);
            // Settled: it is not researched again.
            let before = after.clone();
            research(&before, &mut after, 200, |_, _| panic!("research answered for it"));
        }
        // An answer with no dated source at all settles every model asked of.
        let mut value = answer();
        for finding in value["models"].as_array_mut().unwrap() {
            finding["sources"] = json!([]);
        }
        let mut after = kit();
        research(&Kit::default(), &mut after, 100, |_, _| Ok(value.to_string()));
        assert!(
            after
                .harnesses
                .iter()
                .flat_map(|h| &h.models)
                .all(|m| m.good_at.as_deref() == Some(UNKNOWN))
        );
        assert_eq!(after.first().unwrap().model, None);
    }
    #[test]
    fn a_model_named_with_no_research_is_started_on_until_research_answers_for_it() {
        let named: Kit = serde_json::from_value(json!({"harnesses": [
            {"harness":"claude-code", "command":"claude", "models":[{"model":"fast"}, {"model":"deep", "good_at":"old words"}]},
            {"harness":"pi", "command":"pi", "models":[{"model":"other"}]}
        ]})).unwrap();
        assert_eq!(named.first().unwrap().model.as_deref(), Some("fast"));
        let mut after = named.clone();
        research(&named, &mut after, 100, |_, _| bail!("no answer"));
        // Unsourced words are not what a caster is told; the models still run.
        assert_eq!(after.harnesses[0].models[1].good_at, None);
        assert_eq!(after.range()[0].models.len(), 2);
        research(&named, &mut after, 200, |_, prompt| {
            assert!(prompt.contains(r#"{"harness":"claude-code","model":"fast"}"#), "{prompt}");
            Ok(answer().to_string())
        });
        // Its established order stays: research ranks only what it newly places.
        assert_eq!(after.first().unwrap().model.as_deref(), Some("fast"));
        assert!(
            after.harnesses.iter().flat_map(|h| &h.models).all(|m| m.researched_at == Some(200))
        );
    }
    #[test]
    fn nothing_awaiting_research_starts_no_session_however_old_its_research() {
        let mut before = kit();
        research(&Kit::default(), &mut before, 100, |_, _| Ok(answer().to_string()));
        before.harnesses[1].models[0].good_at = Some(UNKNOWN.into());
        before.harnesses[1].models[0].sources.clear();
        before.harnesses[1].models[0].researched_at = None;
        let mut after = before.clone();
        research(&before, &mut after, 9999999, |_, _| panic!("must not research again"));
        assert_eq!(after, before);
    }
    #[test]
    fn an_identity_is_researched_once_across_its_names_and_harnesses() {
        let mut before = kit();
        research(&Kit::default(), &mut before, 100, |_, _| Ok(answer().to_string()));
        let mut after = before.clone();
        after.harnesses[0].models[0].names.push("new alias".into());
        let mut moved = after.harnesses[0].models[0].clone();
        moved.model = "provider/fast".into();
        after.harnesses[1].models.push(moved);
        research(&before, &mut after, 150, |_, _| panic!("same identity"));
        let listed: Seen =
            serde_json::from_value(json!({"model":"new-generation", "good_at": PENDING})).unwrap();
        after.harnesses[0].models.push(listed);
        research(&before, &mut after, 200, |_, prompt| {
            let targets = prompt
                .split("one entry per identity): ")
                .nth(1)
                .unwrap()
                .split(".\nContext")
                .next()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(targets).unwrap(),
                json!([{"harness":"claude-code", "model":"new-generation"}])
            );
            Ok(json!({"harness_order":["claude-code","pi"],"model_order":[{"harness":"claude-code","models":["new-generation","deep","fast"]},{"harness":"pi","models":["other","provider/fast"]}],"models":[{"harness":"claude-code","model":"new-generation","good_at":"small fixes","sources":[{"url":"https://vendor.example/new","date":"2026-10-24"}]}]}).to_string())
        });
        // Placed, it follows the models established before it.
        let claude = &after.harnesses[0].models;
        let ids: Vec<&str> = claude.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(ids, ["deep", "fast", "new-generation"]);
        assert_eq!(claude[1], before.harnesses[0].models[1]);
        assert_eq!(claude[2].good_at.as_deref(), Some("small fixes"));
    }
}
