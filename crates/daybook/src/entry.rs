//! The one shape of an entry, the lesson it carries, and the lesson index of
//! a day's discussion body.

use serde::{Deserialize, Serialize};

/// What the writer says of one entry. Each heading holds what was said for
/// it; one said of nothing is left out of the entry.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Said {
    pub did: Option<String>,
    pub why: Option<String>,
    pub undo: Option<String>,
    pub lesson: Option<String>,
    pub links: Vec<String>,
}

impl Said {
    /// Whether it says nothing under any heading: no entry is made of it.
    pub fn is_empty(&self) -> bool {
        [&self.did, &self.why, &self.undo, &self.lesson].iter().all(|said| said.is_none())
            && self.links.is_empty()
    }
}

/// What was said for a heading, where it says anything.
fn some(said: &Option<String>) -> Option<&str> {
    said.as_deref().filter(|said| !said.trim().is_empty())
}

/// The one shape of an entry: the repo it is of and the kind of session
/// that wrote it — a word its writer chooses — then each heading said.
pub fn entry(repo: &str, session: &str, said: &Said) -> String {
    let mut text = format!("**Repo** `{repo}` · **Session** {session}\n");
    let mut heading = |name: &str, body: &str| {
        text.push_str(&format!("\n### {name}\n\n{}\n", body.trim()));
    };
    if let Some(did) = some(&said.did) {
        heading("Did", did);
    }
    if let Some(why) = some(&said.why) {
        heading("Why", why);
    }
    if let Some(undo) = some(&said.undo) {
        heading("Undo", undo);
    }
    if let Some(lesson) = some(&said.lesson) {
        heading(LESSON, &one_line(lesson));
    }
    let links: Vec<String> = said
        .links
        .iter()
        .filter(|link| !link.trim().is_empty())
        .map(|link| format!("- {}", link.trim()))
        .collect();
    if !links.is_empty() {
        heading("Links", &links.join("\n"));
    }
    text
}

/// The heading a lesson is under.
const LESSON: &str = "Lesson";

/// A lesson is one sentence: its text on one line, however it was said.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The lesson an entry carries, as its `Lesson` heading holds it.
pub fn lesson_of(entry: &str) -> Option<String> {
    let (_, after) = entry.split_once(&format!("\n### {LESSON}\n\n"))?;
    let lesson = after.split("\n### ").next().unwrap_or_default();
    Some(one_line(lesson)).filter(|lesson| !lesson.is_empty())
}

/// Begins the lesson index of a day's discussion body; all after it is
/// the index.
pub(crate) const INDEX: &str = "<!--convoy:lessons-->";

/// One lesson in a day's index: its entry's URL and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    pub url: String,
    pub lesson: String,
}

/// `body` with its lesson index made of `lessons`, all that came before
/// the index kept; with none, no index.
pub fn indexed(body: &str, lessons: &[Indexed]) -> String {
    let kept = body.split(INDEX).next().unwrap_or_default().trim_end();
    if lessons.is_empty() {
        return kept.to_string();
    }
    let mut text = format!("{kept}\n\n{INDEX}\n## Lessons\n\n");
    for Indexed { url, lesson } in lessons {
        text.push_str(&format!("- [entry]({url}): {lesson}\n"));
    }
    text
}

/// The lessons a day's discussion body indexes, in its order.
pub fn index_of(body: &str) -> Vec<Indexed> {
    let Some((_, index)) = body.split_once(INDEX) else { return Vec::new() };
    index
        .lines()
        .filter_map(|line| {
            let (url, lesson) = line.strip_prefix("- [entry](")?.split_once("): ")?;
            Some(Indexed { url: url.to_string(), lesson: lesson.trim().to_string() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(did: &str) -> Said {
        Said { did: Some(did.to_string()), ..Said::default() }
    }

    #[test]
    fn an_entry_says_its_repo_and_session_and_only_the_headings_said() {
        let entry = entry("acme/dev", "watch", &said("installed jq"));
        assert_eq!(entry, "**Repo** `acme/dev` · **Session** watch\n\n### Did\n\ninstalled jq\n");
        let all = Said {
            did: Some("ruled".into()),
            why: Some("asked".into()),
            undo: Some("rule again".into()),
            lesson: None,
            links: vec!["https://x/1".into(), "https://x/2".into()],
        };
        let entry = super::entry("acme/dev", "worker", &all);
        assert!(entry.contains("### Did\n\nruled\n\n### Why\n\nasked\n\n### Undo\n\nrule again\n"));
        assert!(entry.ends_with("### Links\n\n- https://x/1\n- https://x/2\n"), "{entry}");
        let lesson = Said { lesson: Some("the gate\n needs  jq".into()), ..all.clone() };
        let entry = super::entry("acme/dev", "worker", &lesson);
        assert!(
            entry
                .contains("### Undo\n\nrule again\n\n### Lesson\n\nthe gate needs jq\n\n### Links")
        );
        assert_eq!(lesson_of(&entry).as_deref(), Some("the gate needs jq"));
        assert_eq!(lesson_of(&super::entry("acme/dev", "worker", &all)), None);
        let blank = Said { why: Some("  ".into()), ..said("x") };
        assert!(!super::entry("acme/dev", "interactive", &blank).contains("Why"));
        assert!(Said::default().is_empty() && !blank.is_empty());
    }

    #[test]
    fn a_body_s_index_is_rebuilt_whole_and_read_back() {
        let intro = "What agents did on 2026-11-03.";
        let one = Indexed { url: "https://x#c-1".into(), lesson: "a: b".into() };
        let two = Indexed { url: "https://x#c-2".into(), lesson: "c".into() };
        let body = indexed(intro, std::slice::from_ref(&one));
        assert_eq!(index_of(intro), vec![]);
        assert_eq!(index_of(&body), vec![one.clone()]);
        let again = indexed(&body, &[one.clone(), two.clone()]);
        assert!(again.starts_with(intro) && again.matches(INDEX).count() == 1, "{again}");
        assert_eq!(index_of(&again), vec![one, two]);
        assert_eq!(indexed(&again, &[]), intro);
    }
}
