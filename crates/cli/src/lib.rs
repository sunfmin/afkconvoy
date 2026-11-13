//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::{RepoDir, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_watch::WATCH_ENV;
use afkconvoy_core::line::Line;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::position::Declared;
use afkconvoy_core::role::{
    CONSULT_ENV, HARNESS_ENV, ISSUE_ENV, MODEL_ENV, ROLE_COMMIT_ENV, ROLE_ENV, STAGE_ENV,
    TEAM_COMMIT_ENV,
};

use anyhow::{Context, bail};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::{Value, json};

pub mod account;
pub mod accounts;
pub mod acting;
pub mod admission;
pub mod advance;
pub mod agent_setup;
mod asking_report;
mod asking_terminal;
pub mod blockers;
pub mod board;
mod candidates;
pub mod caster;
pub mod checkout;
pub mod checkout_list;
pub mod checkouts;
pub mod claim_status;
pub mod claims;
mod codex_launch;
pub mod concurrency;
pub mod config;
pub mod copies;
pub mod cut;
pub mod daybook;
mod delivery;
pub mod dispatch;
pub mod distill;
pub mod doctor;
pub mod draft;
pub mod evidence;
pub mod exclusive;
pub mod first_start;
pub mod fleet;
pub mod fleet_report;
pub mod flows;
mod forge;
pub mod forget;
pub mod found;
pub mod gate;
pub mod gh;
mod gh_equivalent;
mod gh_quota;
pub mod git;
mod goal;
pub mod guard;
pub mod harness_down;
pub mod health;
pub mod host_asking;
pub mod host_evidence;
pub mod host_panel;
pub mod host_runner;
pub mod host_settings;
pub mod host_watch;
pub mod hot;
pub mod human_wait;
pub mod instance;
pub mod intake;
pub mod issue_updates;
pub mod jev;
pub mod judge;
pub mod kept;
mod kit;
mod kit_research;
pub mod landing;
pub mod launch;
pub mod legacy;
pub mod lock;
pub mod log;
pub mod look;
pub mod machine;
pub mod measure;
pub mod named_harness;
pub mod orca;
pub mod panel;
pub mod park;
pub mod patrol;
mod patrol_usage;
pub mod paused;
mod plan;
pub mod probe;
mod process;
mod process_scope;
pub mod published;
pub mod queue;
pub mod recovery;
pub mod remote;
mod repair;
pub mod resources;
pub mod rewrite_accounts;
pub mod role;
pub mod roster_issue;
pub mod run_log;
pub mod runner_socket;
pub mod scaffold;
pub mod serve;
pub mod session;
pub mod settle;
mod skill;
mod slots;
pub mod slow_gate;
pub mod stage;
pub mod standings;
pub mod steer;
pub mod stop;
pub mod stop_release;
pub mod switch;
pub mod takeover;
mod target;
pub mod team;
pub mod tell;
pub mod tick;
pub mod tmux;
pub mod tools;
pub mod topic;
pub mod tracker;
mod transient;
mod turn;
mod turn_answer;
mod unbound_issue;
mod unworked;
pub mod wake;
pub mod watch;
mod watch_daybook;
pub mod watch_landing;
mod watch_notify;
mod watch_report;
mod watch_terminal;
pub mod worker_tmp;
pub mod workflow;

use acting::Acting;
use claims::{Me, Scan, Take};
use dispatch::Agent;
use fleet::Fleet;
use git::Repo;
use host_settings::{
    ADMIT_DISK_ENV, ADMIT_INSPECT_AFTER_ENV, ADMIT_LOAD_ENV, ADMIT_MEMORY_ENV, AGENT_GUARDS_ENV,
    AGENT_INSTRUCTIONS_ENV, AGENT_SKILLS_ENV, EXCLUSIVE_SECONDS_ENV, HOST_NAME_ENV, HOT_CORES_ENV,
    HOT_MEMORY_ENV, HOT_WRITTEN_ENV, JEV_KEY_COMMAND_ENV, NOTIFY_COMMAND_ENV, To, WATCH_DIR_ENV,
    WATCH_EVERY_ENV, jev_key_command_or_default, notify_command_or_default, port_or_default,
    thousandths_or_default, watch_dir_or_default, watch_every_or_default, whole_or_default,
};
use launch::Launches;

/// What this binary was built from, as `--version` says it: stamped by
/// `build.rs`.
pub const VERSION: &str = env!("AFKCONVOY_VERSION");

agent_help::binary!("afkconvoy");

/// The groups the manual puts the commands in, each named for who runs it:
/// each command declares its own, and the guide describes each by its name.
const OPERATOR: &str = "Run by the operator";
const DESIGNER: &str = "Run by whoever designs a team";
const HANDS_WORK: &str = "Run by whoever hands the fleet work";
const SETS_UP: &str = "Run by whoever sets an agent up";
const WORKER: &str = "Run by a worker";
const HUMAN: &str = "Run by a human an issue waits for";
const HOOK: &str = "Run by a worker's hook";
const BY_HAND: &str = "One transition by hand";
const PLUMBING: &str = "The claim plumbing";
const READER: &str = "Run by whoever reads this manual";

// `-h` is clap's one-screen listing, each subcommand by its one line; `--help`
// is the index of the manual (`manual`), which opens with the doc comment
// below, the guide: the text an agent reads to learn the tool. `help` is this
// crate's own, not clap's: it prints a topic too, and `help all` the whole
// manual. The listing opens with usage, then commands, so `-h` stays one screen.
/// afkconvoy works ready GitHub issues unattended. `afkconvoy run` starts one host runner with a fleet instance per repo. Each dispatches a coding agent per issue in its own git worktree, gates finished PRs and lands them on the configured base or a task's selected target. Fleet state lives in GitHub and the remote.
///
/// `afkconvoy --help` answers the index: this guide, each command by who runs it, and each topic's first sentence. `afkconvoy help all` answers the whole manual. `afkconvoy -h` lists commands on one screen.
///
/// ## What it needs
///
/// - A config on the base branch: the first yaml block of {const:CONFIG_FILE}. A repo whose config names a team is a bound repo: its workflows say how issues are worked. One whose config names none is unbound: one worker per issue, then a turn that lands its PR.
/// - A private tracker for the issues the fleet reads and writes. Only a private repo can be its own tracker: anyone who can comment writes what workers read. Settle it with the user: [`afkconvoy help first-start`](help:first-start).
/// - {const:FIRST_START_SETTINGS}
/// - `gh` logged in as a user who may push to the repo.
/// - `orca` on the PATH, or Orca's SSH remote runtime as is, its CLI at `~/.orca/cli/bin/orca`: it starts every worker. It is running (`orca open`) and was given this checkout, once (`orca repo add --path "$PWD"`).
/// - `claude` on the PATH and logged in, when it is what starts a worker.
/// - `pi` there too, holding credentials for a provider (`/login` at its prompt), or `codex`, logged in (`codex login`): each when a stage or a patrol is on it, only then.
///
/// `plan`, `tick` and `run` check these first. A missing command, login or checkout registration refuses with a fix for each; nothing is installed, started or added for you.
///
/// ## What to run first, in a checkout of the repo
///
/// 1. `afkconvoy checkouts`: if this checkout is absent, read [`afkconvoy help first-start`](help:first-start) and settle the first start with the user before `run`.
/// 2. `afkconvoy config`: read the config, what the fleet goes by. Its `--help` shows one whole. Then `afkconvoy check`, only in a bound repo: does the team load? A team is designed first: see designing a team, below.
/// 3. `afkconvoy plan`: what a tick would act on. It changes no fleet state, but may push and delete a startup probe on the remote. To read without writing anything, run `afkconvoy doctor`.
/// 4. `afkconvoy run`: the checkout's fleet instance, held by the host runner until it is stopped. It answers once the first tick ended.
///
/// To watch a running fleet, checking on it every few minutes, run `afkconvoy doctor --all --since <until of your last look>`. A first look runs `afkconvoy doctor --all` without `--since`; its `until` is where the next look starts.
///
/// Every host watches itself: in the operator's directory where `afkconvoy run --watch-dir <dir>` names one, otherwise as the stock watch in afkconvoy's own directory. Its host runner takes that look every five minutes, with no model, and when it holds something an operator would act on, starts one watch, continued on another harness where its own gives out. `afkconvoy run --help` says when, and `doctor` says the watch running and what the last one did.
///
/// A watch is the operator for every repo on the host's checkout list. Once a fleet fault is fixed it hands the escalated issue back with the line `doctor` names and checks the next tick dispatches it. It may change any listed repo in a checkout of its own, land it in that repo's turn with `land --watch`. Where the fix must reach an installed tool, it has that repo's fleet cut a version after landing and checks the fault on the new version. Human decisions stay with the human, untouched and listed. A separate resident asking session asks each recorded decision, one at a time, in a Terminal named "afkconvoy asks"; Orca's phone push opens it, and the operator answers there. It acts only on what the fleets own, preserves what cannot be rebuilt and changes no base branch outside a landing turn. Its watch rounds together have one hour; the asking session has no round or hour of its own.
///
/// After rules, Jev judges screens sent to TypeSafe only with `run --jev-key-command`. Without a key or settled answer, workers go to the operator and watch: `help roster`.
///
/// ## What a command answers
///
/// Every command prints exactly one JSON object on stdout. A failure is {"error": …} with a non-zero exit — and "see" beside it, where a topic explains it: the command that prints that topic. Warnings go to stderr. A few print otherwise: `show` a drawing unless asked about an issue, `help` Markdown as `--help` does, `skill` the text of a file, `guard` answers as a hook does, with its exit code, `exclusive` as its command does, `accounts --rows` NDJSON, and `panel` without `--open` one HTML document. `panel --open` is the JSON object `panel --help` describes. `doctor` exits 0 with no critical finding, and non-zero with one, printing the same object, not {"error"}. Under `--all`, a checkout that could not be asked stays {"error", "see"}, as `doctor --help` says: not a doctor that read the checkout. A fleet instance prints nothing: it logs a line as each tick starts and one as it ends, in its tick log.
///
/// ## The commands, by who runs them
///
/// Read `afkconvoy <command> --help` before first running it. Commands are grouped by who runs them:
///
/// - **{const:OPERATOR}** — `config`, `check`, `plan` before `run`; `target` selects a task's destination; `delivery` for a Jira delivery; `tick` for one pass, `stop` to stop; `queue` for the local exclusive queue; `doctor --all --since <until of your last look>`, `checkouts`, `panel` to watch (`doctor` names each fix); `accounts` reads work accounts, `rewrite-accounts` converts old ones; `kit`, `cast`, `switch` for workers' harnesses and models; `patrol-now` runs a patrol; `tell` tells a worker something it must know; `takeover` takes another fleet instance's claims; `daybook` records, after the fact, what an agent did outside any issue.
/// - **{const:DESIGNER}** — `scaffold` to start one or a role of it from a skeleton, `draft` to draft its roster from the host kit, then, once `check` accepts it, `show` to review a workflow or to ask where an issue is in it, and `rehearse` to try one out.
/// - **{const:HANDS_WORK}** — a human or their agent opening decided work. `flows` names the workflow, gives the issue-create line and makes labels. After opening issues, `wake` has the fleet take them up now.
/// - **{const:SETS_UP}** — `skill` prints the skill file that sends their agent to `afkconvoy --help`.
/// - **{const:WORKER}** — `gate`, `land`, `verdict`, `intake`, `lock`, `over`, `exclusive`, `watched`, `asked`: its prompt names each one as it is needed, and `wake`, above, as its last; `cut` is what a cut's terminal runs, and what you run to go on with a cut that failed. Read them only when you are a worker, or are finding out why one stopped (`rule` is a consult's too).
/// - **{const:HUMAN}** — in a bound repo: `rule` to answer an escalated issue, `send` to put an issue at a stage of your choosing. A `by: human` stage waits for your `verdict`, a worker's command too. [`afkconvoy help humans`](help:humans) says what each case asks of you.
/// - **{const:HOOK}** — `guard`, called before each action of a bound repo's worker. Nobody runs it by hand. Its help explains what the worker is refused and why.
/// - **{const:BY_HAND}** — `dispatch`, `close`, `escalate`, `park`, `nudge` and `restart`: each is something a tick does on its own, and `recovery` shows what `dispatch` would continue an issue from. Read one when a plan shows a transition the fleet is not making, or you are told to make it yourself.
/// - **{const:PLUMBING}** — `scan`, `claim`, `release` and `heartbeat`: what the transitions are built on. Read these only to inspect or repair claims and heartbeats on the remote.
///
/// ## The topics
///
/// Read `afkconvoy help <topic>` when this guide or a command's help names it: it prints that topic, Markdown, as `--help` does.
///
/// ## Rules to know before you start
///
/// - A flag of the command's own, including `--instance` and `--run`, does not move the checkout to `-C`. The checkout is a path after the command whenever that command's usage line shows `[CHECKOUT]`, and `-C <checkout>` only when that command's usage line shows `-C`. A fix line and a hand-typed command use that form.
/// - One fleet instance per host per repo. `run` in a second checkout of a repo on the host's checkout list is refused, naming the first: run it there.
/// - `run` returns once its checkout's first tick ended: the host runner ticks on, in the background, until `afkconvoy stop --all` stops it, keeping the checkout list. `afkconvoy stop` takes one checkout off the list. `tick` is one pass that exits.
/// - Between ticks a fleet instance sleeps {const:TICK_BUSY_SECONDS} seconds while it has work, or {const:TICK_IDLE_MINUTES} minutes after {const:TICK_IDLE_AFTER} empty ticks. Commands that change fleet state wake it sooner, including a worker's last `afkconvoy wake`. When a fleet on this host closes an issue or sees it closed, it also wakes every other fleet on the same tracker whose open issues it blocked. That next tick reads GitHub and takes up an issue whose last open blocker closed; its tick log names the closed issue and the fleet instance that saw it. After changing issues with `gh`, run `afkconvoy wake <checkout>` on the fleet's host: `gh` sends no wake.
/// - Each harness's worker launch command comes from this host's host kit, kept outside the repo. `run` writes it on a host that has none, and `afkconvoy kit` writes it again; neither asks anything. `--worker-command` is gone. A known harness with no entry or command is discovered in the worker login shell, checked, and appended to the latest kit without model probes; an explicit command always wins. A running fleet reads command edits at its next worker launch.
/// - In a bound repo the worker launch command must end in its harness — Claude Code, pi, Codex, DSH, Kimi Code or Grok — taking flags: the fleet adds to it what starts each worker in its role and under the guard ([`afkconvoy help worker-launch`](help:worker-launch) says what, for each harness), and a start refuses a command whose `--help` does not name those flags. Every harness the team could ask for is checked first, and none is ever started in another's place.
///
/// ## Designing a team, for a bound repo
///
/// A bound repo names its team in its config: `team: owner/repo`, or `team: owner/repo/path` for a team in a subdirectory. A flow label walks an issue through that workflow's stages, each by a new worker in the stage's role. A team and a role are each a directory of text files, and designing one is editing them. Nothing else holds any of it.
///
/// What those files are, and every key of them, is three topics: [`afkconvoy help team`](help:team) (a team's directory and a role's), [`afkconvoy help workflows`](help:workflows), [`afkconvoy help patrols`](help:patrols).
///
/// The loop, from nothing:
///
/// 1. `afkconvoy scaffold team <dir> --role <name> --repo <owner/repo>`: a team's skeleton and, under roles/<name>/, each role's — `--role` once for each — in one repo created on GitHub, with one workflow of one stage, by the first role. For more than one stage — a review, a landing role — read [`afkconvoy help example`](help:example) first: `--example hospital`.
/// 2. `afkconvoy scaffold role <dir>/roles/<name>`: one more role's skeleton, in that checkout: it answers the line to add to {const:TEAM_FILE}. A role several teams share is a repo of its own instead: `--repo`.
/// 3. `afkconvoy check --dir <dir> --team <owner/repo>`: `afkconvoy draft` with the same flags first drafts the roster into {const:TEAM_FILE}. Edit a file, run it, and read the faults: each names its file and key. Repeat until it exits 0. `check` reads a role from GitHub only: a role's edit counts once it is pushed to its default branch.
/// 4. Push the team, name it under `team` in the config of the repo it is to work, and run `afkconvoy check` in that checkout. Then `show` draws each workflow and `rehearse` tries one out.
///
/// A role's skill is a file you write ([`afkconvoy help skill-file`](help:skill-file) says its form), or one added with the skills installer you already use, run in the role's directory and committed there. The fleet installs no skill and wraps no installer.
#[derive(Parser)]
#[command(name = "afkconvoy", version = VERSION, about, verbatim_doc_comment, disable_help_subcommand = true,
    help_template = "{before-help}{usage-heading} {usage}\n{all-args}{after-help}")]
