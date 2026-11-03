//! The watch's weekly **distillation** of the daybook (control ADR 0008): a
//! look reads, when one is due (`afkconvoy_core::distill`), what each
//! tracker's daybook holds of its span, and a span that holds something is a
//! watch item. The watch keeps only when it last distilled and the last day
//! it covered — never a lesson, a precedent or a proposal: each proposal is
//! its own home's record, and the run's daybook entry maps the lessons to them.

use std::path::PathBuf;

use afkconvoy_core::config::RepoName;
use afkconvoy_core::distill::{self, EVERY};
use afkconvoy_core::host_watch::Item;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::daybook::{day_at, next_day};

/// What the watch keeps of its distillations.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Distilling {
    /// When the last round that took a distillation started.
    pub started: Option<u64>,
    /// The last day the last reported distillation covered.
    pub until: Option<String>,
    /// When the operator asked for one: until a round takes it, or a look
    /// finds its span holds nothing.
    pub asked: Option<u64>,
    /// When a due distillation's span was last read and held nothing.
    pub read: Option<u64>,
}

/// Whether a round of `items` takes a distillation.
pub fn takes(items: &[Item]) -> bool {
    items.iter().any(|item| item.id.starts_with(distill::ITEM))
}

impl Distilling {
    /// A round of `items` started at `started`; one that takes a
    /// distillation reserves the week and takes the operator's ask.
    pub fn started(&mut self, items: &[Item], started: u64) {
        if takes(items) {
            self.started = Some(started);
            self.asked = None;
        }
    }

    /// A round of `items`, started at `started`, reported: the span it
    /// distilled is covered.
    pub fn reported(&mut self, items: &[Item], started: u64) {
        self.started(items, started);
        let until = items.iter().filter_map(|item| distill::until_of(&item.id)).max();
        if let Some(until) = until
            && self.until.as_deref().is_none_or(|covered| covered < until)
        {
            self.until = Some(until.to_string());
        }
    }

    /// The round started at `started` that took a distillation ended
    /// unreported: the week it reserved is free again.
    pub fn unreported(&mut self, started: u64) {
        if self.started == Some(started) {
            self.started = None;
        }
    }

    /// The span a distillation now covers: from the day after the last
    /// one's, or a week back, to yesterday — to today, where asked.
    fn span(&self, now: u64) -> (String, String) {
        let since = match &self.until {
            Some(until) => next_day(until),
            None => day_at(now.saturating_sub(EVERY)),
        };
        let until = match self.asked {
            Some(_) => day_at(now),
            None => day_at(now.saturating_sub(24 * 60 * 60)),
        };
        (since, until)
    }
}

/// Each tracker of this host's checkout list, with a checkout to run `gh`
/// in: the daybooks the host's watch distills.
fn trackers() -> Vec<(RepoName, PathBuf)> {
    let mut trackers: Vec<(RepoName, PathBuf)> = Vec::new();
    let Ok(listed) = crate::checkout_list::List::of_host().and_then(|list| list.read()) else {
        return trackers;
    };
    for entry in listed.checkouts {
        let tracker = match entry.tracker {
            Some(tracker) => Some(tracker),
            None => crate::git::Repo::at(&entry.checkout)
                .and_then(|repo| crate::checkout_list::named_repo(&repo))
                .ok(),
        };
        if let Some(tracker) = tracker.filter(|tracker| trackers.iter().all(|(had, _)| had != tracker)) {
            trackers.push((tracker, entry.checkout));
        }
    }
    trackers
}

