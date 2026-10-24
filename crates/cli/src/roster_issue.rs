//! A team's roster entry that names no model the host kit holds
//! (`casting::UnmatchedModel`). The fleet never changes a team's roster: a
//! tick opens one issue for it in the team's repo instead, carrying that
//! repo's ready label, so that the team's own fleet corrects the roster.
//! None is opened for an entry while one for it is open.

use afkconvoy_core::casting::UnmatchedModel;
use afkconvoy_core::config::RepoName;
use anyhow::Result;

use crate::gh::GitHub;

/// The ready label of a repo whose config does not say its own.
const READY: &str = "ready-for-agent";

/// The mark that opens the body of the issue opened for `entry`: how one
/// still open for it is found.
fn mark(entry: &UnmatchedModel) -> String {
    format!(
        "<!--convoy:roster-unmatched role={} harness={} model={}-->",
        entry.role,
        entry.harness.key(),
        entry.model
    )
}

/// The title and the body of the issue opened for `entry`.
fn issue(entry: &UnmatchedModel) -> (String, String) {
    let title = format!(
        "Roster: the role `{}` names `{}` on {}, which matches no model a host kit holds",
        entry.role,
        entry.model,
        entry.harness.key()
    );
    let body = format!(
        "{}\nA fleet working a repo bound to this team found {}.\n\nA model is matched by its \
         id or any of its other names on its harness, whatever the case, and with the provider \
         prefix left off where that leaves one match: `afkconvoy check` on that host shows the \
         host kit's models. The fleet never changes a roster.\n\n## Acceptance criteria\n\n- \
         [ ] The role `{}`'s entry in the team's `roster` names `{}` by a name its host kit \
         holds — its id, as `afkconvoy draft` writes it — or names it no more.\n",
        mark(entry),
        entry.line(),
        entry.role,
        entry.harness.key()
    );
    (title, body)
}

/// Opens an issue in `team`, the team's repo, for each of `entries` none is
/// open for → how many it opened. Its config's ready label is put on each:
/// the team's own fleet works it.
pub fn open(gh: &GitHub, team: &RepoName, entries: &[UnmatchedModel]) -> Result<usize> {
    if entries.is_empty() {
        return Ok(0);
    }
    let open = gh.open_issues_of(team)?;
    let ready = match gh.file_of(team, crate::config::FILE)? {
        Some(text) => crate::config::parse(&text).map_or(READY.to_string(), |c| c.ready_label),
        None => READY.to_string(),
    };
    let mut opened = 0;
    for entry in entries {
        let mark = mark(entry);
        if open.iter().any(|(_, body)| body.contains(&mark)) {
            continue;
        }
        let (title, body) = issue(entry);
        gh.open_issue_of(team, &title, &body, &ready)?;
        opened += 1;
    }
    Ok(opened)
}

#[cfg(test)]
mod tests {
    use super::*;
    use afkconvoy_core::harness::Harness;

    #[test]
    fn an_entry_s_issue_names_it_what_the_host_kit_holds_and_the_nearest_names() {
        let entry = UnmatchedModel {
            role: "dev".into(),
            harness: Harness::ClaudeCode,
            model: "opuss".into(),
            holds: "claude-code (claude-opus-5-5)".into(),
            said: "`opuss` names no model the host kit holds for claude-code; the nearest \
                   names: `opus`"
                .into(),
        };
        let (title, body) = issue(&entry);
        assert!(title.contains("`opuss` on claude-code"), "{title}");
        assert!(body.starts_with(&mark(&entry)), "{body}");
        for said in
            ["the nearest names: `opus`", "the host kit holds claude-code (claude-opus-5-5)"]
        {
            assert!(body.contains(said), "{body}");
        }
    }
}