struct Cli {
    /// The time to act at, in epoch seconds [default: the clock's]
    #[arg(long, global = true, hide = true)]
    now: Option<u64>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report completion times from this repo's issues' work accounts
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = accounts::HELP)]
    Accounts {
        /// Account ends in the last N days (positive; default 7); conflicts with --all
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=u64::MAX / 86400), conflicts_with = "all")]
        days: Option<u64>,
        /// Include all recorded account ends through now
        #[arg(long)]
        all: bool,
        /// Answer NDJSON: one versioned record per account; unreadable diagnostics on stderr
        #[arg(long)]
        rows: bool,
        #[command(flatten)]
        at: At,
    },
    /// Convert the tracker’s legacy work accounts into versioned records
    #[command(
        next_help_heading = OPERATOR,
        verbatim_doc_comment,
        long_about = rewrite_accounts::HELP,
    )]
    RewriteAccounts {
        /// Save original bodies in this directory and edit validated comments
        #[arg(long)]
        backup: Option<std::path::PathBuf>,
        #[command(flatten)]
        at: At,
    },
    /// Write this host's host kit. `kit --cast` is refused: automatic casting is gone
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = kit::HELP)]
    Kit {
        /// Refused: automatic casting is gone. An old kit `cast` key is ignored
        #[arg(long, value_parser = ["on", "off"], conflicts_with_all = ["harness", "research"])]
        cast: Option<String>,
        /// Place or settle the watch's sourced model research from a checked JSON file
        #[arg(long, conflicts_with = "harness")]
        research: Option<PathBuf>,
        /// Refresh only this harness after a switch has answered
        #[arg(long, hide = true, value_parser = kit_harness)]
        harness: Option<Harness>,
        /// Take this harness out of the host kit until switch puts it back
        #[arg(long, value_parser = kit_harness, conflicts_with_all = ["harness", "research", "cast"])]
        remove: Option<Harness>,
    },
    /// A worker's `model:` label or the user default sets its harness model; automatic casting is gone
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = caster::HELP)]
    Cast {
        #[command(flatten)]
        at: At,
    },
    /// Print the repo's config as JSON, defaults filled
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = config::HELP)]
    Config {
        #[arg(long, help = format!("The config file [default: {} of this checkout]", config::FILE))]
        file: Option<PathBuf>,
        #[command(flatten)]
        at: At,
    },
    /// Read a team and its workflows: what was read, or every fault
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = team::HELP)]
    Check {
        #[command(flatten)]
        team: TeamArgs,
        #[command(flatten)]
        at: At,
    },
    /// Write the skeleton of a team with its roles, or of one role, or a whole example team, and create its repo on GitHub if asked
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment, long_about = scaffold::HELP)]
    Scaffold {
        /// What to write the skeleton of
        #[arg(value_enum)]
        what: scaffold::What,
        /// The directory to write it in: one not there yet, or empty
        dir: PathBuf,
        /// A role of the team, once for each: <name> for one written here, under roles/<name>,
        /// or <name>=<owner/repo[/path]> for one that is elsewhere. The example workflow is
        /// performed by the first
        #[arg(long, value_parser = scaffold::named)]
        role: Vec<scaffold::Named>,
        /// Of a team: write this example whole — a team the binary holds, with its roles —
        /// in place of a skeleton. It takes no --role. `afkconvoy help example` reads the
        /// team with you
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(scaffold::example_names()))]
        example: Option<String>,
        /// Also commit the skeleton and create this repo on GitHub from it, as owner/repo
        #[arg(long, value_parser = repo)]
        repo: Option<RepoName>,
        /// Create that repo public [default: private]
        #[arg(long, requires = "repo")]
        public: bool,
    },
    /// Say how to open an issue the fleet will work, and create the labels that takes
    #[command(next_help_heading = HANDS_WORK, verbatim_doc_comment, long_about = flows::HELP)]
    Flows {
        /// Existing Jira delivery identity; opens tasks without a runnable label, then attaches through intake
        #[arg(long)]
        delivery: Option<String>,
        /// The tracker, where this host's checkout list holds no entry of the checkout: the repo its fleet reads its issues in, the checkout's own where that is its tracker
        #[arg(long, value_name = "OWNER/REPO", value_parser = tracker::repo_named)]
        tracker: Option<RepoName>,
        #[command(flatten)]
        at: At,
    },
    /// End the sleep of this host's `run` for this repo: its next tick starts now (after opening issues with `gh`, and as a worker's last command)
    #[command(next_help_heading = HANDS_WORK, verbatim_doc_comment, long_about = wake::HELP)]
    Wake {
        #[command(flatten)]
        at: At,
    },
    /// Append one after-the-fact entry to the daybook: what an agent did or decided outside any issue
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = daybook::HELP)]
    Daybook {
        /// What was done, put under `Did`; or say each heading with its flag
        #[arg(conflicts_with = "did")]
        text: Option<String>,
        /// What was done
        #[arg(long)]
        did: Option<String>,
        /// Why it was done
        #[arg(long)]
        why: Option<String>,
        /// How to undo it
        #[arg(long)]
        undo: Option<String>,
        /// One sentence that should outlast the day: raw material for the weekly distillation
        #[arg(long, value_name = "SENTENCE")]
        lesson: Option<String>,
        /// A link to what it touched: an issue, a PR, a commit; give it again for more
        #[arg(long = "link", value_name = "URL")]
        links: Vec<String>,
        /// List the lessons of a span of days instead of writing an entry
        #[arg(long, requires = "since", conflicts_with_all = ["text", "did", "why", "undo", "lesson", "links"])]
        lessons: bool,
        /// Record a distillation said in this JSON file: check it carries each lesson of its span once, then write its one entry
        #[arg(long, value_name = "FILE", conflicts_with_all = ["text", "did", "why", "undo", "lesson", "links", "lessons", "distill"])]
        distilled: Option<PathBuf>,
        /// Ask the host's watch to distill the daybook at its next look, without waiting out its week
        #[arg(long, conflicts_with_all = ["text", "did", "why", "undo", "lesson", "links", "lessons", "repo", "tracker"])]
        distill: bool,
        /// The first day of the span `--lessons` lists
        #[arg(long, value_name = "YYYY-MM-DD", requires = "lessons", value_parser = daybook::day_named)]
        since: Option<String>,
        /// The last day of the span `--lessons` lists [default: none, to today]
        #[arg(long, value_name = "YYYY-MM-DD", requires = "lessons", value_parser = daybook::day_named)]
        until: Option<String>,
        /// The repo the entry is of [default: the one the checkout's remote names]
        #[arg(long, value_name = "OWNER/REPO", value_parser = tracker::repo_named)]
        repo: Option<RepoName>,
        /// The tracker whose daybook it is [default: the checkout's tracker on this host's checkout list]
        #[arg(long, value_name = "OWNER/REPO", value_parser = tracker::repo_named)]
        tracker: Option<RepoName>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Print the skill file that sends an agent of your own to `--help`: text, not JSON
    #[command(next_help_heading = SETS_UP, verbatim_doc_comment, long_about = skill::HELP)]
    Skill,
    /// Draw a team's workflows as graphs, or say where one issue is in its workflow — in an unbound repo, on its one path
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment, long_about = workflow::SHOW_HELP)]
    Show {
        /// The workflow to draw [default: every workflow of the team] — or an issue's
        /// number, which is `--issue`
        #[arg(value_name = "WORKFLOW|ISSUE")]
        workflow: Option<String>,
        /// Say instead where this issue is, from its stage records: its position, what
        /// follows, and the rework it has left — in an unbound repo, where it stands on its
        /// one path, from what a tick reads
        #[arg(long, conflicts_with = "workflow")]
        issue: Option<u64>,
        #[command(flatten)]
        team: TeamArgs,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Draft a team's roster from this host's host kit, into the team file of its working tree
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment, long_about = draft::HELP)]
    Draft {
        /// The team's working tree [default: this directory]
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// The team's directory on GitHub the working tree is of, as owner/repo or
        /// owner/repo/path: a role it names as ./path is read from that repo
        #[arg(long, value_parser = team_dir)]
        team: Option<RepoDir>,
        /// Replace a roster the team file holds without asking
        #[arg(long)]
        yes: bool,
    },
    /// Say where an issue would be after each of a list of verdicts, starting no worker
    #[command(
        next_help_heading = DESIGNER,
        verbatim_doc_comment,
        long_about = workflow::REHEARSE_HELP,
    )]
    Rehearse {
        /// The workflow the issue follows
        workflow: String,
        /// What is declared on it, in order: pass, reject, blocked, already-satisfied,
        /// escalate, or ruling
        #[arg(value_parser = declared)]
        declared: Vec<Declared>,
        #[command(flatten)]
        team: TeamArgs,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Say what the fleet on this host is doing and what is wrong with it, each finding with its fix
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = doctor::HELP)]
    Doctor {
        /// Every checkout on this host's checkout list, saying the host's findings once
        #[arg(long, conflicts_with = CHECKOUT_ARG)]
        all: bool,
        /// Add what happened since this time (epoch seconds or YYYY-MM-DDTHH:MM:SSZ); requires --all
        #[arg(long, requires = "all", value_parser = doctor::watching::since)]
        since: Option<u64>,
        /// Include earlier watch rounds; the default keeps only the last round's report
        #[arg(long)]
        rounds: bool,
        #[command(flatten)]
        proxy: ProxyArgs,
        /// The host whose fleet is asked after [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: At,
    },
    /// Show what a tick would act on, changing no fleet state (may push and delete a startup probe; read-only: `afkconvoy doctor`)
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = plan::HELP)]
    Plan {
        #[command(flatten)]
        proxy: ProxyArgs,
        /// The fleet instance whose claims are its own [default: nobody's]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[command(flatten)]
        at: At,
    },
    /// Add a checkout to this host's checkout list, start the host runner where none runs, and answer the checkout's first tick
    #[command(
        next_help_heading = OPERATOR,
        verbatim_doc_comment,
        long_about = host_runner::RUN_HELP,
    )]
    Run {
        /// The fleet instance to run as [default: a new one, named for this host]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        /// Ignored, and accepted for one release: no number bounds a checkout's workers, and
        /// admission alone decides each start. Kept with the checkout on the checkout list,
        /// where `doctor` says it is ignored; `default` drops it
        #[arg(long, value_name = "CLAIMS", value_parser = concurrency::claims_or_default)]
        concurrency: Option<To<NonZeroUsize>>,
        /// The private repo this checkout's fleet reads and writes its issues in: required
        /// on its first `run`; kept on this host and never pushed. `default` says its own repo
        /// explicitly, which must be private; omitted on later runs, its saved choice holds
        #[arg(long, value_name = "OWNER/REPO", value_parser = tracker::repo_or_default)]
        tracker: Option<To<RepoName>>,
        /// The harness this checkout's workers start on, from its next tick. `default` follows
        /// the host kit again. A word that is not a harness, or a harness the host kit does not
        /// hold, is an error and writes nothing
        #[arg(long, value_name = "HARNESS", value_parser = named_harness::harness_or_default)]
        harness: Option<To<Harness>>,
        #[command(flatten)]
        agent: Box<AgentArgs>,
        /// The port the host runner serves the panel on, from its next start: a host setting;
        /// `default` changes it back [default: 7070, or any free port where it is taken]
        #[arg(long, value_name = "PORT", value_parser = port_or_default)]
        panel: Option<To<u16>>,
        /// The operator's directory this host starts a watch in, when a look finds something
        /// to act on: a host setting; `default` uses the stock watch in afkconvoy's own directory
        /// [default: stock watch]
        #[arg(long, env = WATCH_DIR_ENV, value_name = "DIR", value_parser = watch_dir_or_default)]
        watch_dir: Option<To<PathBuf>>,
        /// Seconds between this host's looks: a host setting;
        /// `default` changes it back [default: 300]
        #[arg(long, env = WATCH_EVERY_ENV, value_name = "SECONDS", value_parser = watch_every_or_default)]
        watch_every: Option<To<u64>>,
        /// The cores a worker's processes may take before it turns hot, held two minutes: a
        /// host setting; `default` changes it back [default: half this host's cores, one at
        /// the least]
        #[arg(long, env = HOT_CORES_ENV, value_name = "CORES", value_parser = thousandths_or_default)]
        hot_cores: Option<To<u64>>,
        /// The GiB of memory a worker's processes may hold resident before it turns hot: a
        /// host setting; `default` changes it back. What runs under its `afkconvoy exclusive`
        /// commands counts only while `sysctl kern.memorystatus_vm_pressure_level` is 2 or
        /// above [default: a quarter of this host's memory]
        #[arg(long, env = HOT_MEMORY_ENV, value_name = "GIB", value_parser = thousandths_or_default)]
        hot_memory: Option<To<u64>>,
        /// The GiB a worker's processes may write to disk in ten minutes before it turns hot:
        /// a host setting; `default` changes it back [default: a tenth of the disk free here,
        /// one GiB at the least]
        #[arg(long, env = HOT_WRITTEN_ENV, value_name = "GIB", value_parser = thousandths_or_default)]
        hot_written: Option<To<u64>>,
        /// The load, a whole percent of this host's cores, past which no tick starts a new
        /// worker: a host setting; `default` changes it back [default: 200]
        #[arg(long, env = ADMIT_LOAD_ENV, value_name = "PERCENT", value_parser = whole_or_default)]
        admit_load: Option<To<u64>>,
        /// The memory free, a whole percent of this host's, under which no tick starts a new
        /// worker: a host setting; `default` changes it back [default: 10]
        #[arg(long, env = ADMIT_MEMORY_ENV, value_name = "PERCENT", value_parser = whole_or_default)]
        admit_memory: Option<To<u64>>,
        /// The disk free, a whole percent of the checkout's, under which no tick starts a new
        /// worker — 0 never holds for disk: a host setting; `default` changes it back
        /// [default: 1]
        #[arg(long, env = ADMIT_DISK_ENV, value_name = "PERCENT", value_parser = whole_or_default)]
        admit_disk: Option<To<u64>>,
        /// The seconds admission holds back for one reason before the condition starts a watch: a
        /// host setting; `default` changes it back [default: 600]
        #[arg(
            long,
            env = ADMIT_INSPECT_AFTER_ENV,
            value_name = "SECONDS",
            value_parser = whole_or_default
        )]
        admit_inspect_after: Option<To<u64>>,
        /// Seconds a running exclusive command may hold others up, counting only while
        /// another waits: a host setting; `default` changes it back
        /// [default: {const:EXCLUSIVE_SECONDS}]
        #[arg(long, env = EXCLUSIVE_SECONDS_ENV, value_name = "SECONDS", value_parser = whole_or_default)]
        exclusive_seconds: Option<To<u64>>,
        /// The shell command whose stdout is the TypeSafe key Jev is asked with when it judges a
        /// stopped worker's screen, such as one that reads the key from a keychain: a host
        /// setting, run afresh for each check. The key is sent only to TypeSafe, and is in no
        /// file, answer or log line; a command that fails or prints nothing is an error, and the
        /// worker waits for the operator as with none. `default` removes it [default: none]
        #[arg(long, env = JEV_KEY_COMMAND_ENV, value_name = "COMMAND", value_parser = jev_key_command_or_default)]
        jev_key_command: Option<To<String>>,
        /// Shell command receiving each human decision as JSON on stdin; `default` disables sending.
        #[arg(long, env = NOTIFY_COMMAND_ENV, value_name = "COMMAND", value_parser = boxed_notify_command)]
        notify_command: Option<Box<To<String>>>,
        /// The agent setup's instructions file, linked to the global instructions of every
        /// harness the host kit holds (Claude Code's `~/.claude/CLAUDE.md`, Codex's
        /// `~/.codex/AGENTS.md`, …): a host setting; `default` removes it [default: none]
        #[arg(long, env = AGENT_INSTRUCTIONS_ENV, value_name = "FILE", value_parser = boxed_file)]
        agent_instructions: Option<Box<To<PathBuf>>>,
        /// The agent setup's skill list, one line per source, `<source> <skill>...` or
        /// `<source> *`, installed through `npx skills` for every harness the host kit holds: a
        /// host setting; `default` removes it [default: none]
        #[arg(long, env = AGENT_SKILLS_ENV, value_name = "FILE", value_parser = boxed_file)]
        agent_skills: Option<Box<To<PathBuf>>>,
        /// The agent setup's guard list, one line per guard, `shell <command>` or `edits
        /// <command>`, wired before each matching tool call into the hook settings of every
        /// harness the host kit holds that has some (Claude Code's `~/.claude/settings.json`,
        /// Codex's `~/.codex/hooks.json`): a host setting; `default` removes it [default: none]
        #[arg(long, env = AGENT_GUARDS_ENV, value_name = "FILE", value_parser = boxed_file)]
        agent_guards: Option<Box<To<PathBuf>>>,
        /// The name this host goes by in the records it owns — candidates, claims, slots, logs —
        /// above the name the system gives it, which a network may change: a host renamed
        /// takes its own records for another host's. A `run` that finds none keeps the
        /// system's name then; `default` lets the next `run` keep it afresh
        #[arg(long, env = HOST_NAME_ENV, value_name = "NAME", value_parser = boxed_host_name)]
        host_name: Option<Box<To<String>>>,
        /// Worker lanes on this host, above zero, or `default` ({const:GATE_LANES}). Independent
        /// candidate worktrees of one repo may validate together within this bound.
        #[arg(long, env = "AFKCONVOY_GATE_LANES", value_parser = host_settings::watch_every_or_default)]
        gate_lanes: Option<To<u64>>,
        /// Slow lanes on this host, above zero, or `default` (1): how many rounds' slow gates
        /// run at once, beside the worker lanes, where fast gates run.
        #[arg(long, env = "AFKCONVOY_SLOW_GATE_LANES", value_parser = boxed_lanes)]
        slow_gate_lanes: Option<Box<To<u64>>>,
        /// Fast gates that may run at once on this host, above zero, or `default`
        /// ({const:FAST_LANES}). They take a lane of their own, beside the worker lanes: no slow
        /// gate holds them up, and fast gates of one repo run side by side.
        #[arg(long, env = "AFKCONVOY_FAST_LANES", value_parser = boxed_lanes)]
        fast_lanes: Option<Box<To<u64>>>,
        /// Stop after this many ticks
        #[arg(long, hide = true)]
        ticks: Option<u64>,
        /// Milliseconds one second of sleep takes
        #[arg(long, hide = true, default_value_t = 1000)]
        second_ms: u64,
        /// Panic as this tick ends: its fleet instance breaks
        #[arg(long, hide = true)]
        panic_at_tick: Option<u64>,
        /// Hold the fleet instance in this process, printing its lines, until it is told to stop
        #[arg(long, hide = true)]
        foreground: bool,
        #[command(flatten)]
        at: At,
    },
    /// Take a checkout off this host's checkout list and stop its fleet instance, or with `--all` stop the host runner and keep the list
    #[command(
        next_help_heading = OPERATOR,
        verbatim_doc_comment,
        long_about = host_runner::STOP_HELP,
    )]
    Stop {
        /// Stop every fleet instance and then the host runner, keeping the checkout list
        #[arg(long, conflicts_with = CHECKOUT_ARG)]
        all: bool,
        /// Release the claims its fleet instance holds and end those workers before taking the checkout off the list
        #[arg(long, conflicts_with = "all")]
        release: bool,
        /// The fleet instance whose claims `--release` releases [default: the one this host's runner holds of the checkout, else the newest its tick log names]
        #[arg(long, requires = "release", value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[command(flatten)]
        at: At,
    },
    /// Say which checkouts this host works and how each stands
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = checkouts::HELP)]
    Checkouts,
    /// Read this host's exclusive queue and explain each wait
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = queue::HELP)]
    Queue,
    /// Hold every checkout on this host's checkout list, as `run` starts it in the background
    #[command(hide = true)]
    HostRunner,
    /// Print the panel — every fleet of this host as one web page — as one rebuild of each checkout shows it now, or say where the host runner serves it live
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = panel::HELP)]
    Panel {
        /// The fleet instance whose claims are its own, of every checkout printed [default:
        /// the one its tick log on this host names last]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        /// Say where this host's runner serves the panel, live: nothing is started
        #[arg(long, conflicts_with_all = ["instance", "host", CHECKOUT_ARG])]
        open: bool,
        #[command(flatten)]
        at: At,
    },
    /// Run one reconciliation pass and exit
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = tick::HELP)]
    Tick {
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
        #[command(flatten)]
        at: At,
    },
    /// Run the gate in this worktree and record a green run
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = gate::HELP)]
    Gate {
        /// Take the open issue PR's merge, gate, record and landing turn
        #[arg(long, alias = "train")]
        land: bool,
        /// Run the repo's fast gate alone, under its own limit and budget, recording nothing
        #[arg(long, conflicts_with_all = ["land", "slow"])]
        fast: bool,
        /// Run the repo's slow gate alone, under its own limit and budget, recording nothing
        #[arg(long, conflicts_with = "land")]
        slow: bool,
        /// The host a green run is recorded as made on [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: At,
    },
    /// Land this worktree's PR on its pinned target branch in one turn
    #[command(
        next_help_heading = WORKER,
        verbatim_doc_comment,
        override_usage = "afkconvoy land <n> | afkconvoy land --issue <n>",
        long_about = turn::HELP,
    )]
    Land {
        /// The issue the PR closes
        #[arg(value_name = "ISSUE", conflicts_with_all = ["train", "issue", "watch"])]
        number: Option<u64>,
        /// The same, as a flag
        #[arg(
            long,
            required_unless_present_any = ["train", "number", "watch"],
            conflicts_with_all = ["train", "watch"]
        )]
        issue: Option<u64>,
        /// Land the PR of this watch's checkout, a change of the repo on no issue (a
        /// watch's command)
        #[arg(long, conflicts_with = "train")]
        watch: bool,
        /// Complete migration left by an older release
        #[arg(long, hide = true)]
        train: bool,
        /// Seconds GitHub is given to show a landed PR as merged
        #[arg(long, hide = true, default_value_t = 0)]
        merged_timeout: u64,
        /// The host named in the gate record and watch merge [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Report the current human question settled or put aside (the asking session's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = asking_report::HELP)]
    Asked {
        /// The question's episode id from its brief
        #[arg(long)]
        question: String,
        /// The JSON settlement or put-aside report
        #[arg(long)]
        text_file: PathBuf,
    },
    /// Record what this host's watch did and complete its round (a watch's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = watch_report::HELP)]
    Watched {
        /// The watch id from its brief
        #[arg(long)]
        watch: String,
        /// The JSON report with all five parts
        #[arg(long)]
        text_file: PathBuf,
    },
    /// Declare the verdict that ends a stage, with the text it produced (a worker's command, and a human's for a stage nobody holds)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = stage::HELP)]
    Verdict {
        /// What the stage declares: pass, reject, blocked, already-satisfied, or escalate
        #[arg(value_parser = verdict)]
        verdict: Declared,
        #[command(flatten)]
        issue: IssueArg,
        /// The stage that declares it: the one its worker was started on
        #[arg(long)]
        stage: String,
        #[command(flatten)]
        text: TextArgs,
        /// What a `blocked` verdict waits on, separated by commas: issue numbers, and `cut` for the next cut of this repo — a version that is not installed yet
        #[arg(long, value_delimiter = ',', value_parser = blocker)]
        blocked_by: Vec<Blocker>,
        /// The commit of the role's repo a worker's stage ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a worker's stage ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
        /// The harness a worker's stage ran on [a human names none]
        #[arg(long, env = HARNESS_ENV, value_parser = harness, requires = "role_commit")]
        harness: Option<Harness>,
        /// The model a worker ran, where it was started on one [a human names none]
        #[arg(long, env = MODEL_ENV, requires = "role_commit")]
        model: Option<String>,
        /// The worker is the stage's consult, letting its `escalate` through [a human is none]
        #[arg(long, env = CONSULT_ENV)]
        consult: bool,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Answer an escalated issue with a ruling: its stage runs again under it (a human's command, and a consult's for the stage that asked it)
    #[command(next_help_heading = HUMAN, verbatim_doc_comment, long_about = steer::RULE_HELP)]
    Rule {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        text: TextArgs,
        /// The stage whose hand-off a consult rules on: the one its environment
        /// names [a human names none]
        #[arg(long, env = STAGE_ENV, requires = "role_commit")]
        stage: Option<String>,
        /// The commit of the role's repo a consult ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a consult ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
        /// The harness a consult ran on [a human names none]
        #[arg(long, env = HARNESS_ENV, value_parser = harness, requires = "role_commit")]
        harness: Option<Harness>,
        /// The model a worker ran, where it was started on one [a human names none]
        #[arg(long, env = MODEL_ENV, requires = "role_commit")]
        model: Option<String>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Send an issue to a named stage of its workflow (a human's command, and a consult's)
    #[command(next_help_heading = HUMAN, verbatim_doc_comment, long_about = steer::SEND_HELP)]
    Send {
        #[command(flatten)]
        issue: IssueArg,
        /// The stage to put it at
        #[arg(long)]
        stage: String,
        #[command(flatten)]
        text: TextArgs,
        /// The commit of the role's repo a consult ran under [a human names none]
        #[arg(long, env = ROLE_COMMIT_ENV, requires = "team_commit")]
        role_commit: Option<String>,
        /// The commit of the team's repo a consult ran under [a human names none]
        #[arg(long, env = TEAM_COMMIT_ENV, requires = "role_commit")]
        team_commit: Option<String>,
        /// The harness a consult ran on [a human names none]
        #[arg(long, env = HARNESS_ENV, value_parser = harness, requires = "role_commit")]
        harness: Option<Harness>,
        /// The model a worker ran, where it was started on one [a human names none]
        #[arg(long, env = MODEL_ENV, requires = "role_commit")]
        model: Option<String>,
        /// The worker is the stage's consult [a human is none]
        #[arg(long, env = CONSULT_ENV)]
        consult: bool,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Take an issue in to a workflow: its flow label, on the record (a worker's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = intake::HELP)]
    Intake {
        #[command(flatten)]
        issue: IssueArg,
        /// The workflow it is to follow, by name: what follows `flow:` in its flow label
        #[arg(long)]
        workflow: Option<String>,
        /// Existing Jira delivery identity; selects it instead of inheritance from --from
        #[arg(long)]
        delivery: Option<String>,
        /// The role of the worker that takes it in
        #[arg(long, env = ROLE_ENV)]
        role: Option<String>,
        /// The issue that worker is on
        #[arg(long, env = ISSUE_ENV, conflicts_with = "patrol")]
        from: Option<u64>,
        /// The patrol that worker runs, when it is on no issue
        #[arg(long, env = PATROL_ENV)]
        patrol: Option<String>,
        /// Taken in by the repair worker, for a defect of the base branch: no role, issue or patrol
        #[arg(long, hide = true, alias = "train")]
        repair: bool,
        /// The issues it is to wait for, as numbers separated by commas
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<u64>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Select or read a task's pinned target branch
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = target::HELP)]
    Target {
        #[command(flatten)]
        issue: IssueArg,
        /// An existing same-repo branch on origin; omit to read the destination
        #[arg(long)]
        branch: Option<String>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Create or inspect a Jira delivery
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = delivery::HELP)]
    Delivery {
        /// Full HTTPS Jira browse URL, including the site
        #[arg(long)]
        jira: String,
        /// Existing branch on origin; required for creation and attachment
        #[arg(long)]
        final_target: Option<String>,
        /// Open tracker task to attach before dispatch or intake
        #[arg(long, requires = "final_target")]
        issue: Option<u64>,
        /// Check the attached tasks, gate the combined delivery against its final target, and hand the same final PR to a human
        #[arg(long, conflicts_with_all = ["final_target", "issue"])]
        handoff: bool,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Raise a fleet lock on an issue: the fleet works lock issues only until it closes (a worker's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = lock::HELP)]
    Lock {
        #[command(flatten)]
        issue: IssueArg,
        /// The role that raises it [default: the one this worker was started in, if any]
        #[arg(long, env = ROLE_ENV)]
        role: Option<String>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Have a patrol run now, past its interval and its daily bound: the next tick runs it
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = patrol::HELP)]
    PatrolNow {
        /// The patrol, as the repo's config names it under `patrols`
        name: String,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Declare a patrol's run over: the next tick ends it (a worker's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = claims::OVER_HELP)]
    Over {
        /// The run, as its brief names it
        #[arg(long)]
        run: String,
        #[command(flatten)]
        at: At,
    },
    /// Take the steps of a cut: set the version, land it, tag it, publish and install it (a cut's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = cut::HELP)]
    Cut {
        /// Go on from this step: set, push, gate, tag or publish [default: the step after the last it took]
        #[arg(long, value_name = "STEP", value_parser = cut_step)]
        from: Option<afkconvoy_core::cut::Step>,
        /// The host its gate runs and is recorded as on [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: At,
    },
    /// Run a heavy command in this host's worker lanes (a worker's command)
    #[command(next_help_heading = WORKER, verbatim_doc_comment, long_about = exclusive::HELP)]
    Exclusive {
        /// The command and its arguments, after `--`
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Allow or refuse the tool call described on stdin (a worker's hook, added at launch)
    #[command(next_help_heading = HOOK, verbatim_doc_comment, long_about = guard::HELP)]
    Guard {
        /// Check only asking Terminal input, outside a bound stage
        #[arg(long)]
        terminal_input_only: bool,
    },
    /// Start a fresh worker on one issue
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = dispatch::HELP)]
    Dispatch {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
        /// Where to start from: what survives, or — discarding it — the base branch
        #[arg(long, value_enum, default_value = "auto")]
        start: dispatch::StartFrom,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Show what `dispatch` would continue an issue from, without acting
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = recovery::HELP)]
    Recovery {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Close one of my claims whose issue needed no change
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = judge::CLOSE_HELP)]
    Close {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Hand one of my claims to a human
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = judge::ESCALATE_HELP)]
    Escalate {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        /// Why: what a human needs to act on it in one reply
        #[arg(long)]
        reason: String,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Leave one of my claims waiting on the issues its worker declared it blocked by
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = park::HELP)]
    Park {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Tell the worker of one of my claims, stopped with no PR and no verdict, to carry on
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = judge::NUDGE_HELP)]
    Nudge {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Tell the worker at work on an issue something it must know, busy or stopped
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = tell::HELP)]
    Tell {
        #[command(flatten)]
        issue: IssueArg,
        /// The note
        #[arg(
            long,
            value_name = "TEXT",
            required_unless_present = "file",
            conflicts_with = "file"
        )]
        text: Option<String>,
        /// A file holding the note
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// The host the worker runs on [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Replace the silent worker of one of my claims whose PR is waiting to land, once
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment, long_about = watch::HELP)]
    Restart {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Put a harness or model first in this machine's kit; optionally continue existing workers
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = switch::HELP)]
    Switch {
        /// The known installed harness to put first, adding it when the kit does not hold it
        #[arg(long, value_parser = kit_harness, required_unless_present = "model", conflicts_with = "model")]
        harness: Option<Harness>,
        /// Put this model first within its harness, and that harness first in the kit
        #[arg(long)]
        model: Option<String>,
        /// The issue whose worker to continue in this checkout
        #[arg(value_name = "ISSUE", conflicts_with_all = ["all", "patrol"])]
        issue: Option<u64>,
        /// The patrol whose worker to continue in this checkout
        #[arg(long, conflicts_with = "all")]
        patrol: Option<String>,
        /// Continue workers in every checkout on this machine
        #[arg(long)]
        all: bool,
        /// Optional owner of a targeted claim; not a machine preference
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        agent: AgentArgs,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Take another fleet instance's claims without waiting for the lease
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment, long_about = takeover::HELP)]
    Takeover {
        /// Show every instance the remote remembers, and take nothing
        #[arg(long, conflicts_with_all = ["from", "yes"])]
        list: bool,
        /// The instance whose claims to take
        #[arg(long)]
        from: Option<String>,
        /// Take from an instance whose heartbeat is still fresh
        #[arg(long)]
        yes: bool,
        /// The fleet instance to take them as
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: At,
    },
    /// List every claim and heartbeat on the remote
    #[command(next_help_heading = PLUMBING, verbatim_doc_comment, long_about = claims::SCAN_HELP)]
    Scan {
        #[command(flatten)]
        at: At,
    },
    /// Claim one issue for a fleet instance
    #[command(next_help_heading = PLUMBING, verbatim_doc_comment, long_about = claims::CLAIM_HELP)]
    Claim {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Release a fleet instance's own claim on one issue
    #[command(
        next_help_heading = PLUMBING,
        verbatim_doc_comment,
        long_about = claims::RELEASE_HELP,
    )]
    Release {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Refresh a fleet instance's heartbeat, if it is due
    #[command(
        next_help_heading = PLUMBING,
        verbatim_doc_comment,
        long_about = claims::HEARTBEAT_HELP,
    )]
    Heartbeat {
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: At,
    },
    /// Print the index, the help of the given command, a help topic, or with `all` the whole manual
    #[command(next_help_heading = READER)]
    Help {
        /// A command, a topic, or `all`: `--help` lists the commands and the topics
        name: Option<String>,
    },
}