/// Where a distillation is due, reads what each tracker's daybook holds of
/// its span into the look's `daybook`, [{tracker, since, until, lessons,
/// unlessoned, holds}], or {tracker, error} where it could not be read. A
/// span that holds nothing anywhere is read again only an hour on.
pub fn look(look: &mut Value, distilling: &mut Distilling, now: u64) {
    let asked = distilling.asked.is_some();
    if !distill::due(distilling.started, asked, distilling.read, now) {
        return;
    }
    let (since, until) = distilling.span(now);
    let mut rows = Vec::new();
    if since <= until {
        for (tracker, dir) in trackers() {
            rows.push(match crate::daybook::held(dir, &tracker, &since, &until) {
                Ok(held) => json!({"tracker": tracker, "since": since, "until": until,
                    "lessons": held.lessons, "unlessoned": held.unlessoned,
                    "holds": distill::holds(held.lessons, held.unlessoned)}),
                Err(error) => json!({"tracker": tracker, "error": format!("{error:#}")}),
            });
        }
    }
    if !rows.iter().any(|row| row["holds"] == true) {
        distilling.read = Some(now);
        distilling.asked = None;
    }
    look["daybook"] = json!(rows);
}

/// The look's distillation items: one for each tracker whose span holds
/// something.
pub fn items(look: &Value) -> Vec<Item> {
    look["daybook"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["holds"] == true)
        .filter_map(|row| {
            let (tracker, since, until) =
                (row["tracker"].as_str()?, row["since"].as_str()?, row["until"].as_str()?);
            let reason = format!(
                "the daybook of {tracker} holds {} lessons and {} entries without one from \
                 {since} to {until}: distill them into their lasting homes, then record it with \
                 `afkconvoy daybook --distilled <file> --tracker {tracker} --repo {tracker}`",
                row["lessons"], row["unlessoned"]
            );
            Some(Item::new(distill::item(tracker, since, until), reason))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> Item {
        Item::new(id, "")
    }

    #[test]
    fn a_reported_round_covers_its_span_and_an_unreported_one_frees_its_week() {
        let mut distilling = Distilling { asked: Some(5), ..Distilling::default() };
        distilling.started(&[item("finding:host:x")], 10);
        assert_eq!((distilling.started, distilling.asked), (None, Some(5)));
        let taken = [item(&distill::item("acme/control", "2026-10-27", "2026-11-02"))];
        distilling.started(&taken, 10);
        assert_eq!((distilling.started, distilling.asked), (Some(10), None));
        distilling.unreported(9);
        assert_eq!(distilling.started, Some(10));
        distilling.unreported(10);
        assert_eq!(distilling.started, None);
        distilling.reported(&taken, 20);
        assert_eq!(distilling.started, Some(20));
        assert_eq!(distilling.until.as_deref(), Some("2026-11-02"));
        distilling.reported(&[item(&distill::item("acme/control", "2026-10-01", "2026-10-02"))], 30);
        assert_eq!(distilling.until.as_deref(), Some("2026-11-02"), "a span never moves back");
    }

    #[test]
    fn the_span_runs_from_after_the_last_to_yesterday_or_today_when_asked() {
        let now = 1_793_700_000;
        let distilling = Distilling::default();
        assert_eq!(distilling.span(now), (day_at(now - EVERY), day_at(now - 24 * 60 * 60)));
        let distilling = Distilling { until: Some("2026-10-31".into()), ..Distilling::default() };
        assert_eq!(distilling.span(now).0, "2026-11-01");
        let asked = Distilling { asked: Some(now), ..distilling };
        assert_eq!(asked.span(now).1, day_at(now));
    }

    #[test]
    fn only_a_span_that_holds_something_is_an_item() {
        let look = json!({"daybook": [
            {"tracker": "acme/control", "since": "2026-10-27", "until": "2026-11-02",
                "lessons": 2, "unlessoned": 0, "holds": true},
            {"tracker": "acme/other", "since": "2026-10-27", "until": "2026-11-02",
                "lessons": 0, "unlessoned": 1, "holds": false},
            {"tracker": "acme/third", "error": "no Daybook category"},
        ]});
        let items = items(&look);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "daybook-distill:acme/control:2026-10-27:2026-11-02");
        assert!(items[0].reason.contains("2 lessons"), "{}", items[0].reason);
        assert!(super::items(&json!({})).is_empty());
    }
}
