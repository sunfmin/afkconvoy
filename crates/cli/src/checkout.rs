//! The checkout a command is about: the one named after it, or with `-C`, or
//! else the one it is run in. It is found once, where the command starts, and
//! handed to everything the command does: nothing else reads the repo from the
//! process, so one process may hold the fleets of many checkouts
//! (docs/adr/0023).

use std::path::{Path, PathBuf};

use afkconvoy_core::line::Line;
use afkconvoy_core::tools::{Found, Tool, fault};
use anyhow::{Context, Result, bail};

use crate::git::Repo;
use crate::orca::Worktree;
use crate::topic::See;

/// The checkout `named` names — a checkout of the repo, or any directory in
/// one, relative to where the command is run — or, with none named, the one
/// it is run in. A worker's worktree is refused unless the command is taken
/// there (`taken`): one a worker runs there, or one that only reads. The
/// fleet cut it for that one worker, and any other command run in it would
/// act for the repo from a worker's branch. On a host with no git, which
/// finds a checkout, the refusal is git's own fault, with the line that
/// installs it: a start names every other command it lacks beside it
/// (`started`).
pub fn resolve(named: Option<&Path>, taken: bool) -> Result<Repo> {
    let dir = dir(named)?;
    if !crate::tools::on_path(Tool::Git.name()) {
        let missing = fault(Tool::Git, &Found::Missing, None, &[]).unwrap_or_default();
        return Err(See::Troubleshooting.refusal(missing.trim()));
    }
    let repo = Repo::at(&dir)?;
    if !taken && Worktree::of(&repo)?.holds_a_worker() {
        let own = repo.main_checkout().map(|own| format!(", such as {}", own.display()));
        bail!(
            "{} is a worker's worktree, where the fleet put one of its workers: only a worker's \
             own commands, and those that only read, are run there. Name another checkout of the \
             repo{}",
            repo.dir.display(),
            own.unwrap_or_default()
        );
    }
    Ok(repo)
}

/// `resolve`, for a start — `run`, `tick` or `plan`. On a host with no git
/// its refusal is a start's first check (`probe::without_git`): git, and
/// every other command the fleet needs that this host lacks, each with the
/// line that installs it, in one refusal.
pub fn started(named: Option<&Path>, taken: bool) -> Result<Repo> {
    match crate::probe::without_git(&dir(named)?) {
        Some(refused) => Err(refused),
        None => resolve(named, taken),
    }
}

/// The directory `named` names, relative to where the command is run, or
/// with none named that one.
pub fn dir(named: Option<&Path>) -> Result<PathBuf> {
    let here = std::env::current_dir().context("this process has no working directory")?;
    Ok(named.map_or_else(|| here.clone(), |named| here.join(named)))
}

/// `line` naming `checkout`, in the form that command takes — a path after
/// it, or `-C` where what follows it is its own — so the line runs from any
/// directory. The form is the one `checkout_help` writes into the manual.
/// A line that is not an afkconvoy command, whose command takes no checkout,
/// `afkconvoy doctor --all`, or one that already names the checkout, is
/// returned as it is. Naming it again changes nothing.
pub(crate) fn about_checkout(checkout: &Path, line: &str) -> String {
    let Some(rest) = line.strip_prefix("afkconvoy ") else {
        return line.to_string();
    };
    let (command, after) = command_of(rest);
    if command.is_empty() {
        return line.to_string();
    }
    // `doctor --all` is the host's look: it is run from anywhere, and names
    // no checkout.
    if command == "doctor" && line_tokens(after).any(|token| token == "--all") {
        return line.to_string();
    }
    let Some(positional) = crate::positional_checkout(command) else {
        return line.to_string();
    };
    if names_checkout(positional, after) {
        return line.to_string();
    }
    let path = quoted(checkout);
    if positional {
        format!("afkconvoy {command} {path}{after}")
    } else {
        format!("afkconvoy {command} -C {path}{after}")
    }
}