/// The one issue a command is about: its number, as an argument or as
/// `--issue`. One of the two, never both.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct IssueArg {
    /// The issue's number
    #[arg(value_name = "ISSUE")]
    number: Option<u64>,
    /// The same, as a flag
    #[arg(long = "issue", value_name = "ISSUE")]
    flag: Option<u64>,
}

impl IssueArg {
    fn number(&self) -> u64 {
        // The group has clap refuse a command line that names neither.
        self.number.or(self.flag).unwrap_or_default()
    }
}

/// The checkout a command is about, named after it: for a command with no
/// argument of its own after it (`AtFlag` for one with).
#[derive(clap::Args)]
struct At {
    /// The checkout it is about, or a directory in it [default: the one it is run in]
    #[arg(value_name = "CHECKOUT")]
    checkout: Option<PathBuf>,
}

/// The step `word` names, of a cut (`--from`).
fn cut_step(word: &str) -> Result<afkconvoy_core::cut::Step, String> {
    afkconvoy_core::cut::Step::named(word).ok_or_else(|| {
        let words: Vec<&str> =
            afkconvoy_core::cut::Step::ALL.iter().map(|step| step.word()).collect();
        format!("names no step of a cut: {}", words.join(", "))
    })
}

/// The checkout a command is about, named with `-C`: for a command whose
/// arguments after it are its own.
#[derive(clap::Args)]
struct AtFlag {
    /// The checkout it is about, or a directory in it [default: the one it is run in]
    #[arg(short = 'C', value_name = "CHECKOUT")]
    checkout: Option<PathBuf>,
}

