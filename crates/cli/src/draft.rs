//! `afkconvoy draft`: a team's roster drafted from this host's host kit, into
//! the team file of the team's working tree (docs/adr/0020). The host kit's
//! first model says how each role is
//! to be cast among what it may run; the rest is the host kit's, in its
//! order. It commits nothing: a human reads the draft, edits it and commits
//! it. A roster there already is replaced only once its human confirms.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use afkconvoy_core::config::RepoDir;
use afkconvoy_core::roster::{Guidance, Strength, draft_prompt, unsaid};
use afkconvoy_core::team::{ROSTER_KEY, TEAM_FILE};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::team::From;

/// How long the drafting model is given to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(540);

/// Drafts the roster of the team whose files are in `dir` — an edit of the
/// team `team` on GitHub, where it is said — and writes it into its team file,
/// in place of the roster there: where there is one, only once `yes` or a
/// human at the terminal confirms what it would change.
///
///   {"path", "drafted_by": {harness, model}, "roster", "replaces", "removed",
///    "added", "unsaid", "written"}
pub fn draft(dir: PathBuf, team: Option<RepoDir>, yes: bool) -> Result<Value> {
    let file = dir.join(TEAM_FILE);
    // The labels are those of the checkout the team's working tree is in, where
    // it is in one: the team's own repo is about no other.
    let checkout = crate::git::Repo::at(&dir).ok();
    let loaded = crate::team::load(From::Dir(dir.clone(), team), checkout.as_ref())?;
    let (kit, _) = crate::kit::running()?;
    let range = kit.range();
    let first = kit.first().context("the host kit holds no harness: run `afkconvoy kit`")?;
    let launch = kit.command(first.harness).context("the host kit holds no harness")?;
    let kept = crate::kept::keep(first.harness, false)?.display().to_string();
    let strengths: Vec<Strength> = kit
        .harnesses
        .iter()
        .flat_map(|held| {
            held.models.iter().map(|seen| Strength {
                harness: held.harness.key().into(),
                model: Some(seen.model.clone()),
                good_at: seen.good_at.clone().unwrap_or_else(|| "not researched".into()),
            })
        })
        .collect();
    let prompt = format!(
        "{}\nHost kit research (use as given): {:?}",
        draft_prompt(&loaded.team, &range),
        strengths
    );
    let question = first.harness.prompt_question(launch, &kept, first.model.as_deref(), &prompt);
    // Asked in the team's working tree, which is what it drafts for.
    let asked = crate::launch::ask_login_shell_within(&dir, &question, ANSWER_TIMEOUT);
    let Some((true, answer)) = asked else {
        bail!("the host kit's first, `{}`, did not answer the draft", first.harness.key());
    };
    let guidance = read_answer(&answer);
    let roster = afkconvoy_core::roster::draft(&loaded.team, &range, &strengths, &guidance);
    let unsaid = unsaid(&loaded.team, &range, &strengths, &guidance);
    for said in &unsaid {
        eprintln!("warning: the draft says nothing of {said}: write it in by hand");
    }

    let markdown = std::fs::read_to_string(&file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    let (written_out, had) = with_roster(&markdown, &roster)
        .with_context(|| format!("{} has no ```yaml block", file.display()))?;
    let drafted: Vec<&str> = roster.lines().collect();
    let removed: Vec<&str> = had.iter().copied().filter(|line| !drafted.contains(line)).collect();
    let added: Vec<&str> = drafted.iter().copied().filter(|line| !had.contains(line)).collect();
    let replaces = !had.is_empty();
    let mut said = json!({
        "path": file,
        "drafted_by": {"harness": first.harness, "model": first.model},
        "roster": roster,
        "replaces": replaces,
        "removed": removed,
        "added": added,
        "unsaid": unsaid,
        "written": false,
    });
    if replaces && removed.is_empty() && added.is_empty() {
        return Ok(said);
    }
    if replaces && !yes {
        let mut out = std::io::stderr();
        writeln!(out, "{} holds a roster: the draft would change it so", file.display())?;
        for line in &removed {
            writeln!(out, "- {line}")?;
        }
        for line in &added {
            writeln!(out, "+ {line}")?;
        }
        if !confirmed(&mut out)? {
            return Ok(said);
        }
    }
    std::fs::write(&file, written_out)
        .with_context(|| format!("cannot write {}", file.display()))?;
    said["written"] = json!(true);
    Ok(said)
}

/// Whether the human at the terminal confirms writing the draft: never with
/// no terminal to ask at.
fn confirmed(out: &mut impl Write) -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        writeln!(out, "nothing written: confirm at a terminal, or with --yes")?;
        return Ok(false);
    }
    write!(out, "Write this roster into {TEAM_FILE}? [y/N] ")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Reads only role guidance. Model abilities always come from the host kit,
