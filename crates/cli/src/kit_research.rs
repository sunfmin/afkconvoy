//! The watch's sourced research of the host kit's listed models. The whole
//! file is checked before any placement; the kit itself calls no model.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::kit::Kit;

/// Older research settled models without evidence here; they never start.
pub(crate) const UNKNOWN: &str = "not researched";
/// A listed model awaiting the watch's research; it never starts.
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
struct Research {
    models: Vec<Finding>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Finding {
    harness: String,
    model: String,
    good_at: String,
    #[serde(default)]
    names: Vec<String>,
    sources: Vec<Source>,
}

/// Checks the entire research file against the current kit, then places
/// awaiting identities across their harnesses, in file order after established
/// priority. Omitted identities stay as held. Existing research is retained.
/// Refuses an answer placing nothing, with no mutation, like every bad file.
pub fn place(kit: &mut Kit, text: &str, now: u64) -> Result<Vec<serde_json::Value>> {
    let research: Research = serde_json::from_str(text)
        .context("research must be one JSON object with a `models` array")?;
    let mut seen = BTreeSet::new();
    for finding in &research.models {
        let at = format!("{} {}", finding.harness, finding.model);
        let held =
            kit.harnesses.iter().find(|held| held.harness.key() == finding.harness).with_context(
                || format!("research names a harness not in the host kit: {}", finding.harness),
            )?;
        if !held.models.iter().any(|model| model.model == finding.model) {
            bail!("research names a model not listed in the host kit: {at}");
        }
        if !seen.insert(identity(&finding.model)) {
            bail!(
                "research names an identity twice: {at}; name each identity once across harnesses"
            );
        }
        if finding.good_at.trim().is_empty()
            || matches!(finding.good_at.as_str(), PENDING | UNKNOWN)
        {
            bail!("research {at}: good_at must describe strengths and weaknesses for coding work");
        }
        if finding.names.iter().any(|name| name.trim().is_empty()) {
            bail!("research {at}: names must contain only nonempty model names");
        }
        if finding.sources.is_empty() {
            bail!("research {at}: sources must contain a dated source URL");
        }
        for source in &finding.sources {
            let url =
                source.url.strip_prefix("https://").or_else(|| source.url.strip_prefix("http://"));
            if !url.is_some_and(|tail| {
                !tail.split(['/', '?', '#']).next().unwrap_or("").is_empty()
                    && !tail.chars().any(char::is_whitespace)
            }) {
                bail!("research {at}: source URL must be an HTTP or HTTPS URL: {}", source.url);
            }
            if !date(&source.date) {
                bail!(
                    "research {at}: source date must be a calendar date YYYY-MM-DD: {}",
                    source.date
                );
            }
        }
    }
    let mut after = kit.clone();
    let mut placed = vec![];
    for held in &mut after.harnesses {
        for model in &mut held.models {
            if !model.awaits_research() {
                continue;
            }
            if let Some(finding) = research
                .models
                .iter()
                .find(|finding| identity(&finding.model) == identity(&model.model))
            {
                model.good_at = Some(finding.good_at.clone());
                model.sources = finding.sources.clone();
                model.researched_at = Some(now);
                for name in &finding.names {
                    if name != &model.model && !model.names.contains(name) {
                        model.names.push(name.clone());
                    }
                }
                placed.push(serde_json::json!({"harness": held.harness, "model": model.model}));
            }
        }
        held.models.sort_by_key(|model| {
            research
                .models
                .iter()
                .position(|finding| identity(&finding.model) == identity(&model.model))
                .unwrap_or(usize::MAX)
        });
    }
    if placed.is_empty() {
        bail!(
            "research places no model awaiting research in the current host kit; read `afkconvoy doctor --all`, then correct the file and run `afkconvoy kit --research <file>`"
        );
    }
    crate::kit::retain_model_order(kit, &mut after);
    *kit = after;
    Ok(placed)
}

fn date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
    {
        return false;
    }
    let year: u32 = text[..4].parse().unwrap_or(0);
    let month: u32 = text[5..7].parse().unwrap_or(0);
    let day: u32 = text[8..].parse().unwrap_or(0);
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    year != 0 && day > 0 && day <= days
}

/// Unplaced models are host findings, even with no checkout on its list.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(kit) = crate::kit::load() else { return vec![] };
    kit.harnesses
        .iter()
        .filter_map(|held| {
            let unplaced: Vec<&str> = held
                .models
                .iter()
                .filter(|model| !model.placed())
                .map(|model| model.model.as_str())
                .collect();
            if unplaced.is_empty() {
                return None;
            }
            Some(crate::doctor::finding(
                crate::doctor::Kind::KitUnplaced,
                format!("the harness `{}` has unplaced models in the host kit", held.harness.key()),
                format!(
                    "{}; {}; the watch researches models and writes its checked research",
                    unplaced.join(", "),
                    if held.placed().next().is_none() {
                        "workers run its default model"
                    } else {
                        "workers use only its placed models"
                    }
                ),
                "afkconvoy kit --research <file>".into(),
            ))
        })
        .collect()
}