/// The id of the argument of `At` and of `AtFlag`: a command that has it is
/// about one repo, and one that has neither is about none.
const CHECKOUT_ARG: &str = "checkout";

/// The team a command reads.
#[derive(clap::Args)]
struct TeamArgs {
    /// The team's directory on GitHub, as owner/repo or owner/repo/path [default: the
    /// team this checkout's config names]
    #[arg(long, value_parser = team_dir)]
    team: Option<RepoDir>,
    /// A directory of a team's files, read as they are instead of from GitHub; with
    /// --team, as an edit of that team: a role it names as ./path is read from that repo
    #[arg(long)]
    dir: Option<PathBuf>,
}

impl TeamArgs {
    /// Where the team they name is read from, with the checkout to read it
    /// for: `checkout`, which names the team where they name none — and must
    /// then be had — and otherwise gives its labels where it is had.
    fn read(self, checkout: anyhow::Result<Repo>) -> anyhow::Result<(team::From, Option<Repo>)> {
        let from = self.from();
        let checkout = match from {
            team::From::Config => Some(checkout?),
            _ => checkout.ok(),
        };
        Ok((from, checkout))
    }

    fn from(self) -> team::From {
        match (self.team, self.dir) {
            (of, Some(dir)) => team::From::Dir(dir, of),
            (Some(team), None) => team::From::Repo(team),
            (None, None) => team::From::Config,
        }
    }
}

/// The text a stage record carries.
#[derive(clap::Args)]
struct TextArgs {
    /// The text of the record: what the stage produced, or what a human says
    #[arg(long, required_unless_present = "text_file", conflicts_with = "text_file")]
    text: Option<String>,
    /// A file holding that text
    #[arg(long)]
    text_file: Option<PathBuf>,
}

impl TextArgs {
    fn read(self) -> anyhow::Result<String> {
        match (self.text, self.text_file) {
            (Some(text), _) => Ok(text),
            (None, Some(file)) => std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display())),
            (None, None) => Ok(String::new()),
        }
    }
}

/// The host's network settings, never the repo's config.
#[derive(clap::Args, Default, Clone)]
struct ProxyArgs {
    /// Hosts reached directly, comma-separated, for the default harness or
    /// HARNESS; repeat for each harness. DSH defaults to api.deepseek.com.
    /// An empty list disables the stock addition. Extends no_proxy and NO_PROXY.
    #[arg(long, value_name = "[HARNESS=]HOSTS")]
    worker_no_proxy: Vec<String>,
}

/// How a command that starts a worker starts it.
#[derive(clap::Args, Clone)]
struct AgentArgs {
    // Keep only a diagnostic for callers migrating from the removed flag.
    #[arg(long = "worker-command", hide = true, value_parser = removed_worker_command)]
    _worker_command: Option<String>,

    #[command(flatten)]
    proxy: ProxyArgs,
    /// Seconds an agent is given to be ready for its prompt
    #[arg(long, default_value_t = 120, hide = true)]
    ready_timeout: u64,
}

/// The fleet instance a command acts as.
#[derive(clap::Args)]
struct Who {
    #[arg(long, value_parser = InstanceId::parse)]
    instance: InstanceId,
    /// The host the instance runs on [default: this machine's name]
    #[arg(long)]
    host: Option<String>,
}

impl Who {
    fn me(self, now: u64) -> Me {
        Me { instance: self.instance, host: self.host.unwrap_or_else(this_host), now }
    }
}

/// The start of a command line that runs this very binary, for a prompt: a
/// worker's shell need not have it on its PATH. What follows is its arguments.
pub fn own_command() -> Line {
    Line::new().program(&own_program())
}

/// The same, run from `dir`: a worker that starts in its role's copy runs its
/// commands from its worktree.
pub fn own_command_in(dir: &str) -> Line {
    Line::new().fixed("cd ").word(dir).fixed(" && ").program(&own_program())
}

/// `afkconvoy` where the `PATH` finds this very file under that name, so a
/// brief's lines follow the binary installed there, as a worker's terminal
/// keeps running them after it moves; else this file's whole path. A brief
/// that named the path of an install since replaced kept its workers on the
/// old release (2026-10-29).
fn own_program() -> String {
    let Some(exe) = std::env::current_exe().ok() else {
        return "afkconvoy".to_string();
    };
    if on_path("afkconvoy").is_some_and(|found| same_file(&found, &exe)) {
        return "afkconvoy".to_string();
    }
    exe.to_str().map_or_else(|| "afkconvoy".to_string(), str::to_string)
}

/// The first file the `PATH` finds under `name`.
fn on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|file| file.is_file())
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// What a command is to the fleet: whether it acts, whether a worker's
/// worktree is taken as its checkout, and whether it starts the fleet's
/// work. Declared once, by `kind`; its help (`cli`), the checkout it takes
/// (`checkout::resolve`) and its acting log (`Command::acting`) all read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kind {
    /// It acts: it logs a line and wakes the `run` as it ends
    /// (`Command::acting`), and its `--help` says so (`acting::HELP`).
    acts: bool,
    worktree: Worktree,
    /// It starts the fleet's work: it checks this host before anything else
    /// (`probe::startup`), and is refused on a host with no git naming every
    /// command it lacks (`checkout::started`).
    starts: bool,
}

/// Whether a worker's worktree is taken as a command's checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Worktree {
    /// It is not: only a worker's own commands, and those that only read,
    /// are run there.
    Refused,
    /// A worker runs it in its own worktree, as its brief, a stage's
    /// consult's and a watch's give it.
    Its,
    /// It only reads — but with the flag named, which makes it act: it
    /// changes nothing on GitHub, on the remote, in the checkout or of a
    /// `run`. A consult's brief lets it run any command that only reads,
    /// and what one reads from a worker's branch acts for nobody.
    Reads(Option<&'static str>),
}

impl Kind {
    const PLAIN: Kind = Kind { acts: false, worktree: Worktree::Refused, starts: false };
    const ACTS: Kind = Kind { acts: true, ..Kind::PLAIN };
    const WORKERS: Kind = Kind { worktree: Worktree::Its, ..Kind::PLAIN };
    const READS: Kind = Kind { worktree: Worktree::Reads(None), ..Kind::PLAIN };

    /// Whether a worker's worktree is taken for it, as `given`: one that only
    /// reads but for a flag is not given that flag.
    fn takes_worktree(self, given: &clap::ArgMatches) -> bool {
        let acts =
            |flag: &str| given.value_source(flag) == Some(clap::parser::ValueSource::CommandLine);
        match self.worktree {
            Worktree::Refused => false,
            Worktree::Its | Worktree::Reads(None) => true,
            Worktree::Reads(Some(flag)) => !acts(flag),
        }
    }
}

/// The kind of the command `name`: each command's, here once. One not named
/// acts never, refuses a worker's worktree and starts nothing.
fn kind(name: &str) -> Kind {
    let worker_acts = Kind { acts: true, worktree: Worktree::Its, starts: false };
    match name {
        "dispatch" | "close" | "escalate" | "park" | "nudge" | "tell" | "restart" | "switch"
        | "claim" | "release" | "heartbeat" | "patrol-now" => Kind::ACTS,
        "takeover" => Kind { worktree: Worktree::Reads(Some("from")), ..Kind::ACTS },
        "cast" | "target" | "delivery" | "gate" | "land" | "verdict" | "rule" | "send"
        | "intake" | "lock" | "over" | "cut" | "wake" | "rewrite-accounts" => worker_acts,
        "tick" => Kind { starts: true, ..Kind::ACTS },
        "doctor" | "show" | "exclusive" | "daybook" => Kind::WORKERS,
        "plan" => Kind { starts: true, ..Kind::WORKERS },
        "run" => Kind { starts: true, ..Kind::PLAIN },
        "accounts" | "config" | "check" | "rehearse" | "recovery" | "scan" | "panel" => Kind::READS,
        _ => Kind::PLAIN,
    }
}