/// even if a drafting model volunteers its own assessment.
fn read_answer(answer: &str) -> Vec<Guidance> {
    answer
        .lines()
        .filter_map(|line| serde_json::from_str(line.trim().trim_matches('`').trim()).ok())
        .collect()
}

/// Whether `line` of a yaml block is its top-level `roster` key.
fn is_roster_key(line: &str) -> bool {
    line.strip_prefix(ROSTER_KEY)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// `markdown`, a team file, with `roster` in place of the `roster` key of
/// its first yaml block and everything under it — or at the block's end,
/// where it has none — and the lines that roster had, none where there was
/// none. A line in the block counts as under the key while it is indented or
/// empty, and the empty lines before the next key stay. `None`: it has no
/// yaml block.
fn with_roster<'m>(markdown: &'m str, roster: &'m str) -> Option<(String, Vec<&'m str>)> {
    enum At {
        Above,
        In,
        Below,
    }
    let mut at = At::Above;
    let mut out: Vec<&str> = vec![];
    let mut had: Vec<&'m str> = vec![];
    let mut under = false;
    let mut blank: Vec<&'m str> = vec![];
    let mut placed = false;
    for line in markdown.lines() {
        match at {
            At::Above => {
                out.push(line);
                if line.trim_end() == "```yaml" {
                    at = At::In;
                }
            }
            At::In if under && line.trim().is_empty() => blank.push(line),
            At::In if under && line.starts_with([' ', '\t']) => {
                had.append(&mut blank);
                had.push(line);
            }
            At::In => {
                if under {
                    under = false;
                    out.append(&mut blank);
                }
                if line.trim_end() == "```" {
                    if !placed {
                        out.extend(roster.lines());
                    }
                    out.push(line);
                    at = At::Below;
                } else if is_roster_key(line) && !placed {
                    had.push(line);
                    out.extend(roster.lines());
                    under = true;
                    placed = true;
                } else {
                    out.push(line);
                }
            }
            At::Below => out.push(line),
        }
    }
    if !matches!(at, At::Below) {
        return None;
    }
    let mut written = out.join("\n");
    if markdown.ends_with('\n') {
        written.push('\n');
    }
    Some((written, had))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROSTER: &str = "roster:\n  dev:\n    run:\n      - { harness: pi }\n";

    #[test]
    fn a_roster_takes_the_place_of_the_one_there_and_of_nothing_else() {
        let markdown = "A team.\n\n```yaml\nroles:\n  dev: acme/dev\nroster:\n  dev:\n    run: \
                        [{ harness: codex }]\n\n  # a note\n\nsplit: false\n```\n\nBelow.\n";
        let (written, had) = with_roster(markdown, ROSTER).unwrap();
        assert_eq!(
            written,
            "A team.\n\n```yaml\nroles:\n  dev: acme/dev\nroster:\n  dev:\n    run:\n      - { \
             harness: pi }\n\nsplit: false\n```\n\nBelow.\n"
        );
        assert_eq!(had, ["roster:", "  dev:", "    run: [{ harness: codex }]", "", "  # a note"]);
        // A roster written on its one line is the same.
        let (written, had) = with_roster("```yaml\nroster: {}\n```", ROSTER).unwrap();
        assert_eq!(written, format!("```yaml\n{ROSTER}```"));
        assert_eq!(had, ["roster: {}"]);
    }

    #[test]
    fn a_team_file_with_no_roster_has_one_put_at_its_block_s_end() {
        let markdown =
            "```yaml\nroles:\n  dev: acme/dev\nrosters: x\n```\n```yaml\nroster: {}\n```\n";
        let (written, had) = with_roster(markdown, ROSTER).unwrap();
        assert_eq!(
            written,
            format!(
                "```yaml\nroles:\n  dev: acme/dev\nrosters: x\n{ROSTER}```\n```yaml\nroster: {{}}\n```\n"
            )
        );
        assert!(had.is_empty());
        assert_eq!(with_roster("no block\n", ROSTER), None);
        assert_eq!(with_roster("```yaml\nroles: {}\n", ROSTER), None);
    }

    #[test]
    fn only_role_guidance_is_read_from_the_drafting_answer() {
        let answer = "Here:\n```\n{\"harness\": \"pi\", \"model\": \"a/b\", \"good_at\": \"x\"}\n\
                      {\"harness\": \"dsh\", \"good_at\": \"y\"}\nnoise\n{\"role\": \"dev\", \
                      \"cast\": \"z\"}\n```";
        let guidance = read_answer(answer);
        assert_eq!(guidance, [Guidance { role: "dev".into(), cast: "z".into() }]);
    }
}
