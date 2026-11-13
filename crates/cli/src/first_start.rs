//! A first start's questions, shared by the help topic and a `run`'s
//! refusal. Checking the answers reads only the checkout list, before a
//! start writes a kit, labels, settings, logs, or starts a host runner.

use std::fmt::Write;

use anyhow::Result;
use serde::Serialize;

use crate::checkout_list::List;
use crate::git::Repo;
use crate::host_settings;
use crate::instance::Start;

/// What the guide and first-start topic tell the user about host settings,
/// with numeric defaults taken from the rules that use them.
pub fn settings_help() -> String {
    use afkconvoy_core::{admission::SHARES, resources::SPAN};
    format!(
        "Host settings apply to every fleet; `run` changes those named. Defaults: watch workers above half the cores (at least one) for two minutes, a quarter of memory resident, or a tenth of free disk written in ten minutes (at least one GiB); admission holds starts above {}% load per core or below {}% memory or {}% disk free, calling for the watch after {} seconds held for one reason or memory pressure; end exclusive commands after {} seconds holding others up; panel port {} (a free port if taken); the host runner looks every {} seconds in the operator's directory, or the stock watch's by default. No number bounds a checkout's workers: admission alone decides each start, by these readings, the usage windows of the accounts its workers start on, and the exclusive queue, which holds starts once its commands would take over {} seconds to drain. Flags: `afkconvoy run --help`.",
        SHARES.load,
        SHARES.memory,
        SHARES.disk,
        SPAN,
        crate::exclusive::SECONDS,
        host_settings::PANEL_PORT,
        host_settings::WATCH_EVERY,
        afkconvoy_core::admission::QUEUE_BOUND,
    )
}

/// One answer the user can give, and the line that says it. Placeholders
/// are filled from their answer; lines for different settings are combined
/// into one `run` after every required answer has been given.
#[derive(Debug, Serialize)]
pub struct OptionLine {
    option: &'static str,
    line: String,
}

/// A user decision: one home for its question, default, recommendation and
/// all its options. `setting` is the corresponding flag's name.
#[derive(Debug, Serialize)]
pub struct Ask {
    setting: Setting,
    question: &'static str,
    default: String,
    recommend: &'static str,
    options: Vec<OptionLine>,
}

/// A question's setting. Adding one requires deciding whether an unanswered
/// value refuses `run`, as well as supplying its shared question.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Setting {
    Tracker,
    WatchDir,
}

impl std::fmt::Display for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Setting::Tracker => "tracker",
            Setting::WatchDir => "watch-dir",
        })
    }
}

/// Every first-start question. New user decisions belong here.
pub fn questions() -> Vec<Ask> {
    let line = |option, flag: &str| OptionLine {
        option,
        line: format!("afkconvoy run <checkout> {flag}"),
    };
    vec![
        Ask {
            setting: Setting::Tracker,
            question: "Which private repo should hold this checkout's fleet issues?",
            default: "the checkout's own repo, only when private".into(),
            recommend: "Use a private tracker already used by your other checkouts, listed by `afkconvoy checkouts`; otherwise use this repo when private. Two people working one repo need their own trackers.",
            options: vec![
                line("This repo itself, only when private", "--tracker default"),
                line("An existing private repo, starting with the trackers other checkouts use", "--tracker <owner>/<tracker>"),
                OptionLine {
                    option: "A new private repo, created only on the user's yes to its owner and name",
                    line: "gh repo create <owner>/<name> --private && afkconvoy run <checkout> --tracker <owner>/<name>".into(),
                },
            ],
        },
        Ask {
            setting: Setting::WatchDir,
            question: "Which operator directory should a watch start in, or should watches stay off?",
            default: "none; no looks or watches".into(),
            recommend: "Use an existing operator directory holding the rules for looking after this host's projects; leave watches off until the user names one.",
            options: vec![
                line("An existing operator directory chosen by the user", "--watch-dir '<operator-directory>'"),
                line("No watch", "--watch-dir default"),
            ],
        },
    ]
}

/// The question list as Markdown, inserted into the first-start topic.
pub fn help() -> String {
    let mut text = String::new();
    for ask in questions() {
        writeln!(
            text,
            "### {}\n\n{}\n\nDefault: {}. Recommendation: {}\n",
            ask.setting, ask.question, ask.default, ask.recommend
        )
        .unwrap();
        for option in ask.options {
            writeln!(text, "- {}: `{}`", option.option, option.line).unwrap();
        }
        text.push('\n');
    }
    text
}

/// Settles a start's required answers, or refuses with every missing
/// decision together, writing nothing. An explicit default tracker of a new
/// checkout needs no change applied: its startup probe checks privacy after
/// checking all required tools. Existing entries and settings keep their
/// meaning.
pub fn settled(
    checkout: &Repo,
    mut start: Start,
    settings: &host_settings::Named,
) -> Result<Start> {
    let (listed, has_settings) = List::of_host()?.read_with_settings()?;
    let new_checkout = !listed.checkouts.iter().any(|entry| entry.checkout == checkout.dir);
    let tracker = new_checkout && start.tracker.is_none();
    if tracker {
        let asks = questions()
            .into_iter()
            .filter(|ask| match ask.setting {
                Setting::Tracker => tracker,
                Setting::WatchDir => !has_settings && settings.watch_dir.is_none(),
            })
            .collect();
        return Err(Refused { asks }.into());
    }
    if new_checkout && matches!(start.tracker, Some(host_settings::To::Default)) {
        start.tracker = None;
    }
    Ok(start)
}

/// The unanswered decisions carried by a first-start refusal.
#[derive(Debug)]
pub struct Refused {
    pub asks: Vec<Ask>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("settle this first start with the user before `run`: answer each question in `asks`, then combine the chosen flags in one `afkconvoy run <checkout>`")
    }
}

impl std::error::Error for Refused {}