/// The command line `cli` as the operator's session reads it. The watch, and
/// the asking session beside it, are the operator on their host: a `rule` or
/// `send` they run is a human's, though the fleet started them on a harness
/// and their environment names it (#554). Only a consult names its harness
/// and model to those two, beside its commits.
fn operators(cli: clap::Command) -> clap::Command {
    let session = daybook::Kind::of(|name| std::env::var(name).ok());
    if !matches!(session, daybook::Kind::Watch | daybook::Kind::Asking) {
        return cli;
    }
    ["rule", "send"].into_iter().fold(cli, |cli, name| {
        cli.mut_subcommand(name, |command| {
            command
                .mut_arg("harness", |arg| arg.env(None::<&str>))
                .mut_arg("model", |arg| arg.env(None::<&str>))
        })
    })
}

/// The command line as parsed: `Cli`, each command's text whole, each command
/// about one repo with how it takes its checkout added to its text, and each
/// command that acts with what it logs.
///
/// A command's text opens with its one line, the doc comment of its `Command`
/// variant, and goes on with what its `long_about` names: a const of the
/// module that implements it.
fn cli() -> clap::Command {
    let mut cli = Cli::command().mut_subcommands(|command| {
        let Some(rest) = command.get_long_about() else { return command };
        let about = command.get_about().map(ToString::to_string).unwrap_or_default();
        let text = format!("{about}\n\n{rest}");
        command.long_about(text)
    });
    let about: Vec<(String, String)> = cli
        .get_subcommands()
        .filter_map(|command| {
            let arg = command.get_arguments().find(|arg| arg.get_id() == CHECKOUT_ARG)?;
            let name = command.get_name();
            Some((name.to_string(), checkout_help(name, arg.is_positional())))
        })
        .collect();
    for (name, said) in about {
        cli = cli.mut_subcommand(name, |command| added(command, &said));
    }
    let acts: Vec<String> = cli
        .get_subcommands()
        .map(|command| command.get_name().to_string())
        .filter(|name| kind(name).acts)
        .collect();
    for name in acts {
        cli = cli.mut_subcommand(name, |command| added(command, acting::HELP));
    }
    cli
}

/// `command` with `said` added to the end of its text.
fn added(command: clap::Command, said: &str) -> clap::Command {
    let text = command.get_long_about().map(ToString::to_string).unwrap_or_default();
    command.long_about(format!("{}\n\n{said}", text.trim_end()))
}

/// What the manual adds to the text of the command `name`, about one repo:
/// how it is told the checkout — after it, or with `-C` where what follows it
/// is its own (`positional`) — and whether a worker's worktree is one.
fn checkout_help(name: &str, positional: bool) -> String {
    let (named, own) = match positional {
        true => ("Name it after the command, as `[CHECKOUT]`", ""),
        false => (
            "Name it with `-C <CHECKOUT>`",
            " What follows the command is its own: the checkout is not among it.",
        ),
    };
    let refused = "A worker's worktree is refused as its checkout: only a worker's own commands, \
                   and those that only read, are run there.";
    let worker = match kind(name).worktree {
        Worktree::Its => "A worker runs it in its own worktree.".to_string(),
        Worktree::Reads(None) => "It only reads: a worker's worktree is one too.".to_string(),
        Worktree::Reads(Some(flag)) => format!(
            "Without `--{flag}` it only reads, and a worker's worktree is one too. With `--{flag}`, \
             it is not: only a worker's own commands, and those that only read, are run there."
        ),
        Worktree::Refused => refused.to_string(),
    };
    let none = match name {
        "panel" => {
            "Naming none, it is about every checkout on this host's checkout list — or, where the \
             list holds none, the checkout it is run in."
        }
        _ => "Naming none, it is about the checkout it is run in.",
    };
    format!(
        "## Which checkout it is about\n\n{named}: a checkout of the repo, or any directory in \
         one, which names that checkout.{own} {none} {worker}"
    )
}

/// Whether `name` takes its checkout as a path after the command
/// (`Some(true)`), with `-C` (`Some(false)`), or takes none (`None`). The
/// same fact `checkout_help` writes into the manual: a fix line uses it, so
/// the line and a command typed by hand name the checkout the same way.
pub(crate) fn positional_checkout(name: &str) -> Option<bool> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static FORMS: OnceLock<HashMap<String, bool>> = OnceLock::new();
    FORMS
        .get_or_init(|| {
            cli()
                .get_subcommands()
                .filter_map(|command| {
                    let arg = command.get_arguments().find(|arg| arg.get_id() == CHECKOUT_ARG)?;
                    Some((command.get_name().to_string(), arg.is_positional()))
                })
                .collect()
        })
        .get(name)
        .copied()
}

/// The manual: `--help` answers its index, `help all` it whole, and `help
/// <topic>` one topic of it. Its text is the guide (`Cli`'s doc comment), each
/// command's and each topic's, in the order given here: the way in first, the
/// reference last.
pub fn manual() -> agent_help::Help {
    use topic::*;
    let mut help = agent_help::Help::new(cli())
        .topic::<FirstStart>()
        .topic::<Concepts>()
        .topic::<Team>()
        .topic::<Roles>()
        .topic::<Roster>()
        .topic::<SkillFile>()
        .topic::<Workflows>()
        .topic::<Patrols>()
        .topic::<Example>()
        .topic::<ExampleRun>()
        .topic::<Cache>()
        .topic::<TickLog>()
        .topic::<WorkerLaunch>()
        .topic::<WorkerCodex>()
        .topic::<WorkerProxy>()
        .topic::<Humans>()
        .topic::<Troubleshooting>()
        .topic::<Glossary>()
        .table::<doctor::Severity>("severity")
        .table::<claim_status::ClaimStatus>("claim_status")
        .constant("CONFIG_FILE", config::FILE)
        .constant("ACCOUNT_SCHEMA_VERSION", afkconvoy_core::account::SCHEMA_VERSION)
        .constant("LAND_ENDINGS", turn::endings_help())
        .constant("REMOTE_READS", remote::reads_help())
        .constant("GH_IN_TRANSIT", gh::in_transit_help())
        .constant("PANEL_PORT", host_settings::PANEL_PORT)
        .constant("FIRST_START_SETTINGS", first_start::settings_help())
        .constant("FIRST_START_QUESTIONS", first_start::help())
        .constant("EXCLUSIVE_SECONDS", exclusive::SECONDS)
        .constant("GATE_LANES", host_settings::GATE_LANES)
        .constant("FAST_LANES", host_settings::FAST_LANES)
        .constant("EXCLUSIVE_BOUND_CODE", exclusive::BOUND_CODE)
        .constant("NOTIFY_SECONDS", watch_notify::SECONDS)
        .constant("NOTIFY_FAILURES", watch_notify::FAILURES)
        .constant("DAYBOOK_RETRY_MAX", watch_daybook::RETRY_MAX)
        .constant("WATCH_EVERY", host_settings::WATCH_EVERY)
        .constant("WATCH_TITLE", watch_terminal::TITLE)
        .constant("WATCH_CHECKOUT", host_watch::CHECKOUT)
        .constant("KIT_RESEARCH_STATES", kit_research::states_help())
        .constant("TEAM_FILE", afkconvoy_core::team::TEAM_FILE)
        .constant("ESCALATE_LABEL", afkconvoy_core::config::ESCALATE_LABEL)
        .constant("QUEUE_BOUND", afkconvoy_core::admission::QUEUE_BOUND)
        .constant("GATE_SECONDS", afkconvoy_core::config::GATE_SECONDS)
        .constant("RECOVERY_SECONDS", crate::health::RECOVERY_SECONDS)
        .constant("CAPACITY_WAIT_SECONDS", health::CAPACITY_WAIT_SECONDS)
        .constant("CAPACITY_SECOND_WAIT_SECONDS", health::CAPACITY_WAIT_SECONDS * 2)
        .constant("MAX_CAPACITY_STOPS", health::MAX_CAPACITY_STOPS)
        .constant("CAPACITY_HEALTHY_SECONDS", health::CAPACITY_HEALTHY_SECONDS)
        .constant("INTAKE_LIMIT", afkconvoy_core::config::INTAKE_LIMIT)
        .constant("MAX_REWORK", afkconvoy_core::team::MAX_REWORK)
        .constant("MAX_CONSECUTIVE_REWORK", afkconvoy_core::team::MAX_CONSECUTIVE_REWORK)
        .constant("PATROL_NAME_MOST", afkconvoy_core::team::PATROL_NAME_MOST)
        .constant("SLOW_GATE_LANDINGS", afkconvoy_core::slow_gate::LANDINGS)
        .constant("SLOW_GATE_PERCENT", afkconvoy_core::slow_gate::PERCENT)
        .constant("HARNESS_PROBE_MINUTES", harness_down::PROBE_SECONDS / 60)
        .constant("LOG_KEPT_HOURS", log::KEPT_FOR / 3600)
        .constant(
            "CODEX_GOAL_TOKENS",
            afkconvoy_core::harness::Harness::Codex.goal_tokens().unwrap_or_default(),
        )
        .constant("LOG_KEPT_LEAST", log::KEPT_LEAST)
        .constant("LOG_KEPT_MOST", log::KEPT_MOST)
        .constant("WAKE_ANSWER_SECONDS", wake::ANSWER_WITHIN.as_secs())
        .constant("BACK_TO_BACK", afkconvoy_core::tick::BACK_TO_BACK)
        .constant("TICK_BUSY_SECONDS", afkconvoy_core::tick::BUSY_SECONDS)
        .constant("TICK_IDLE_MINUTES", afkconvoy_core::tick::IDLE_SECONDS / 60)
        .constant("TICK_IDLE_AFTER", afkconvoy_core::tick::IDLE_AFTER)
        .constant("GRACE", afkconvoy_core::worker::GRACE)
        .constant("LEASE_MINUTES", afkconvoy_core::lease::LEASE / 60)
        .constant("BEAT_EVERY_MINUTES", afkconvoy_core::lease::BEAT_EVERY / 60)
        .constant("BACKOFF_FIRST_MINUTES", afkconvoy_core::host_runner::BACKOFF_FIRST / 60)
        .constant("BACKOFF_MOST_MINUTES", afkconvoy_core::host_runner::BACKOFF_MOST / 60)
        .constant("INSTALLED_LOOKED_SECONDS", host_runner::INSTALLED_LOOKED_EVERY.as_secs())
        .constant("QUEUE_SECONDS", queue::WITHIN.as_secs())
        .constant("MOVED_WITHIN_SECONDS", host_runner::MOVED_WITHIN.as_secs())
        .constant("MOVE_WAIT_SECONDS", host_runner::MOVE_WAIT_WITHIN.as_secs())
        .constant("PUBLISHED_LOOKED_MINUTES", published::LOOKED_EVERY.as_secs() / 60)
        .constant("OUTGROWN_TIMES", afkconvoy_core::landing::OUTGROWN_TIMES)
        .constant("OUTGROWN_GIB", afkconvoy_core::landing::OUTGROWN_FLOOR >> 30)
        .constant("TIMED_OUT_CODE", gate::TIMED_OUT_CODE)
        .constant("STRAYS_LISTED", gate::STRAYS_LISTED)
        .constant("DOCTOR_FAILED_SHOWN", doctor::FAILED_SHOWN)
        .constant("DOCTOR_EVENTS_SHOWN", run_log::EVENTS_SHOWN)
        .constant("CUT_MINOR_LABEL", afkconvoy_core::cut::MINOR_LABEL)
        .constant("CUT_BUSY_AFTER_SECONDS", afkconvoy_core::cut::BUSY_AFTER_SECONDS);
    for (name, group) in [
        ("OPERATOR", OPERATOR),
        ("DESIGNER", DESIGNER),
        ("HANDS_WORK", HANDS_WORK),
        ("SETS_UP", SETS_UP),
        ("WORKER", WORKER),
        ("HUMAN", HUMAN),
        ("HOOK", HOOK),
        ("BY_HAND", BY_HAND),
        ("PLUMBING", PLUMBING),
    ] {
        help = help.constant(name, group);
    }
    help
}

/// The 1-based line of `help` that names `docs/adr/`.
///
/// That path is a decision record in this repository. A reader of the binary
/// has the help and nothing else, so the citation names a document it cannot
/// open. `None` when `help` names none.
pub fn names_closed_adr(help: &str) -> Option<usize> {
    help.lines().position(|line| line.contains("docs/adr/")).map(|index| index + 1)
}

/// Whether `args` ask for the index: `--help` before any command, where clap
/// would answer with the root's help. `-h` is clap's own.
fn asks_for_index(args: &[OsString]) -> bool {
    matches!(first_word(args), Some("--help"))
}

/// The first of `args` after the program that is not the global `--now` and
/// its value: the command, or a flag of the root.
fn first_word(args: &[OsString]) -> Option<&str> {
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--now") => {
                args.next();
            }
            Some(arg) if arg.starts_with("--now=") => {}
            word => return word,
        }
    }
    None
}

/// The command whose text a help clap was asked for answers with: the one
/// `args` name, where they ask for its `--help` and not its `-h` — the
/// one-screen help of its flags, which clap prints.
fn asks_for_text(args: &[OsString]) -> Option<&str> {
    let asked = args.iter().take_while(|arg| *arg != "--");
    let mut long = false;
    for arg in asked {
        match arg.to_str() {
            Some("--help") => long = true,
            Some("-h") => return None,
            _ => {}
        }
    }
    long.then(|| first_word(args)).flatten()
}

/// What `help` prints: the index with no name, the whole manual for `all`,
/// the topic called `name`, or the text of the command called that, as its
/// `--help` prints it. `Err`: what it is refused with.
fn help(name: Option<&str>) -> Result<String, String> {
    let manual = manual();
    let Some(name) = name else {
        return manual.index().map_err(|e| e.to_string());
    };
    let unknown = match manual.topic_help(name) {
        Ok(topic) => return Ok(topic),
        Err(error) => error,
    };
    if cli().find_subcommand(name).is_some() {
        return manual.command_help(&[name]).map_err(|e| e.to_string());
    }
    let topics = unknown.faults().iter().find_map(|fault| match fault {
        agent_help::Fault::UnknownTopic { topics, .. } => Some(topics.clone()),
        _ => None,
    });
    Err(topic::unknown(name, &topics.unwrap_or_default()))
}

fn repo(text: &str) -> Result<RepoName, String> {
    RepoName::read(text).ok_or_else(|| "a repo is named as owner/repo".to_string())
}

fn team_dir(text: &str) -> Result<RepoDir, String> {
    RepoDir::read(text).map_err(|_| {
        "a team is named as its repo or a directory of one: owner/repo or owner/repo/path"
            .to_string()
    })
}

