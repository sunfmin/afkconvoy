//! The watch's weekly **distillation** of the daybook (ADR-0065): a
//! look reads, when one is due (`afkconvoy_core::distill`), what this
//! host's daybook holds of its span (ADR-0118), and a span that holds something is a
//! watch item. The watch keeps only when it last distilled and the last day
//! it covered — never a lesson, a precedent or a proposal: each proposal is
//! its own home's record, and the run's daybook entry maps the lessons to them.

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

/// Where a distillation is due, reads what this host's daybook holds of its
/// span into the look's `daybook`, {since, until, lessons, unlessoned,
/// holds}, or {error} where it could not be read. A span that holds nothing
/// is read again only an hour on.
pub fn look(look: &mut Value, distilling: &mut Distilling, now: u64) {
    let asked = distilling.asked.is_some();
    if !distill::due(distilling.started, asked, distilling.read, now) {
        return;
    }
    let (since, until) = distilling.span(now);
    let read = match since <= until {
        true => match crate::daybook::held(&since, &until) {
            Ok(held) => json!({"since": since, "until": until,
                "lessons": held.lessons, "unlessoned": held.unlessoned,
                "holds": distill::holds(held.lessons, held.unlessoned)}),
            Err(error) => json!({"error": format!("{error:#}")}),
        },
        false => Value::Null,
    };
    if read["holds"] != true {
        distilling.read = Some(now);
        distilling.asked = None;
    }
    look["daybook"] = read;
}

/// The look's distillation item, where the daybook's span holds something.
pub fn items(look: &Value) -> Vec<Item> {
    let read = &look["daybook"];
    let (Some(since), Some(until)) = (read["since"].as_str(), read["until"].as_str()) else {
        return Vec::new();
    };
    if read["holds"] != true {
        return Vec::new();
    }
    let reason = format!(
        "this host's daybook holds {} lessons and {} entries without one from {since} to \
         {until}: distill them into their lasting homes, then record it with `afkconvoy \
         daybook --distilled <file>` (with `--repo <owner/repo>`, the repo it is of, where you \
         run in no checkout)",
        read["lessons"], read["unlessoned"]
    );
    vec![Item::new(distill::item(since, until), reason)]
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
        let taken = [item(&distill::item("2026-10-27", "2026-11-02"))];
        distilling.started(&taken, 10);
        assert_eq!((distilling.started, distilling.asked), (Some(10), None));
        distilling.unreported(9);
        assert_eq!(distilling.started, Some(10));
        distilling.unreported(10);
        assert_eq!(distilling.started, None);
        distilling.reported(&taken, 20);
        assert_eq!(distilling.started, Some(20));
        assert_eq!(distilling.until.as_deref(), Some("2026-11-02"));
        distilling.reported(&[item(&distill::item("2026-10-01", "2026-10-02"))], 30);
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
        let look = json!({"daybook": {"since": "2026-10-27", "until": "2026-11-02",
            "lessons": 2, "unlessoned": 0, "holds": true}});
        let items = items(&look);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "daybook-distill:2026-10-27:2026-11-02");
        assert!(items[0].reason.contains("2 lessons"), "{}", items[0].reason);
        let nothing = json!({"daybook": {"since": "2026-10-27", "until": "2026-11-02",
            "lessons": 0, "unlessoned": 1, "holds": false}});
        assert!(super::items(&nothing).is_empty());
        assert!(super::items(&json!({"daybook": {"error": "locked"}})).is_empty());
        assert!(super::items(&json!({})).is_empty());
    }
}