/// `text` with each backticked afkconvoy command named for `checkout`. A
/// text with no backtick is named whole when it is one. A comment and a
/// board hold the same lines `plan` and `doctor` print.
pub(crate) fn name_printed(checkout: &Path, text: &str) -> String {
    if !text.contains('`') {
        return about_checkout(checkout, text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let Some(end) = rest.find('`') else {
            out.push('`');
            out.push_str(rest);
            return out;
        };
        let span = &rest[..end];
        out.push('`');
        let trimmed = span.trim();
        if trimmed.starts_with("afkconvoy ") {
            let lead = span.len() - span.trim_start().len();
            let trail = span.len() - span.trim_end().len();
            out.push_str(&span[..lead]);
            out.push_str(&about_checkout(checkout, trimmed));
            out.push_str(&span[span.len() - trail..]);
        } else {
            out.push_str(span);
        }
        out.push('`');
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The command `rest` starts with, and what follows it, including the
/// character that ended the name.
fn command_of(rest: &str) -> (&str, &str) {
    let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '-').unwrap_or(rest.len());
    rest.split_at(end)
}

/// The words of the command's own line. A later line is another command.
fn line_tokens(after: &str) -> impl Iterator<Item = &str> {
    after.lines().next().unwrap_or("").split_whitespace()
}

/// Whether `after` already names the checkout: the next word, where the
/// command takes it there, or `-C` where that flag does.
fn names_checkout(positional: bool, after: &str) -> bool {
    let mut tokens = line_tokens(after);
    if positional {
        tokens.next().is_some_and(|token| !token.starts_with('-'))
    } else {
        tokens.any(|token| token == "-C")
    }
}

/// `checkout` as one word of a shell line, as `Line` quotes every word.
fn quoted(checkout: &Path) -> String {
    let shown = checkout.to_string_lossy();
    Line::new().word(&shown).own()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{about_checkout, name_printed};

    fn named(line: &str) -> String {
        about_checkout(Path::new("/repos/widgets"), line)
    }

    #[test]
    fn a_fix_line_names_the_checkout_as_the_command_takes_it() {
        let path = "'/repos/widgets'";
        assert_eq!(
            named("afkconvoy rule --issue 11 --text-file <file>"),
            format!("afkconvoy rule -C {path} --issue 11 --text-file <file>")
        );
        assert_eq!(
            named("afkconvoy restart --issue 177 --instance felix"),
            format!("afkconvoy restart -C {path} --issue 177 --instance felix")
        );
        assert_eq!(named("afkconvoy run"), format!("afkconvoy run {path}"));
        assert_eq!(
            named("afkconvoy tick --instance alpha"),
            format!("afkconvoy tick {path} --instance alpha")
        );
        assert_eq!(named("afkconvoy tick --help"), format!("afkconvoy tick {path} --help"));
        assert_eq!(
            named("afkconvoy exclusive -- <one step>"),
            format!("afkconvoy exclusive -C {path} -- <one step>")
        );
        assert_eq!(named("afkconvoy flows"), format!("afkconvoy flows {path}"));
        let already = "afkconvoy run /repos/widgets --concurrency 5, then delete `concurrency`";
        assert_eq!(named(already), already);
        let ruled = format!("afkconvoy rule -C {path} --issue 11 --text-file <file>");
        assert_eq!(named(&ruled), ruled);
        assert_eq!(named("afkconvoy kit"), "afkconvoy kit");
        assert_eq!(
            named("afkconvoy kit, or the team's roster: roster.dev"),
            "afkconvoy kit, or the team's roster: roster.dev"
        );
        assert_eq!(named("afkconvoy kit --research <file>"), "afkconvoy kit --research <file>");
        assert_eq!(named("afkconvoy doctor --all"), "afkconvoy doctor --all");
        assert_eq!(named("afkconvoy doctor --all --since 1"), "afkconvoy doctor --all --since 1");
        assert_eq!(named("git -C /repos/widgets status"), "git -C /repos/widgets status");
        assert_eq!(
            named("orca worktree rm --worktree path:/repos/widgets"),
            "orca worktree rm --worktree path:/repos/widgets"
        );
        assert_eq!(
            named("docs/agents/afkconvoy.md: gate_seconds"),
            "docs/agents/afkconvoy.md: gate_seconds"
        );
        assert_eq!(named("gh issue close 7"), "gh issue close 7");
        let odd = about_checkout(Path::new("/it's"), "afkconvoy rule --issue 1");
        assert_eq!(odd, "afkconvoy rule -C '/it'\\''s' --issue 1");
        assert_eq!(about_checkout(Path::new("/it's"), &odd), odd);
        let spaced = about_checkout(Path::new("/my repo"), "afkconvoy run --concurrency 5");
        assert_eq!(spaced, "afkconvoy run '/my repo' --concurrency 5");
        assert_eq!(about_checkout(Path::new("/my repo"), &spaced), spaced);
    }

    #[test]
    fn a_printed_comment_names_the_checkout_inside_each_backtick() {
        let text = "rule on it — `afkconvoy rule --issue 7 --text-file <file>`\n\
                    close it — `gh issue close 7`";
        let printed = name_printed(Path::new("/repos/widgets"), text);
        assert!(
            printed.contains("`afkconvoy rule -C '/repos/widgets' --issue 7 --text-file <file>`")
        );
        assert!(printed.contains("`gh issue close 7`"));
        assert_eq!(name_printed(Path::new("/repos/widgets"), &printed), printed);
    }
}