fn declared(text: &str) -> Result<Declared, String> {
    Declared::read(text).ok_or_else(|| {
        let words: Vec<&str> = Declared::ALL.iter().map(|declared| declared.word()).collect();
        format!("what is declared is one of: {}", words.join(", "))
    })
}

/// A stage or consult verdict. A ruling answers an escalation separately.
/// One thing a `blocked` verdict waits on, as `--blocked-by` names it.
#[derive(Debug, Clone, Copy)]
enum Blocker {
    Issue(u64),
    Cut,
}

/// `--blocked-by`'s item: an issue's number — `#3` too — or `cut`.
fn blocker(text: &str) -> Result<Blocker, String> {
    use afkconvoy_core::blocked::CUT;
    let text = text.trim();
    if text == CUT {
        return Ok(Blocker::Cut);
    }
    afkconvoy_core::record::canonical_number(text.strip_prefix('#').unwrap_or(text))
        .map(Blocker::Issue)
        .ok_or_else(|| format!("names neither an issue's number nor `{CUT}`"))
}

/// What `--blocked-by`'s items name together.
fn blocked_on(items: &[Blocker]) -> afkconvoy_core::blocked::BlockedOn {
    let mut on = afkconvoy_core::blocked::BlockedOn::default();
    for item in items {
        match item {
            Blocker::Issue(number) => on.issues.push(*number),
            Blocker::Cut => on.cut = true,
        }
    }
    on
}

fn verdict(text: &str) -> Result<Declared, String> {
    let verdicts = || Declared::ALL.into_iter().filter(|declared| *declared != Declared::Ruling);
    verdicts().find(|declared| declared.word() == text).ok_or_else(|| {
        let words: Vec<&str> = verdicts().map(Declared::word).collect();
        format!("a verdict is one of: {}", words.join(", "))
    })
}

/// An existing kit harness to promote, by its key. The old `config`
/// pseudo-harness is rejected because priority has no independent override.
fn kit_harness(text: &str) -> Result<Harness, String> {
    match text {
        "config" => {
            Err("`--harness config` is gone: switch to a harness already in the kit".into())
        }
        _ => harness(text),
    }
}

fn harness(text: &str) -> Result<Harness, String> {
    Harness::read(text).ok_or_else(|| Harness::unknown(text))
}

/// What a worker that declares ran under, when the commits say one does: a
/// human names none. A worker started before its environment named a harness
/// names none either (`Harness::UNNAMED`).
fn ran(
    role_commit: Option<String>,
    team_commit: Option<String>,
    harness: Option<Harness>,
    model: Option<String>,
) -> Option<stage::Ran> {
    let (role_commit, team_commit) = role_commit.zip(team_commit)?;
    let harness = harness.unwrap_or(Harness::UNNAMED);
    Some(stage::Ran { role_commit, team_commit, harness, model })
}

/// `--slow-gate-lanes` and `--fast-lanes`, boxed: `Run` is the largest
/// command, and clippy bounds it.
fn boxed_lanes(said: &str) -> Result<Box<To<u64>>, String> {
    host_settings::watch_every_or_default(said).map(Box::new)
}

/// `--agent-instructions`, `--agent-skills` and `--agent-guards`, boxed: `Run` is the largest
/// command, and clippy bounds it.
fn boxed_file(said: &str) -> Result<Box<To<PathBuf>>, String> {
    host_settings::file_or_default(said).map(Box::new)
}

/// `--notify-command`, boxed: `Run` is the largest command, and clippy bounds it.
fn boxed_notify_command(said: &str) -> Result<Box<To<String>>, String> {
    notify_command_or_default(said).map(Box::new)
}

/// `--host-name`, boxed: `Run` is the largest command, and clippy bounds it.
fn boxed_host_name(said: &str) -> Result<Box<To<String>>, String> {
    host_settings::host_name_or_default(said).map(Box::new)
}

pub(crate) fn this_host() -> String {
    host_settings::host_name()
}

fn removed_worker_command(_: &str) -> Result<String, String> {
    Err("--worker-command is gone: run `afkconvoy kit` to set this host's worker launch commands"
        .into())
}

impl AgentArgs {
    /// Reads this host's host kit for the checkout `repo` (`Launches::new`), with no probe:
    /// the one worker a command started this way starts is refused where its
    /// harness's command resolves to nothing (`Agent::command`). What that
    /// command warns of is the caller's to say (`Launches::warned`).
    fn agent(self, repo: &Repo) -> anyhow::Result<Agent> {
        let launches = Launches::new(repo, true)?.with_no_proxy(&self.proxy.worker_no_proxy)?;
        Ok(Agent { launches, ready_timeout: self.ready_timeout })
    }

    /// Makes the startup probe of a start that starts workers → what it
    /// starts them with (`start_agent`), its warnings said on stderr.
    fn start(self, repo: &Repo, now: u64, trusting: bool) -> anyhow::Result<Agent> {
        let no_proxy = &self.proxy.worker_no_proxy;
        let (agent, warnings) = start_agent(repo, no_proxy, self.ready_timeout, now, trusting)?;
        probe::announce(&warnings);
        Ok(agent)
    }
}

/// Builds the agent of a start that starts workers, in the checkout `repo`
/// with this host's host kit, and makes its startup probe → the agent, and
/// what the probe and the launch commands it resolved warned of
/// (`Launches::warned`). What each command needs of this host is the
/// probe's to check: for every harness the fleet could be asked for, before
/// any worker is started. A start that is `trusting` makes none while one that
/// checked these commands stands (`probe::standing`).
pub(crate) fn start_agent(
    repo: &Repo,
    worker_no_proxy: &[String],
    ready_timeout: u64,
    now: u64,
    trusting: bool,
) -> anyhow::Result<(Agent, Vec<String>)> {
    let launches = Launches::new(repo, true)?.with_no_proxy(worker_no_proxy)?;
    let agent = Agent { launches, ready_timeout };
    // The default harness's is settled whatever stands: a human is
    // asked, where one must be, before anything else is.
    agent.launches.key()?;
    let stands = if trusting { probe::standing(repo, now, &agent.launches) } else { None };
    let probed = match stands {
        Some(warnings) => warnings,
        None => probe::startup(repo, now, &agent.launches)?,
    };
    let mut warnings = agent.launches.warned();
    warnings.extend(probed);
    Ok((agent, warnings))
}

/// What `run` asks for, from its flags and the global `--now` → whether it
/// was `--foreground`, its start, and what it named of the host settings;
/// `None` for any other command. One mapping for `run` and for a host runner
/// that starts a checkout again.
fn start_of(
    command: Command,
    now: Option<u64>,
) -> Option<(bool, instance::Start, host_settings::Named)> {
    let Command::Run {
        instance,
        host,
        concurrency,
        tracker,
        harness,
        agent,
        panel,
        watch_dir,
        watch_every,
        hot_cores,
        hot_memory,
        hot_written,
        admit_load,
        admit_memory,
        admit_disk,
        admit_inspect_after,
        exclusive_seconds,
        jev_key_command,
        notify_command,
        agent_instructions,
        agent_skills,
        agent_guards,
        host_name,
        gate_lanes,
        slow_gate_lanes,
        fast_lanes,
        ticks,
        second_ms,
        panic_at_tick,
        foreground,
        ..
    } = command
    else {
        return None;
    };
    let named = instance::Named {
        instance: instance.map(|instance| instance.as_str().to_string()),
        host,
        worker_no_proxy: agent.proxy.worker_no_proxy,
    };
    let settings = host_settings::Named {
        hot_cores,
        hot_memory,
        hot_written,
        admit_load,
        admit_memory,
        admit_disk,
        admit_inspect_after,
        exclusive_seconds,
        gate_lanes,
        slow_gate_lanes: slow_gate_lanes.map(|lanes| *lanes),
        fast_lanes: fast_lanes.map(|lanes| *lanes),
        panel,
        watch_dir,
        watch_every,
        jev_key_command,
        notify_command: notify_command.map(|command| *command),
        host_name: host_name.map(|name| *name),
        agent_instructions: agent_instructions.map(|file| *file),
        agent_skills: agent_skills.map(|file| *file),
        agent_guards: agent_guards.map(|file| *file),
    };
    let ready_timeout = agent.ready_timeout;
    let panic_at = panic_at_tick;
    let knobs = instance::Knobs { ticks, second_ms, ready_timeout, now, panic_at };
    Some((foreground, instance::Start { named, concurrency, tracker, harness, knobs }, settings))
}

/// The start of a `run` that named nothing, in this process's environment:
/// how a host runner starts a checkout on its list, with what that checkout's
/// `run` named laid over it. What its environment names of the host settings
/// is no start's: a `run` names those.
pub(crate) fn unflagged_start() -> anyhow::Result<instance::Start> {
    // Only a variable can refuse it: clap's first line says which flag, and why.
    let unflagged = Cli::try_parse_from(["afkconvoy", "run"]).map_err(|error| {
        let said = error.to_string();
        let said = said.lines().next().unwrap_or_default();
        anyhow::anyhow!(
            "the login shell's environment sets a flag of `run` to what it refuses — correct it \
             in your login shell's rc files: {}",
            said.trim_start_matches("error: ")
        )
    })?;
    let start = start_of(unflagged.command, None).map(|(_, start, _)| start);
    start.context("a `run` asks for a start")
}

/// A workflow is not named by a number: one given to `show` is an issue's.
fn shown_issue(workflow: Option<&str>, issue: Option<u64>) -> Option<u64> {
    issue.or_else(|| workflow.and_then(|named| named.parse().ok()))
}

/// A copyable correction when naming an existing directory with `-C` makes
/// an otherwise refused `show` valid. Reparse with the real argument schema:
/// a directory that is a flag's value, or another fault, is no checkout hint.
fn show_checkout_hint(error: &clap::Error, args: &[OsString]) -> Option<String> {
    let directory = match error.kind() {
        clap::error::ErrorKind::UnknownArgument => {
            error.get(clap::error::ContextKind::InvalidArg)?.to_string()
        }
        clap::error::ErrorKind::ArgumentConflict => {
            let parsed = Cli::command().ignore_errors(true).try_get_matches_from(args).ok()?;
            parsed.subcommand_matches("show")?.get_one::<String>("workflow")?.clone()
        }
        _ => return None,
    };
    if !PathBuf::from(&directory).is_dir() {
        return None;
    }
    for (index, arg) in args.iter().enumerate().skip(1) {
        if arg != directory.as_str() {
            continue;
        }
        let mut corrected = args.to_vec();
        corrected.insert(index, OsString::from("-C"));
        let Ok(Cli { command: Command::Show { workflow, issue, team, at }, now }) =
            Cli::try_parse_from(corrected)
        else {
            continue;
        };
        let checkout = at.checkout?;
        let mut line = Line::new().fixed("afkconvoy show -C ").word(&checkout.to_string_lossy());
        match shown_issue(workflow.as_deref(), issue) {
            Some(issue) => line = line.fixed(" --issue ").number(issue),
            None => {
                if let Some(workflow) = workflow {
                    line = line.fixed(" ").word(&workflow);
                }
            }
        }
        if let Some(dir) = team.dir {
            line = line.fixed(" --dir ").word(&dir.to_string_lossy());
        }
        if let Some(team) = team.team {
            line = line.fixed(" --team ").word(&team.to_string());
        }
        if let Some(now) = now {
            line = line.fixed(" --now ").number(now);
        }
        return Some(format!("show names its checkout with -C: {}", line.own()));
    }
    None
}

/// Runs one command line. Prints exactly one JSON object on stdout: the
/// command's answer, or `{"error": …}` with a non-zero exit — `doctor` exits
/// non-zero with its answer, when that holds a critical finding. An answer that
/// is text — a drawing, the skill file — is printed as it is. `guard` answers the
/// harness that asks it, in that harness's way: for Claude Code, a hook's,
/// with its exit code and a reason on stderr.
pub fn main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    tools::reach_orca();
    let args: Vec<OsString> = args.into_iter().collect();
    if asks_for_index(&args) {
        return match help(None) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => fail(1, &error),
        };
    }
    let parsed = operators(cli())
        .try_get_matches_from(&args)
        .and_then(|matches| Ok((Cli::from_arg_matches(&matches)?, matches)));
    let (cli, matches) = match parsed {
        Ok(parsed) => parsed,
        // A command's `--help` is its text in the manual.
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            let text = asks_for_text(&args).and_then(|name| help(Some(name)).ok());
            match text {
                Some(text) => {
                    print!("{text}");
                    return ExitCode::SUCCESS;
                }
                None => e.exit(),
            }
        }
        // `-h` and `--version` are asked for by a human: clap prints them.
        Err(e) if !e.use_stderr() => e.exit(),
        Err(e) => {
            let mut said = e.render().to_string().trim().to_string();
            if let Some(hint) = show_checkout_hint(&e, &args) {
                said.push_str(&format!("\n\n{hint}"));
            }
            return fail(2, &said);
        }
    };
    if let Command::Guard { terminal_input_only } = cli.command {
        return guard::guard(terminal_input_only);
    }
    if let Command::Help { name } = &cli.command {
        return match help(name.as_deref()) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => fail(2, &error),
        };
    }
    // The checkout the command is about, found once, here, and handed to all
    // it does. One about no repo is about none.
    let about = matches.subcommand().filter(|(name, _)| {
        Cli::command()
            .find_subcommand(name)
            .is_some_and(|command| command.get_arguments().any(|arg| arg.get_id() == CHECKOUT_ARG))
    });
    let named = about.and_then(|(_, command)| command.get_one::<PathBuf>(CHECKOUT_ARG));
    let named = named.map(PathBuf::as_path);
    let kind = matches.subcommand_name().map_or(Kind::PLAIN, kind);
    let checkout = match about {
        Some((_, given)) => {
            let taken = kind.takes_worktree(given);
            match kind.starts {
                true => checkout::started(named, taken),
                false => checkout::resolve(named, taken),
            }
        }
        None => Err(anyhow::anyhow!("this command is about no repo")),
    };
    // Run anywhere, in a checkout or not: the queue is the host's. A
    // directory named is refused before it waits where it is no checkout's.
    if let Command::Exclusive { command, .. } = &cli.command {
        return match (checkout, checkout::dir(named)) {
            (Err(error), _) if named.is_some() => exclusive::failed(&format!("{error:#}")),
            (checkout, Ok(dir)) => {
                exclusive::exclusive(command, &dir, checkout.ok(), || cli.now.unwrap_or_else(clock))
            }
            (_, Err(error)) => exclusive::failed(&format!("{error:#}")),
        };
    }
    // A checkout named is one the command is about, whether it needs one or
    // not: where it cannot be had, the command is refused before it starts. A
    // wake is about the checkout it is run in, named or not: in none, it has
    // no fleet to wake and no tick log to say so in.
    let ran = match checkout {
        Err(error) if named.is_some() || matches!(cli.command, Command::Wake { .. }) => Err(error),
        checkout => {
            let in_it = checkout.as_ref().ok().cloned();
            let acting = kind.acts.then(|| cli.command.acting()).flatten();
            let acting = acting.map(|acting| acting.in_checkout(in_it));
            let now = || cli.now.unwrap_or_else(clock);
            if let Some(acting) = &acting {
                acting.start(now());
            }
            let ran = run(cli.command, checkout, cli.now);
            match &acting {
                Some(acting) => acting.end(now(), ran),
                None => ran,
            }
        }
    };
    match ran {
        Ok(Value::String(drawing)) => {
            print!("{drawing}");
            ExitCode::SUCCESS
        }
        Ok(answer) => {
            println!("{answer}");
            if answer["added"] == true
                && let Some(harness) = answer["harness"].as_str().and_then(Harness::read)
                && let Err(error) = kit::refresh_after_answer(harness)
            {
                eprintln!(
                    "warning: the added harness could not be refreshed in the background: {error:#}"
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if let Some(doctor::Unwell(answer)) = e.downcast_ref() {
                // Not an error to report: the answer itself, which says what is wrong.
                println!("{answer}");
            } else if let Some(host_runner::Answered(answer)) = e.downcast_ref() {
                // The host runner's own words, with which host runner said them.
                println!("{answer}");
            } else {
                println!("{}", error_json(&e));
            }
            ExitCode::from(1)
        }
    }
}

/// What a command prints for an error: {"error", "faults"} of a team's
/// refusal, {"error", "see"} where a topic explains it, else {"error"}.
pub(crate) fn error_json(e: &anyhow::Error) -> Value {
    if let Some(refused) = e.downcast_ref::<team::Refused>() {
        json!({ "error": refused.to_string(), "faults": refused.json() })
    } else if let Some(refused) = e.downcast_ref::<first_start::Refused>() {
        json!({ "error": format!("{e:#}"), "asks": refused.asks, "see": topic::See::FirstStart })
    } else if let Some(see) = topic::explains(e) {
        json!({ "error": format!("{e:#}"), "see": see })
    } else {
        json!({ "error": format!("{e:#}") })
    }
}

fn fail(code: u8, message: &str) -> ExitCode {
    println!("{}", json!({ "error": message }));
    ExitCode::from(code)
}

fn clock() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// A name no other thread of this process makes: the pid and a counter. A
/// file or a directory named with it is one thread's own, however many fleet
/// instances work in the process.
pub(crate) fn unique() -> String {
    static MADE: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), MADE.fetch_add(1, Ordering::Relaxed))
}

