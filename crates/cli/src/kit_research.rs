//! Sourced, one-time research of new answered identities in the host kit.
use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::kit::{Kit, Seen};

const LIMIT: Duration = Duration::from_secs(15 * 60);
const UNKNOWN: &str = "not researched";

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
    sources: Vec<Source>,
}

/// Runs on the kit's first, through the same orca terminal and login as a probe.
/// Failure is data: writing the kit must still succeed.
pub fn session(kit: &Kit, prompt: &str) -> Result<String> {
    let first = kit.first().context("no harness answered")?;
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

/// Researches only identities not held before, once across all harnesses.
/// Missing or unsourced findings stay in their fixed slots. A malformed answer
/// applies nothing, including its order. Existing research is never overwritten.
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
            if model.good_at.as_deref().is_none_or(|s| s.trim().is_empty())
                || model.sources.is_empty()
                || model.researched_at.is_none()
            {
                if model.good_at.as_deref().is_some_and(|good| good != UNKNOWN) {
                    eprintln!(
                        "afkconvoy: {} {} not researched: held good_at has no dated research",
                        held.harness.key(),
                        model.model
                    );
                }
                model.good_at = Some(UNKNOWN.into());
                model.sources.clear();
                model.researched_at = None;
            }
            let id = identity(&model.model);
            if !old.contains(&id) {
                model.good_at = Some(UNKNOWN.into());
                model.sources.clear();
                model.researched_at = None;
                if new.insert(id) {
                    targets.push(json!({"harness": held.harness.key(), "model": model.model}));
                }
            }
        }
    }
    if new.is_empty() {
        return;
    }
    let prompt = format!(
        "Research new coding-agent models using web search now. Use primary and high-trust \
         sources: vendor model cards and release notes, SWE-bench, Terminal-Bench and Aider \
         leaderboards. Do not infer ability from probe latency or rely on training knowledge. \
         Research only these new identities (one entry per identity): {}.\n\
         Context: every kept model, under all its names and harness display names: {}.\n\
         Answer with exactly one JSON object, no fences: {{\"harness_order\":[\"pi\",...],\
         \"model_order\":[{{\"harness\":\"pi\",\"models\":[\"kept id\",...]}}],\
         \"models\":[{{\"harness\":\"pi\",\"model\":\"id from targets\",\"good_at\":\
         \"a few words: strengths and weaknesses for long refactors, small fixes, review, speed, cost\",\
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
                "afkconvoy: host kit research not applied: {error:#}; keeping fixed order and held good_at"
            );
            return;
        }
    };
    let sourced: Vec<_> = answer.models.iter().filter(|finding| valid(finding)).collect();
    if sourced.is_empty() {
        eprintln!(
            "afkconvoy: host kit research has no dated source URL; keeping fixed order and not researched"
        );
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
            } else {
                eprintln!(
                    "afkconvoy: {} {} not researched: missing finding or dated source URL; keeping fixed place",
                    held.harness.key(),
                    model.model
                );
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
        assert!(
            asked.contains("--search exec --skip-git-repo-check --ephemeral --model 'fast'"),
            "{asked}"
        );
        assert!(asked.starts_with("export CODEX_HOME='/kept'"));
    }
    fn kit() -> Kit {
        serde_json::from_value(json!({"harnesses": [
            {"harness":"claude-code", "command":"claude", "models":[{"model":"fast", "names":["default", "Fast"], "good_at":"not researched"}, {"model":"deep", "good_at":"not researched"}]},
            {"harness":"pi", "command":"pi", "models":[{"model":"other", "good_at":"not researched"}]}
        ]})).unwrap()
    }
    fn answer() -> serde_json::Value {
        let finding = |h, m| json!({"harness":h,"model":m,"good_at":"review; costly", "sources":[{"url":"https://vendor.example/card","date":"2026-10-24"}]});
        json!({"harness_order":["pi","claude-code"],"model_order":[{"harness":"pi","models":["other"]},{"harness":"claude-code","models":["deep","fast"]}],"models":[finding("pi","other"),finding("claude-code","deep"),finding("claude-code","fast")]})
    }
    #[test]
    fn a_sourced_answer_sets_research_and_preference_order() {
        let mut after = kit();
        research(&Kit::default(), &mut after, 100, |first, prompt| {
            assert_eq!(first.first().unwrap().model.as_deref(), Some("fast"));
            assert!(
                prompt.contains("default")
                    && prompt.contains("Fast")
                    && prompt.contains("SWE-bench")
            );
            Ok(answer().to_string())
        });
        assert_eq!(after.harnesses[0].harness.key(), "claude-code");
        assert_eq!(after.harnesses[0].models[0].model, "deep");
        for m in after.harnesses.iter().flat_map(|h| &h.models) {
            assert_eq!(m.good_at.as_deref(), Some("review; costly"));
            assert_eq!(m.researched_at, Some(100));
            assert_eq!(m.sources.len(), 1);
        }
    }
    #[test]
    fn unknown_duplicate_malformed_and_no_answers_apply_nothing() {
        let mut unknown = answer();
        unknown["models"][0]["model"] = json!("not kept");
        let mut duplicate = answer();
        let repeated = duplicate["models"][0].clone();
        duplicate["models"].as_array_mut().unwrap().push(repeated);
        for value in [unknown.to_string(), duplicate.to_string(), "invalid JSON".into()] {
            let mut after = kit();
            research(&Kit::default(), &mut after, 100, |_, _| Ok(value));
            assert_eq!(after.harnesses[0].harness.key(), "claude-code");
            assert!(
                after
                    .harnesses
                    .iter()
                    .flat_map(|h| &h.models)
                    .all(|m| m.good_at.as_deref() == Some(UNKNOWN) && m.researched_at.is_none())
            );
        }
        let mut after = kit();
        research(&Kit::default(), &mut after, 100, |_, _| bail!("no answer"));
        assert_eq!(after.harnesses[0].models[0].good_at.as_deref(), Some(UNKNOWN));
    }
    #[test]
    fn omitted_or_unsourced_models_keep_their_fixed_place() {
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
        }
    }
    #[test]
    fn no_new_identity_starts_no_session_however_old_its_research() {
        let mut before = kit();
        research(&Kit::default(), &mut before, 100, |_, _| Ok(answer().to_string()));
        let mut after = before.clone();
        research(&before, &mut after, 9999999, |_, _| panic!("must not research again"));
        assert_eq!(after, before);
    }
    #[test]
    fn only_a_new_answered_identity_is_researched_not_an_alias_or_another_harness() {
        let before = kit();
        let mut after = before.clone();
        after.harnesses[0].models[0].names.push("new alias".into());
        let mut moved = after.harnesses[0].models[0].clone();
        moved.model = "provider/fast".into();
        after.harnesses[1].models.push(moved);
        research(&before, &mut after, 100, |_, _| panic!("same identity"));
        after.harnesses[0].models[0].model = "new-generation".into();
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
            Ok(json!({"harness_order":["claude-code","pi"],"model_order":[{"harness":"claude-code","models":["new-generation","deep"]},{"harness":"pi","models":["other","provider/fast"]}],"models":[{"harness":"claude-code","model":"new-generation","good_at":"small fixes","sources":[{"url":"https://vendor.example/new","date":"2026-10-24"}]}]}).to_string())
        });
        assert_eq!(after.harnesses[0].models[0].good_at.as_deref(), Some("small fixes"));
        assert_eq!(after.harnesses[0].models[1], before.harnesses[0].models[1]);
    }
}