impl Command {
    /// What it says of itself as it acts (`acting`), if it changes fleet
    /// state: writes to GitHub — issues, comments, labels, blocked-by edges —
    /// or to the fleet's refs on the remote. One that only reads says nothing.
    /// Asked only of a command whose kind acts (`kind`).
    fn acting(&self) -> Option<Acting> {
        let of = |name, issue: &IssueArg, who: Option<&Who>| {
            let acting = Acting::new(name).on_issue(Some(issue.number()));
            match who {
                Some(who) => acting.as_instance(who.instance.as_str()),
                None => acting,
            }
        };
        let as_who = |name, who: &Who| Acting::new(name).as_instance(who.instance.as_str());
        Some(match self {
            Command::Dispatch { issue, who, .. } => of("dispatch", issue, Some(who)),
            Command::Close { issue, who, .. } => of("close", issue, Some(who)),
            Command::Escalate { issue, who, .. } => of("escalate", issue, Some(who)),
            Command::Park { issue, who, .. } => of("park", issue, Some(who)),
            Command::Nudge { issue, who, .. } => of("nudge", issue, Some(who)),
            Command::Tell { issue, .. } => of("tell", issue, None),
            Command::Restart { issue, who, .. } => of("restart", issue, Some(who)),
            Command::Switch { issue, patrol, all, .. } => {
                let acting = Acting::new("switch");
                match (issue, patrol, all) {
                    (Some(issue), _, _) => acting.on_issue(Some(*issue)),
                    (None, Some(patrol), _) => acting.on(format!("patrol {}", patrol.trim())),
                    (None, None, true) => acting.on("all".to_string()),
                    (None, None, false) => acting,
                }
            }
            Command::Takeover { list: true, .. } => return None,
            Command::Takeover { from, instance, .. } => {
                let acting = Acting::new("takeover");
                let acting = match from {
                    Some(from) => acting.on(format!("from {from}")),
                    None => acting,
                };
                match instance {
                    Some(instance) => acting.as_instance(instance.as_str()),
                    None => acting,
                }
            }
            Command::Claim { issue, who, .. } => of("claim", issue, Some(who)),
            Command::Release { issue, who, .. } => of("release", issue, Some(who)),
            Command::Heartbeat { who, .. } => as_who("heartbeat", who),
            Command::Rule { issue, .. } => of("rule", issue, None),
            Command::Send { issue, .. } => of("send", issue, None),
            Command::Land { watch: true, .. } => match std::env::var(WATCH_ENV) {
                Ok(worker) => Acting::new("land").on(worker),
                Err(_) => Acting::new("land"),
            },
            Command::Land { number, issue, .. } => Acting::new("land").on_issue(issue.or(*number)),
            Command::Verdict { issue, .. } => of("verdict", issue, None),
            Command::Intake { issue, .. } => of("intake", issue, None),
            Command::Target { issue, branch: Some(_), .. } => of("target", issue, None),
            Command::Target { branch: None, .. } => return None,
            Command::Delivery { handoff: true, .. } => {
                Acting::new("delivery").on("handoff".to_string())
            }
            Command::Delivery { final_target: Some(_), issue, .. } => {
                Acting::new("delivery").on_issue(*issue)
            }
            Command::Delivery { final_target: None, .. } => return None,
            Command::Lock { issue, .. } => of("lock", issue, None),
            Command::PatrolNow { name, .. } => {
                Acting::new("patrol-now").on(name.trim().to_string())
            }
            Command::Over { run, .. } => Acting::new("over").on(format!("run {}", run.trim())),
            Command::Gate { .. } => Acting::new("gate").on_own_issue().saying_start(),
            Command::Cut { .. } => Acting::new("cut").saying_start(),
            Command::Wake { .. } => Acting::new("wake").on_own_issue(),
            Command::Cast { .. } => Acting::new("cast"),
            Command::Tick { who, .. } => as_who("tick", who),
            // Its kind says it does not act (`kind`).
            _ => return None,
        })
    }
}

/// Runs `command` about the checkout `checkout`, found where it started: an
/// error where it is about none, or the one it would be about could not be had
/// — which a command that needs none passes over.
fn run(
    command: Command,
    checkout: anyhow::Result<Repo>,
    fixed_now: Option<u64>,
) -> anyhow::Result<Value> {
    let now = fixed_now.unwrap_or_else(clock);
    match command {
        Command::Asked { question, text_file } => asking_report::record(&question, &text_file),
        Command::Watched { watch, text_file } => watch_report::record(&watch, &text_file, now),
        Command::Kit { cast: Some(_), .. } => bail!("automatic casting is gone"),
        Command::Kit { remove: Some(harness), .. } => kit::remove(harness, now),
        Command::Kit { research: Some(file), .. } => kit::apply_research(&file, now),
        Command::Kit { harness: Some(harness), .. } => kit::write_one(harness, now),
        Command::Kit { .. } => kit::write(now),
        Command::Cast { .. } => bail!("automatic casting is gone"),
        Command::Config { file, .. } => {
            let file = match file {
                Some(file) => file,
                None => config::file_of(&checkout?),
            };
            Ok(serde_json::to_value(config::load(&file)?)?)
        }
        Command::Check { team, .. } => {
            let (from, checkout) = team.read(checkout)?;
            team::check(from, checkout.as_ref())
        }
        Command::Scaffold { what, dir, role, example, repo, public } => {
            let create = repo.map(|repo| scaffold::Create { repo, public });
            // The parser took the name for one there is an example of.
            let example = example.as_deref().and_then(scaffold::example);
            scaffold::scaffold(what, &dir, &role, example, create.as_ref())
        }
        Command::Accounts { days, all, rows, .. } => {
            accounts::accounts(&checkout?, days.unwrap_or(7), all, rows, now)
        }
        Command::RewriteAccounts { backup, .. } => {
            rewrite_accounts::run(&checkout?, backup.as_deref(), now)
        }
        Command::Flows { delivery, tracker, .. } => {
            flows::flows(&checkout?, delivery.as_deref(), tracker.as_ref())
        }
        Command::Daybook { distill: true, .. } => host_watch::ask_distillation(now),
        Command::Daybook { distilled: Some(file), repo, tracker, .. } => {
            daybook::distilled(checkout, repo, tracker, &file)
        }
        Command::Daybook { lessons: true, since, until, tracker, .. } => daybook::lessons(
            checkout,
            tracker,
            since.as_deref().unwrap_or_default(),
            until.as_deref(),
        ),
        Command::Daybook { text, did, why, undo, lesson, links, repo, tracker, .. } => {
            let said = daybook::Said { did: did.or(text), why, undo, lesson, links };
            daybook::daybook(checkout, daybook::Asked { said, repo, tracker })
        }
        Command::Skill => Ok(Value::String(skill::SKILL.to_string())),
        Command::Draft { dir, team, yes } => draft::draft(dir, team, yes),
        Command::Show { workflow, issue, team, .. } => {
            match shown_issue(workflow.as_deref(), issue) {
                // An issue is of the checkout's repo, whichever team reads it.
                Some(issue) => {
                    let repo = checkout?;
                    let from = team.from();
                    // An unbound repo has no team: its issues take its one path.
                    let unbound = matches!(from, team::From::Config)
                        && config::load(&config::file_of(&repo))?.bound.is_none();
                    let mut answer = match unbound {
                        true => unbound_issue::show(&repo, now, issue)?,
                        false => {
                            let loaded = team::load(from, Some(&repo))?;
                            let gh = gh::GitHub::of(&repo)?;
                            stage::show_issue(&gh, &loaded.team, issue)?
                        }
                    };
                    let target = target::select(&repo, issue, None)?;
                    answer["target_branch"] = target["target_branch"].clone();
                    answer["delivery"] = target["delivery"].clone();
                    Ok(answer)
                }
                None => {
                    let (from, checkout) = team.read(checkout)?;
                    let loaded = team::load(from, checkout.as_ref())?;
                    Ok(Value::String(workflow::show(&loaded, workflow.as_deref())?))
                }
            }
        }
        Command::Rehearse { workflow, declared, team, .. } => {
            let (from, checkout) = team.read(checkout)?;
            workflow::rehearse(&team::load(from, checkout.as_ref())?, &workflow, &declared)
        }
        Command::Doctor { all: true, host, proxy, since, rounds, .. } => {
            doctor::all_since_rounds(now, host.as_deref(), &proxy.worker_no_proxy, since, rounds)
        }
        Command::Doctor { host, proxy, rounds, .. } => doctor::doctor_with_proxy_rounds(
            &checkout?,
            now,
            &host.unwrap_or_else(this_host),
            &proxy.worker_no_proxy,
            rounds,
        ),
        Command::Plan { instance, proxy, .. } => {
            plan::plan(&checkout?, now, instance.as_ref(), &proxy.worker_no_proxy)
        }
        Command::Run { .. } => {
            let Some((foreground, start, settings)) = start_of(command, fixed_now) else {
                unreachable!()
            };
            let checkout = checkout?;
            let start = first_start::settled(&checkout, start, &settings)?;
            match foreground {
                true => host_runner::foreground(&checkout, &start, settings),
                false => host_runner::run(&checkout, start, &settings, now),
            }
        }
        Command::Stop { all: true, .. } => host_runner::stop_all(),
        Command::Stop { release: true, instance, .. } => {
            stop_release::stop_release(&checkout?, instance, now)
        }
        Command::Stop { .. } => host_runner::stop(&checkout?),
        Command::HostRunner => host_runner::serve(),
        Command::Checkouts => checkouts::checkouts(),
        Command::Queue => queue::queue(fixed_now),
        Command::Panel { open: true, .. } => host_runner::panel(),
        Command::Panel { instance, host, at, .. } => {
            let host = host.unwrap_or_else(this_host);
            let named = at.checkout.is_some();
            let page = panel::printed(checkout, named, instance.as_ref(), &host, now)?;
            Ok(Value::String(panel::render(&page)))
        }
        Command::Tick { who, agent, .. } => {
            let repo = checkout?;
            let agent = agent.start(&repo, now, true)?;
            let me = who.me(now);
            let ticked = tick::tick(&repo, &me, &agent, &stop::Stop::never(), None);
            probe::announce(&agent.launches.warned());
            let (ticked, _) = ticked?;
            ticked.log_cut_said(&repo, &me);
            Ok(ticked.json())
        }
        Command::Gate { land, fast, slow, host, .. } => {
            let repo = checkout?;
            claims::own_issue_on_this_host(&repo)?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let host = host.unwrap_or_else(this_host);
            match (fast, slow) {
                (true, _) => gate::one(&repo, &config, afkconvoy_core::config::Which::Fast),
                (_, true) => gate::one(&repo, &config, afkconvoy_core::config::Which::Slow),
                _ => gate::gate(&repo, &config, land, &host, now),
            }
        }
        Command::Land { number, issue, watch, merged_timeout, host, .. } => {
            let repo = checkout?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub::of(&repo)?;
            let host = host.unwrap_or_else(this_host);
            match issue.or(number) {
                Some(issue) => turn::land(&repo, &config, &gh, issue, &host, now),
                None if watch => watch_landing::land(&repo, &config, &gh, &host, now),
                None => {
                    let _ = merged_timeout;
                    legacy::migrate(&repo, &config, &gh, &host, now)
                }
            }
        }
        Command::Verdict {
            verdict,
            issue,
            stage,
            text,
            blocked_by,
            role_commit,
            team_commit,
            harness,
            model,
            consult,
            ..
        } => {
            let declaring = stage::Declaring {
                issue: issue.number(),
                stage,
                declared: verdict,
                blocked_by: blocked_on(&blocked_by),
                text: text.read()?,
                ran: ran(role_commit, team_commit, harness, model),
                consult,
            };
            stage::verdict(&checkout?, declaring)
        }
        Command::Rule { issue, text, stage, role_commit, team_commit, harness, model, .. } => {
            let repo = checkout?;
            let issue = issue.number();
            // With the commits it ran under, the ruling is a consult's: a
            // stage record under its claim, as a verdict is.
            if let Some(ran) = ran(role_commit, team_commit, harness, model) {
                let Some(stage) = stage else {
                    anyhow::bail!(
                        "a consult rules with `--stage`, the stage that asked it: its brief \
                         gives the line. A human rules with no commit named and none in the \
                         environment"
                    );
                };
                let declaring = stage::Declaring {
                    issue,
                    stage,
                    declared: Declared::Ruling,
                    blocked_by: Default::default(),
                    text: text.read()?,
                    ran: Some(ran),
                    consult: true,
                };
                return stage::verdict(&repo, declaring);
            }
            let team = team::load(team::From::Config, Some(&repo))?.team;
            steer::rule(&repo, &team, issue, text.read()?)
        }
        Command::Send {
            issue,
            stage,
            text,
            role_commit,
            team_commit,
            harness,
            model,
            consult,
            ..
        } => {
            let repo = checkout?;
            let issue = issue.number();
            let team = team::load(team::From::Config, Some(&repo))?.team;
            // With the commits it ran under, the sending is a consult's: a
            // stage record under its claim, as its ruling is.
            if let Some(ran) = ran(role_commit, team_commit, harness, model) {
                if !consult {
                    anyhow::bail!(
                        "a worker sends an issue to a stage only as its stage's consult: a \
                         stage declares its verdict, and the workflow says where it leads"
                    );
                }
                return stage::consult_send(&repo, &team, issue, &stage, text.read()?, ran);
            }
            steer::send(&repo, &team, issue, &stage, text.read()?)
        }
        Command::Intake {
            issue,
            workflow,
            delivery,
            role,
            from,
            patrol,
            repair,
            blocked_by,
            ..
        } => {
            use afkconvoy_core::intake::{By, On};
            let on = match (from, patrol) {
                (Some(issue), _) => Some(On::Issue(issue)),
                (None, Some(patrol)) => Some(On::Patrol(patrol)),
                (None, None) => None,
            };
            let by = match (repair, role, on) {
                (true, None, None) => Some(By::Repair),
                (true, ..) => anyhow::bail!(
                    "the repair worker has no role and is on no issue and no patrol: `--repair` \
                     takes none of them ({ROLE_ENV}, {ISSUE_ENV} and {PATROL_ENV} unset)"
                ),
                (false, Some(role), Some(on)) => Some(By::Role { role, on }),
                (false, None, _) => None,
                (false, Some(_), None) => anyhow::bail!(
                    "an intake names what its worker is on: the issue (--from, or {ISSUE_ENV} in \
                     the environment) or the patrol (--patrol, or {PATROL_ENV})"
                ),
            };
            let taking =
                intake::Taking { issue: issue.number(), workflow, by, blocked_by, delivery, from };
            intake::intake(&checkout?, taking)
        }
        Command::Target { issue, branch, .. } => {
            target::select(&checkout?, issue.number(), branch.as_deref())
        }
        Command::Delivery { jira, final_target, issue, handoff, .. } => {
            delivery::command(&checkout?, &jira, final_target.as_deref(), issue, handoff, now)
        }
        Command::Lock { issue, role, .. } => {
            let repo = checkout?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub::of(&repo)?;
            let role = role.as_deref().map(str::trim).filter(|role| !role.is_empty());
            lock::lock(&config, &gh, issue.number(), role)
        }
        Command::PatrolNow { name, .. } => patrol::ask(&checkout?, name.trim(), &this_host(), now),
        Command::Over { run, .. } => {
            if let Ok(issue) = std::env::var(ISSUE_ENV) {
                anyhow::bail!(
                    "this worker is on issue #{issue} ({ISSUE_ENV}), whose stage ends with \
                     `afkconvoy verdict`: only a patrol's run are declared over"
                );
            }
            claims::declare_over(&checkout?, run.trim())
        }
        Command::Cut { from, host, .. } => {
            let repo = checkout?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let host = host.unwrap_or_else(this_host);
            cut::run(&repo, &config, from, &host, clock)
        }
        // `main` answers it: a hook's answer is no JSON object.
        Command::Guard { .. } => anyhow::bail!("guard answers with its exit code"),
        Command::Exclusive { .. } => {
            anyhow::bail!("exclusive answers with its command's exit code")
        }
        Command::Help { .. } => anyhow::bail!("help answers with Markdown"),
        Command::Dispatch { issue, who, agent, start, .. } => {
            let repo = checkout?;
            let agent = agent.agent(&repo)?;
            let mut fleet = Fleet::open(&repo, now)?;
            let found = &mut found::Found::default();
            let me = who.me(now);
            let dispatched =
                dispatch::dispatch(&mut fleet, found, &me, &agent, issue.number(), start);
            probe::announce(&agent.launches.warned());
            dispatched.map(|dispatched| dispatched.answer)
        }
        Command::Recovery { issue, .. } => {
            recovery::recovery(&Fleet::open(&checkout?, now)?, issue.number())
        }
        Command::Close { issue, who, .. } => {
            judge::close(&mut Fleet::open(&checkout?, now)?, &who.me(now), issue.number())
        }
        Command::Escalate { issue, who, reason, .. } => {
            let mut fleet = Fleet::open(&checkout?, now)?;
            let found = &mut found::Found::default();
            judge::escalate(&mut fleet, found, &who.me(now), issue.number(), &reason)
        }
        Command::Park { issue, who, .. } => {
            park::park(&mut Fleet::open(&checkout?, now)?, &who.me(now), issue.number())
        }
        Command::Nudge { issue, who, .. } => {
            judge::nudge(&mut Fleet::open(&checkout?, now)?, &who.me(now), issue.number())
        }
        Command::Tell { issue, text, file, host, .. } => {
            let text = match (text, file) {
                (Some(text), _) => text,
                (None, Some(file)) => std::fs::read_to_string(&file)
                    .with_context(|| format!("cannot read the note in {}", file.display()))?,
                (None, None) => anyhow::bail!("tell takes --text or --file"),
            };
            let host = host.unwrap_or_else(this_host);
            tell::tell(&Fleet::open(&checkout?, now)?, &host, issue.number(), &text)
        }
        Command::Restart { issue, who, agent, .. } => {
            let repo = checkout?;
            let agent = agent.agent(&repo)?;
            let restarted =
                watch::restart(&mut Fleet::open(&repo, now)?, &who.me(now), &agent, issue.number());
            probe::announce(&agent.launches.warned());
            restarted
        }
        Command::Switch { harness, model, issue, patrol, all, instance, host, agent, .. } => {
            let target = kit::switch_target(harness, model.as_deref())?;
            let which = match (issue, patrol, all) {
                (Some(issue), _, _) => switch::Which::Issue(issue),
                (None, Some(patrol), _) => switch::Which::Patrol(patrol.trim().to_string()),
                (None, None, true) => switch::Which::All,
                _ => switch::Which::Nothing,
            };
            switch::on_host(
                checkout.ok(),
                target,
                which,
                instance,
                &host.unwrap_or_else(this_host),
                now,
                |repo| agent.clone().agent(repo),
            )
        }
        Command::Takeover { list, from, yes, instance, host, .. } => {
            let mut fleet = Fleet::open(&checkout?, now)?;
            let me =
                instance.map(|instance| Me { instance, host: host.unwrap_or_else(this_host), now });
            match (list, from, me) {
                (true, _, me) => Ok(takeover::list(&fleet, me.as_ref())),
                (false, Some(from), Some(me)) => takeover::takeover(&mut fleet, &me, &from, yes),
                (false, Some(_), None) => {
                    anyhow::bail!("takeover --from needs --instance: the fleet to take them as")
                }
                (false, None, _) => anyhow::bail!("takeover takes --list, or --from <instance id>"),
            }
        }
        // `Acting::end` sends it, as it does for every command that acts.
        Command::Wake { .. } => Ok(json!({})),
        Command::Scan { .. } => Ok(Scan::read_said(&checkout?)?.json()),
        Command::Claim { issue, who, .. } => {
            let repo = checkout?;
            claims::take(&repo, &mut Scan::read(&repo)?, &who.me(now), issue.number(), Take::Fresh)
        }
        Command::Release { issue, who, .. } => {
            let repo = checkout?;
            claims::release(&repo, &mut Scan::read_said(&repo)?, &who.instance, issue.number())
        }
        Command::Heartbeat { who, .. } => {
            let repo = checkout?;
            claims::beat(&repo, &mut Scan::read(&repo)?, &who.me(now))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manual is whole: every command has its text and its group, and
    /// every placeholder and every link to a topic names one there is.
    #[test]
    fn a_link_on_the_path_is_the_file_it_points_at_and_another_file_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("afkconvoy-local");
        std::fs::write(&exe, "").unwrap();
        let link = dir.path().join("afkconvoy");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        assert!(same_file(&link, &exe));
        let other = dir.path().join("afkconvoy-old");
        std::fs::write(&other, "").unwrap();
        assert!(!same_file(&other, &exe));
        assert!(!same_file(&dir.path().join("gone"), &exe));
    }

    /// A command's doc comment here is its one line; the rest of its text
    /// is its module's, which `cli` puts after that line. A doc comment that
    /// went on would print its line twice.
    #[test]
    fn a_commands_text_after_its_one_line_is_its_modules() {
        for command in Cli::command().get_subcommands() {
            let (Some(about), Some(rest)) = (command.get_about(), command.get_long_about()) else {
                continue;
            };
            let name = command.get_name();
            assert!(
                !rest.to_string().starts_with(&about.to_string()),
                "the doc comment of `{name}` holds more than its one line: the rest is the \
                 module's, named by its `long_about`"
            );
        }
    }

    #[test]
    fn the_manual_is_whole() {
        agent_help::check(&manual()).unwrap_or_else(|faults| panic!("{faults}"));
    }

    /// The help never names `docs/adr/`, a document its reader cannot open.
    /// The sentence that held the citation still states the rule; the path
    /// does not.
    #[test]
    fn the_help_never_names_a_closed_document() {
        let cited = "so a repo config names none (`docs/adr/0025`).\nThe rest stays.\n";
        assert_eq!(names_closed_adr(cited), Some(1));
        assert_eq!(names_closed_adr("so a repo config names none.\n"), None);
        let later = "The rule.\nSee docs/adr/0027.\n";
        assert_eq!(names_closed_adr(later), Some(2));
        let rendered = manual().manual().unwrap();
        assert_eq!(
            names_closed_adr(&rendered),
            None,
            "help names a document the reader cannot open, at line {}",
            names_closed_adr(&rendered).unwrap_or(0)
        );
    }

    /// A command whose kind acts logs what it acts on, as its wake names it;
    /// one given the flag that makes it only read says nothing.
    #[test]
    fn a_command_that_acts_names_what_it_acts_on() {
        let acting = |line: &[&str]| {
            let cli = Cli::try_parse_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            let command = cli.unwrap().command;
            kind(line[0]).acts.then(|| command.acting()).flatten().map(|acting| acting.named())
        };
        assert_eq!(acting(&["takeover", "--list"]), None, "the listing changes nothing");
        assert_eq!(acting(&["target", "7"]), None);
        assert_eq!(acting(&["doctor"]), None, "its kind does not act");
        let named = |line: &[&str]| acting(line).unwrap();
        assert_eq!(named(&["park", "--issue", "46", "--instance", "a-h1-1"]), "park #46");
        assert_eq!(named(&["over", "--run", " sweep "]), "over run sweep");
        assert_eq!(named(&["patrol-now", "sweep"]), "patrol-now sweep");
        assert_eq!(
            named(&["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"]),
            "takeover from b-h2-2"
        );
        assert_eq!(named(&["land", "--train"]), "land");
        assert_eq!(named(&["cast"]), "cast");
        let manual = manual();
        let help = |name: &str| manual.command_help(&[name]).unwrap();
        assert!(help("park").contains("\n## What it logs\n"));
        assert!(!help("doctor").contains("\n## What it logs\n"));
    }

    /// A worker's worktree is taken for a command a worker runs, and for one
    /// that only reads but without the flag that makes it act.
    #[test]
    fn a_workers_worktree_is_taken_for_a_command_a_worker_runs_or_one_that_only_reads() {
        let taken = |line: &[&str]| {
            let matches = cli()
                .try_get_matches_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            let matches = matches.unwrap();
            let (name, given) = matches.subcommand().unwrap();
            kind(name).takes_worktree(given)
        };
        assert!(taken(&["takeover", "--list"]));
        assert!(!taken(&["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"]));
        assert!(taken(&["panel"]));
        assert!(taken(&["panel", "--open"]), "it asks the host runner, changing nothing");
        assert!(taken(&["scan"]));
        assert!(taken(&["gate"]));
        assert!(taken(&["wake"]), "a worker's last command");
        assert!(!taken(&["claim", "7", "--instance", "a-h1-1"]));
        assert!(!taken(&["run"]));
        // The watch and asking session's host-wide reports are about no repo.
        let cli = Cli::command();
        let workers = cli.get_subcommands().filter(|c| c.get_next_help_heading() == Some(WORKER));
        for command in workers.filter(|command| !["watched", "asked"].contains(&command.get_name()))
        {
            let name = command.get_name();
            assert_eq!(kind(name).worktree, Worktree::Its, "a worker runs `{name}`");
        }
    }

    #[test]
    fn no_two_threads_make_one_name() {
        let threads: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..100).map(|_| unique()).collect::<Vec<_>>()))
            .collect();
        let made: Vec<String> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
        let distinct: std::collections::BTreeSet<&String> = made.iter().collect();
        assert_eq!(distinct.len(), 800, "{made:?}");
        let pid = format!("{}-", std::process::id());
        assert!(made.iter().all(|name| name.starts_with(&pid)), "{made:?}");
    }

    #[test]
    fn a_run_names_of_the_host_settings_each_flag_it_was_given() {
        let asked = |args: &[&str]| {
            let line = [&["afkconvoy", "run"][..], args].concat();
            let (_, start, named) =
                start_of(Cli::try_parse_from(line).unwrap().command, None).unwrap();
            (start, named)
        };
        let line = ["--hot-cores", "2.5", "--admit-disk", "0", "--admit-load", "default"];
        let (start, named) = asked(&[&line[..], &["--panel", "7070"]].concat());
        assert_eq!(named.hot_cores, Some(To::Value(2500)));
        assert_eq!(named.admit_disk, Some(To::Value(0)), "0 never holds for disk");
        assert_eq!(named.admit_load, Some(To::Default));
        assert_eq!(named.panel, Some(To::Value(7070)));
        // a host setting alone: nothing of the checkout's own start
        assert_eq!(start, asked(&line).0);
        assert_eq!(asked(&["--panel", "default"]).1.panel, Some(To::Default));
        assert!(Cli::try_parse_from(["afkconvoy", "run", "--hot-cores", "0"]).is_err());
        assert!(Cli::try_parse_from(["afkconvoy", "run", "--admit-load", "5%"]).is_err());
        assert!(asked(&[]).1.is_empty(), "named none, it changes none");
        // host sleep is gone: its flags are no flags (docs/adr/0050)
        for gone in ["--no-host-sleep", "--host-sleep", "--host-sleep-after"] {
            assert!(Cli::try_parse_from(["afkconvoy", "run", gone]).is_err(), "{gone}");
        }
        let gone = ["afkconvoy", "run", "--host-sleep-hours", "22:00-06:00"];
        assert!(Cli::try_parse_from(gone).is_err());
    }
}
