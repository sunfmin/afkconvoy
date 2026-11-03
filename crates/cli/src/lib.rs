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
pub mod agent_guards;
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
pub mod forget;
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
pub mod runner_socket;
pub mod scaffold;
pub mod serve;
pub mod session;
pub mod settle;
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
pub mod tools;
pub mod topic;
pub mod tracker;
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

/// The skill file `skill` prints, and the one `npx skills add` installs: a
/// user's own agent learns from it that afkconvoy is there.
const SKILL: &str = include_str!("../../../skills/afkconvoy/SKILL.md");

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
/// - `orca` on the PATH: every worker is started through it. It is running (`orca open`) and was given this checkout, once (`orca repo add --path "$PWD"`).
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
    ///
    /// ## When to run it
    ///
    /// Run `afkconvoy accounts` in a checkout to measure how long its issues took, or compare windows before and after a fleet change. Needs git and a signed-in `gh`; no running fleet or orca is needed.
    ///
    /// ## Who runs it
    ///
    /// The operator, or an agent reading completion times for them, in any checkout or worktree of the repo on the host whose tracker choice is to be read.
    ///
    /// ## What it reads and writes
    ///
    /// Reads this host's checkout list to find the tracker, then every open and closed issue's work account through `gh`, paging through issues and comments. Where the tracker is another repo, only this repo's repo label's issues belong. An issue carrying several repo labels belongs to none. With no tracker named on the host, the checkout's own repo is its tracker. It also reads this host's durable watch-round, asking-question and patrol-run usage records. It writes nothing and starts no worker.
    ///
    /// The window includes accounts and host usage records whose end is between now minus 7 days and now, inclusive. `--days <n>` changes that duration; `--all` includes every record ending by now. Issue closure and update dates do not select the window. Issues with no account are not samples. Watch rounds, asking questions and patrol runs are selected by their recorded end.
    ///
    /// Every readable work account carries one authoritative versioned JSON object in a collapsed details block with a fenced json block. Its human text is rendered from that object and is never parsed back. Old prose is converted by `rewrite-accounts` before this command can read it.
    ///
    /// ## What it answers
    ///
    /// One JSON object: `window` has `start` and `end` in epoch seconds (`start` null with `--all`); `count` counts readable accounts in it; `wall_seconds` has p50, p90, p95 and max of end minus earliest claim start. Percentiles use nearest rank: sort ascending and take rank ceil(p * count / 100), one based; an empty sample answers null for each measure.
    ///
    /// `by_harness` maps the first worker's harness to its `count` and `wall_seconds`, with the same percentiles; `unknown` holds issues with no worker. `multiple_workers` counts issues with more than one worker. `gate_seconds` has p50 and p95 of each issue's sum of timed gate rows, zero when no gate ran. Worker rows and gate rows overlap the wall clock: they are never added into it, and no remainder is computed.
    ///
    /// `tokens` sums known workers' raw counts as `totals` (input, cache_read, cache_write, output and reasoning); reasoning is part of output, not added to it. `unknown_workers` counts workers with no readable usage. `p50` and `p95` are nearest-rank issue totals over issues with at least one known worker; partial totals still count and unknown workers are reported. `by_harness` gives the same totals, percentiles and unknown count for each harness, grouping each issue's workers of that harness. `top` lists up to five issues by known total, ties by issue number, with number, url and total. No price is applied.
    ///
    /// `watch_rounds`, `asking_questions` and `patrol_runs` summarize host work with no issue work account. Each has `count`, known token `totals`, `unknown_rounds`, nearest-rank `p50` and `p95` per known record, `by_harness` with the same measures, and `top`: up to five known records by descending total, ties by id, each with `id`, `conditions` and `total`. A watch round's start is its delivery and end its report, with continuations included and standby excluded. Asking questions merge their delivery segments. Patrol runs have one record per harness. Missing or unreadable session usage is unknown, never zero. No price is applied to these either.
    ///
    /// Their NDJSON rows and durable records have `id`, `harness`, `model`, `start`, `end`, `seconds`, `tokens` and `conditions`. Times are epoch seconds; `seconds` is end minus start. `tokens` has the same model map and null-on-unknown meaning as a work account worker's. Watch conditions are the delivered condition ids; asking conditions identify the question's condition; patrol conditions are `patrol:<name>`.
    ///
    /// `slowest` lists up to five issues in descending wall clock (ties by issue number), each with `number`, `url` and `wall_seconds`. `unreadable` lists `number` and `reason` for accounts with unknown or invalid starts, ends, worker rows or gate seconds, duplicate accounts, or comments that could not be read. They are excluded from all measures. An unreadable end or failed comment read cannot be placed in a window and is reported in every window; an account with a readable end outside the window is excluded.
    ///
    /// A failed issue listing refuses the command with {"error": …} and a non-zero exit. Check `gh auth status`, correct the reported GitHub or checkout-list fault, then run it again.
    ///
    /// With `--rows`, stdout is NDJSON instead: one compact JSON object per account in the window, ordered by issue number, with the schema below plus `url`, the issue's URL. Host usage rows follow, ordered by id within each kind: `watch_round`, then `asking_question`, then `patrol_run`, with `kind` beside the usage fields above. An empty window prints no lines. Unknown measurements remain null in these rows even when they exclude the account from the summary. Accounts that cannot be decoded, including unknown schema versions, duplicate accounts and failed comment reads, are omitted and listed under `unreadable` in one JSON object on stderr (only when nonempty). Read stderr as well as stdout. This mode still exits 0 for unreadable accounts and writes nothing.
    ///
    /// ## Work account schema
    ///
    /// `schema_version` is {const:ACCOUNT_SCHEMA_VERSION}. A field's meaning stays fixed within a version; a breaking change raises the version. Unknown values are JSON null, never a substitute zero or an invented default. Times (`start`, `end`, `at`) are epoch seconds. Duration and size field names carry their units.
    ///
    /// - `issue`: the tracker issue number. `repo`: the code repo, owner/name. `tracker`: the tracker, owner/name. `afkconvoy_version`: the version of the binary that wrote the account.
    /// - `host`: {name, cpu, logical_cpus, memory_bytes}, as reported by the writing host; cpu is its chip name, else its machine model identifier.
    /// - `start`: earliest recorded claim since, retained across later ends. `end`: the end being accounted for. `wall_seconds`: end minus start, null if unknown or reversed.
    /// - `outcome`: landed, closed (nothing more to do), escalated, ended (the workflow ended), or human-closed (closed by a human while claimed). An unknown outcome is null.
    /// - `workers`: a list of {harness, model, role, stage, claim, start, end, seconds, verdict, tokens}. `tokens` is null when usage is unknown; otherwise it maps model names to {input, cache_read, cache_write, output, reasoning}, raw counts with reasoning null when not stated. An empty model name means unknown. Cached input is excluded from input, and reasoning is included in output. A model not explicitly named is null. Role and stage are null where there is none or neither was recorded. Claim is the claim's identity. A bound worker ends at its stage record or failure, else the account end; an unbound worker ends at the account end. Seconds are end minus start. Verdict is its stage record's verdict, null without one. Landing workers are not rows; continuations on the same harness are not new rows. A worker on a harness that keeps a goal — Codex — also has `goal`, absent for any other: {set, objective, goal_turns, goal_turns_after_verdict, status, tokens_used, token_budget, session}, read from its session record and its home's goal store while its worktree holds them. `set` says whether its session was set a goal, and `objective` is what `/goal` was typed with: the brief pointer. `goal_turns` counts the turns with `turn_trigger: goal`, its setting's first turn among them; `goal_turns_after_verdict` those begun after its verdict stood — its stage record, else its last verdict comment — null without one. `status` is the store's final `thread_goals.status`, or `cleared` where the goal was set and its row is gone, as `/goal clear` leaves it. `tokens_used` and `token_budget` are the store's, uncached input and output; a cleared goal's are null. `session` is the session record's path on the writing host.
    /// - `gates`: a list of {tree, command, lane, `result`, at, seconds, host}. Tree is the tested git tree, command is the exact command, lane is fast, slow or whole when known, `result` is green or red, at is the gate record's epoch timestamp, seconds times the command after its queue turn, and host is the host that ran it. Prior red runs survive later edits after their remote refs disappear. For converted records, a lane stays null unless its original marker names one.
    ///
    /// A converted account retains its original evidence in `legacy_evidence`. Worker durations, models and host hardware stated by the old rows are recovered; facts not stated remain null. The overlapping worker and gate durations do not partition wall_seconds.
    ///
    /// ## What trips you up
    ///
    /// The sample is work accounts, including an account on an issue still open after escalation. It does not measure every closed issue. Unknown gate seconds make the account unreadable; no gate row means zero. Read `unreadable` with the count before comparing two windows.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Run once per tracker after upgrading all fleet instances. It needs git and a signed-in gh. Run with no flags first to inspect every old and proposed body and its check.
    ///
    /// ## Who runs it
    ///
    /// The operator or an agent working on the operator's behalf, in a checkout of a repo using that tracker.
    ///
    /// ## What it reads and writes
    ///
    /// Reads all issues and comments in the checkout’s tracker. With no flags it writes nothing. With `--backup <directory>` it saves each original in that directory before editing the comment, and skips records it cannot validate. The directory must be kept for recovery. Repeating it skips JSON accounts.
    ///
    /// ## What it answers
    ///
    /// One JSON object with `dry_run`, `backup`, counts `rewrite` and `left`, and `changes` containing issue, comment, old_body, new_body and check. `before` and `after` show this checkout’s all-time account summaries as currently read and as projected after conversion. It refuses a changed comment or a conflicting backup; run it again after examining the error.
    ///
    /// ## What trips you up
    ///
    /// Store the backups in a durable private directory. Accounts with unrecognized facts are left as they are and remain unreadable to `accounts` until repaired.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    RewriteAccounts {
        /// Save original bodies in this directory and edit validated comments
        #[arg(long)]
        backup: Option<std::path::PathBuf>,
        #[command(flatten)]
        at: At,
    },
    /// Write this host's host kit. `kit --cast` is refused: automatic casting is gone
    ///
    /// ## When to run it
    ///
    /// When a harness is installed or removed, a login changes, or its models are to be listed again. Research comes from the watch, through `afkconvoy kit --research <file>`. `run` writes the host kit itself on a host that has none, and a tick writes it again once a day (below). It needs no repo and changes no remote.
    ///
    /// ## Who runs it
    ///
    /// The operator, or a machine in their place, on the host that starts the workers: it asks nothing, at a terminal or not, and does the same whoever runs it.
    ///
    /// ## What it reads and writes
    ///
    /// The host kit is $XDG_CONFIG_HOME/afkconvoy/kit.json, or $HOME/.config/afkconvoy/kit.json when XDG_CONFIG_HOME is unset. It is kept on this host and never pushed. It is the only source of worker launch commands. It holds the harnesses this host can start, in order of preference, each with its worker launch command and listed models. Former `stock_consult` kit choices are accepted on read and dropped on write. It is written automatically, with no confirmation:
    ///
    /// 1. Harnesses. Each known harness whose program `type` finds in the login shell a worker gets — `$SHELL -lic`, or `/bin/zsh` where `SHELL` is unset, with its rc files, PATH and logins, started from the environment a fresh login brings in and not from this process's — on the command the host kit holds for it, or else its stock worker launch command. No terminal is opened.
    /// 2. Models. Each harness found is asked for the models its own login runs, in that same login shell, and no terminal is opened. Claude Code is asked through its command with an `initialize` request and no prompt; Codex through its command with `debug models`, its server's list; pi through its command with `--list-models`; Kimi Code through its command with `provider list --json`, under the fleet's isolated home; Grok through its command with `models`, under the fleet's isolated home and with no prompt; DSH through `dsh headless`, whose listing plugin asks `https://api.deepseek.com/models` with DSH's account token from `$DSH_HOME/.credentials.yaml` (default `~/.dsh`), or its `DEEPSEEK_API_KEY` when no account token is held. The server's ids and display names are used, never a bundled catalog. This is the sole credential the fleet reads, through DSH's credential service, only for listing: never printed or stored, and sent only to DeepSeek. Not signed in or a server refusal leaves DSH out with the reason.
    /// 3. Default first. No model is sent a prompt. A harness found and listed is held at once, each model it newly lists `research pending`, which no worker is started on. A harness with no placed model starts its workers on its own login's default model, with no model flag. Codex automatic workers instead read the user's current model preference from `~/.codex/config.toml`, unless an issue has a model label; with no preference they use Codex's native default. `plan` and `doctor` report the kit's `default_model` and `research_pending` independently of that preference.
    /// 4. Names. A model's id is exactly the id its harness lists, and a worker is started on it. Its `names` are the names its listing shows it by (Claude Code's `Opus 5.5`) and the aliases research ties to it with a dated source.
    /// 5. Order. Established harness and model order is retained, including switches made while listing or research run. New harnesses follow in this fixed order: claude-code, pi, codex, dsh, kimi, grok. New models follow established models; research may rank discoveries without displacing established priority. Harness order is this list. A worker's model is its issue's one `model:` label of the selected harness, where that label is followed, otherwise that harness's user default. A harness with no placed model starts with no model flag.
    /// 6. Research. The watch researches listed identities using its web tools, once across harnesses, and writes a JSON file through `afkconvoy kit --research <file>`. The kit itself calls no model. Unplaced models are a doctor finding: a watch started for other work takes them too; research alone starts one at most once a day. A harness with no placed model stays on its user default. Sourced research is kept as good_at, names, sources ({url, date}) and researched_at (epoch seconds), and is never refreshed on a schedule. `afkconvoy draft` copies good_at into a roster.
    ///
    /// `afkconvoy kit --remove <harness>` takes the harness, its command and its models out and records `removed: [<harness>]` in the host kit. No worker, patrol, landing turn or watch later starts on it, including as a fallback. Relisting, the daily refresh and worker-launch discovery leave it out even while its program remains installed. Only `afkconvoy switch --harness <harness>` clears the removal and puts it back first. Removing a harness with live fleet workers is refused: the error names their issues or worktrees and gives the `afkconvoy switch --all --harness <harness>` line that moves them first. Removal ends no worker.
    ///
    /// A model named by a roster entry or a `model:` label matches a placed model the host kit holds for its harness when it is its id or one of its names, whatever the case; with the provider prefix left off where that leaves one match (`gpt-6-sol` for pi's `openai-codex/gpt-6-sol`). A name with two matches matches neither, and says which two; one with none says the nearest names.
    ///
    /// `afkconvoy kit --cast` is refused: automatic casting is gone. An old host-kit `cast` key is ignored, changes no start, and is dropped the next time the kit is written. It does not fail `kit` or `check`. A worker's model is its one `model:` label of the selected harness, or that harness's user default (`afkconvoy cast --help`).
    ///
    /// Without --cast, --research or --remove it says on stderr where the file is, what it holds — each harness's placed models, or its default model, and those `research pending`, `not researched` or settled — and what was left out.
    ///
    /// ## What it answers
    ///
    /// With --cast: an error, automatic casting is gone. With --research: {path, written, placed, settled, kit}, the host kit file, true, each model placed or settled as {harness, model} in its respective array, and the resulting kit. With --remove: {path, written, removed, kit, left_out}, with removed the harness key and the kit without it. Otherwise {path, written, kit, left_out}: the file; true; the kit written; and each harness or model not kept, as {harness, model, why} — model absent for a whole harness — why `not installed`, `listing failed`, `no longer listed`, or `removed by the operator`. `plan` and `doctor` show operator removals in `host_kit.left_out` with that same reason. Run twice with nothing changed, it writes the same kit. The written kit has no `cast` key. Errors: the login shell did not answer within 20 seconds; a kit that cannot be read or written; or a removal refused because live workers are still on the harness.
    ///
    /// A harness with `models` [] or no placed model starts on its default. Automatic command discovery can append a harness with no models unless the operator removed it; one written by hand or by an older release is also read as it is: a model named in it with no research is started on until research answers for it, when the watch supplies its research. Neither the kit nor `left_out` carries probe evidence any more: an older kit's `probed` fields are read and not written again.
    ///
    /// ## The research file
    ///
    /// {const:KIT_RESEARCH_STATES}
    ///
    /// `afkconvoy kit --research research.json` reads the existing host kit, checks the whole file, and atomically writes the placements and settlements. It needs neither a checkout nor orca. Read `afkconvoy doctor --all` for current ids and names. To place a model, use:
    ///
    /// ```json
    /// {"models":[{"harness":"claude-code","model":"listed-id","good_at":"wide refactors; slower on small fixes","names":["sourced alias"],"sources":[{"url":"https://vendor.example/model-card","date":"2026-10-26"}]}]}
    /// ```
    ///
    /// To settle an unsuitable model, replace good_at and names with a nonempty settled reason:
    ///
    /// ```json
    /// {"models":[{"harness":"codex","model":"codex-auto-review","settled":"approval reviewer, unsuited to workers","sources":[{"url":"https://vendor.example/reviewer","date":"2026-10-26"}]}]}
    /// ```
    ///
    /// Each entry names an exact listed id of a held harness. Name each identity once across harnesses: the same id without its provider prefix shares research, including settlements. Each entry supplies either nonempty good_at describing coding strengths and weaknesses, with optional nonempty sourced names, or a settled reason with no names. Every entry needs at least one HTTP or HTTPS source URL and its publication or update date as a calendar date YYYY-MM-DD. Use primary and high-trust sources, with live web search. The models array ranks newly placed models, after established model priority; harness priority, commands and existing dated research stay as held. Models omitted stay as held for a later watch.
    ///
    /// Refusals name the file and fault and write nothing: malformed JSON, unknown fields, unknown harnesses or ids, repeated identities, conflicting outcomes, empty good_at, settled reason or names, missing or malformed dated sources, or a file answering no model awaiting research. Correct the file and run the command again. The answer lists every placement and settlement, including identities shared by another harness. Research does not change when the models were last listed (`refreshed`).
    ///
    /// ## What trips you up
    ///
    /// A harness or model left out is gone from the file: commands and research are kept only for what is kept. At a worker launch, a known harness with no command is discovered in the worker's login shell and its checked stock command is merged into the kit, without model probes or research, unless the operator removed it. An explicit command always wins. `--worker-command` is gone: a harness's command is the host kit's. A command is an opaque shell string; an unbound repo starts it under nice with its worktree's TMPDIR, adding its harness's model flag only where a model label is followed, and its harness's worktree-trust setup. With no followed label the worker starts with no model flag, on that harness's user default. Codex also receives a session-private user directory and the effort and explicit service tier read from the host user's own Codex config at each launch; no saved worker preference selects them. A bound repo adds its role flags ([`afkconvoy help worker-launch`](help:worker-launch)).
    ///
    /// Writing the kit sends no prompt to any listed model. A listed model that fails when its worker starts is found by worker health, classed `model` at model scope and left out; its hourly recovery probe brings it back. A harness refusing every prompt is paused and has its own hourly recovery probe. Those health probes are the only model probes; kit writes, switches and refreshes make none. A recovery probe asks that same login shell and opens no terminal. A pause is running state and recovers; an operator removal is a choice and no probe undoes it.
    ///
    /// ## It stays current
    ///
    /// A tick — of `run`, or `afkconvoy tick` — writes the host kit again once its file was last written a day ago or more, before anything else: a newly listed model waits for the watch, and a model no longer listed is dropped. A harness that cannot be found or listed is kept, and a pause leaves it out, as above. An operator removal survives this refresh and every other relisting. A harness merely left out by a hand edit is read as absent, but only `kit --remove` records the removal that survives a refresh; the next relisting may add a hand-omitted installed harness back. The file's modification time is when it was last written — by `kit`, by `run` on a host without one, or by a tick — and `plan` and `doctor` answer it as `host_kit.refreshed`. Every fleet of the host shares the file and the lock beside it (kit.refresh): one of them writes it again, once a day, and the others read what it wrote. A refresh that changed the host kit is a line of the run log, `refresh_kit (fleet): the host kit was written again: added claude-code sonnet; dropped claude-code opus`; one that failed is one too, and is tried again an hour later. The pause is the host's running state, in its cache: this file is not changed. `plan`, `doctor` and the panel say which is paused, since when and why.
    ///
    /// ## The agent setup
    ///
    /// The operator's global instructions, skills and guards, declared once in the operator's own files and named by three host settings, `run --agent-instructions <FILE>`, `run --agent-skills <FILE>` and `run --agent-guards <FILE>` (`afkconvoy run --help`). The skill list holds one line per source, `<source> <skill>...`, or `<source> *` for every skill the source ships; blank and `#` lines say nothing. Writing the host kit gives every harness it holds the setup: the instructions file is linked at the harness's global instructions location (Claude Code `~/.claude/CLAUDE.md`, Codex `~/.codex/AGENTS.md`, pi `~/.pi/agent/AGENTS.md`, Kimi Code `~/.kimi-code/AGENTS.md`, Grok `~/.grok/rules/AGENTS.md`, DSH `~/.dsh/AGENTS.md`), with what stood there and was no link to it kept beside it as `<name>.before-agent-setup`; the list is installed through `npx skills add <source> -g -y -a <agent>...` for each harness's `npx skills` agent (DSH has none, and reads the shared copies), and a skill installed for one of those agents that the list does not name is removed. The copies, in the shared `~/.agents/skills`, are read-only between syncs: no harness and no shell command edits one in place; a sync opens them for its own writes and closes them again. The guard list holds one line per guard, `shell <command>` or `edits <command>`; blank and `#` lines say nothing. Each guard is wired before the tool calls it applies to into the hook settings of every held harness that has some: Claude Code `~/.claude/settings.json` (matchers `Bash`, and `Edit|Write|MultiEdit|NotebookEdit`), Codex `~/.codex/hooks.json` (`Bash`, and `apply_patch`); pi, Kimi Code, Grok and DSH are given none. Wiring merges and owns nothing else: an entry of the kit's is one whose command opens with `: afkconvoy-agent-guard;`; those are replaced by the declared ones, after every other entry at `PreToolUse`, and every other key and entry is written back as it was; a file already as declared is not written. Codex asks its user to review a new hook once (`/hooks`). A Codex session outside a role, started with a user directory the host keeps rather than `~/.codex`, is given the guards in that directory's `hooks.json`, which holds them alone and nothing else of `~/.codex/hooks.json`; it starts with `--dangerously-bypass-hook-trust`, so nothing is asked. A worker in a role is given none of them. A guard reads its harness's call as JSON on standard input: a shell call is `tool_name` `Bash` with the line in `tool_input.command` on both harnesses, with `cwd`; an edit is Claude Code's `Edit`, `Write`, `MultiEdit` or `NotebookEdit` with `tool_input.file_path` (or `notebook_path`), and Codex's `apply_patch` with the patch in `tool_input.command`, whose `*** Add File: `, `*** Update File: ` and `*** Delete File: ` lines name the files; `AFKCONVOY_HARNESS` names the harness. It refuses by exiting 2 with why on standard error, or by printing `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"<why>"}}`: both harnesses take either. A tick gives the setup to a harness held since it was last given — found by a refresh, a switch or a launch — with no step by the operator; one whose giving failed is tried again an hour later. A `run` that changes either setting gives it to every held harness again. What was given is recorded beside the host kit, in `agent-setup.json`. `doctor` names each held harness that differs from the declaration (`differs from the agent setup`), with `afkconvoy kit` as its fix. With none of the settings said, nothing is given and nothing is reported.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Do not. `afkconvoy cast` fails and says automatic casting is gone. So does `afkconvoy kit --cast`. Nothing is chosen, no prompt is sent, and no label is added or removed.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout. A tick never runs it, and no issue waits.
    ///
    /// ## What it reads and writes
    ///
    /// Nothing. It fails before it reads an issue or writes a label, a comment, or the host kit.
    ///
    /// ## What a worker runs
    ///
    /// The host kit's order and the role's range choose the harness. `switch --harness` reorders the kit. A `harness:` label never selects a harness. `switch --model` moves a model first inside its harness and does not choose the model a worker starts on.
    ///
    /// A worker's model is the issue's one `model:` label when that label names exactly one model of the harness already selected, matched by its id or any name, whatever the case, or with the provider prefix left off where that leaves one match. The worker starts on that model's id. The label is left as written.
    ///
    /// Otherwise the worker runs that harness's user default, and the label stays: no `model:` label, a name that matches nothing the harness holds, a name that matches two, two `model:` labels, or a name that matches a model the role's range does not hold. A label that is not followed is said once on the issue, with the nearest names the harness holds, or the two ids. The worker starts on the user default in that same dispatch. Nothing is taken off the issue.
    ///
    /// Claude Code, pi, Kimi and Grok start with no model flag. Kimi then uses `default_model` from the user's Kimi config. DSH starts with no model in the worker environment, so the profile's model runs. Codex reads `model` from the user's Codex config when that value is a nonempty string the host kit lists for Codex and passes it; with no such key, Codex starts with no model flag. A Codex `model` that is not a nonempty string, or that the kit does not list, refuses the start. A Codex model named that way and outside the role's range refuses too. An unnamed default cannot be checked against a range, and the worker starts.
    ///
    /// A watch, a cut and an inspection have no model label and use the user default. A landing worker follows its issue's label by the same rule. A range that holds one model does not name it.
    ///
    /// ## What it answers
    ///
    /// An error: automatic casting is gone. `plan` and `doctor` say, under `casting`, whether a model label was followed (`model_label`) or the user default was used (`user_default`). `needed` is false. `skipped` is empty. They report no caster and no host cast flag. A launch record that already says `casting` is still read as the issue's model label.
    ///
    /// ## What trips you up
    ///
    /// An old host-kit `cast` key and a roster `cast` key do not change a start and do not fail `check` or `kit`. A label that is not followed is said once and left in place. Switch alone leaves healthy workers on their current model; --all explicitly continues them.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Cast {
        #[command(flatten)]
        at: At,
    },
    /// Print the repo's config as JSON, defaults filled
    ///
    /// ## When to run it
    ///
    /// First, before `plan` and `run`: it shows what the fleet goes by in this repo, or why the config cannot be read. Run it again after editing the config.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It reads one file — {const:CONFIG_FILE} of the checkout it is about, or `--file` — and of that file the first ```yaml block and any ```yaml gate-budget blocks. It writes nothing, and reads neither GitHub nor the remote.
    ///
    /// ## The config file
    ///
    /// A markdown file. The keys of its ```yaml block are the keys of the answer below, named and written alike: a file holding every key, each at its default, is
    ///
    /// ~~~markdown
    /// # afkconvoy config
    ///
    /// ```yaml
    /// base_branch: "main"                # required: it has no default
    /// gate_command: ./gate.sh            # required: it has no default
    /// gate_seconds: {const:GATE_SECONDS}
    /// ready_label: ready-for-agent
    /// epic_labels: [epic, prd]
    /// escalate_label: ready-for-human
    /// lock_label: fleet-lock
    /// team: acme/team                    # naming a team binds the repo
    /// patrols: []
    /// intake_limit: {const:INTAKE_LIMIT}
    /// ```
    /// ~~~
    ///
    /// The optional `gate_budget_seconds` is omitted from this example: it has no default of its own. A key left out is at the value shown, so the shortest config is the first two lines, and a bound repo's those and `team`. The last two are read only beside `team`: an unbound repo leaves all three out, and may say one key more, never beside `team`:
    ///
    /// ```
    /// escalate_to: human                 # legacy consult is accepted; both escalate for the watch
    /// ```
    ///
    /// An optional `gate_budget_seconds` declares a fixed warning budget: a positive whole number of seconds below `gate_seconds`. It never ends a run. Put it in the first yaml block, or in a separate block opened by ```yaml gate-budget; that block accepts only `gate_budget_seconds`. Declare it once: duplicate keys fail, including across blocks. Older versions skip the separate block and keep their termination limit, so they can land and cut the version that first supports the budget. They keep warning at {const:SLOW_GATE_PERCENT}% of that limit until upgraded. The budget is fixed in the repo, with its machine class, baseline version, cache conditions and rationale; it never follows recent runs upward. Queue waiting is outside both durations.
    ///
    /// A repo may split its gate in two, in a block of its own opened by the line ```yaml gates: the fast gate, what a worker passes before it opens or updates its PR, and the slow gate, everything else the gate checks. Each says its `command`, its limit `seconds` and its warning budget `budget_seconds`:
    ///
    /// ~~~text
    /// ```yaml gates
    /// fast:
    ///   command: afkconvoy exclusive -- ./gate.sh fast
    ///   seconds: 600
    ///   budget_seconds: 300
    /// slow:
    ///   command: afkconvoy exclusive -- ./gate.sh slow
    ///   seconds: 3000
    ///   budget_seconds: 300
    /// ```
    /// ~~~
    ///
    /// The two are the gate split, not a gate more: together they check what `gate_command` checks: a landing turn runs the fast gate per candidate and the slow gate once per round (`afkconvoy land --help`), and a base tip with no record is gated with `gate_command` whole. So each budget is below its limit, their limits add up to no more than `gate_seconds`, and their budgets to no more than `gate_budget_seconds` where it is declared. `afkconvoy gate --fast` and `--slow` run each alone (`afkconvoy gate --help`). Older versions skip the block.
    ///
    /// What is outside these blocks is yours to explain it in — but for a second block opened by the line ```yaml cut, which names the repo's cut steps and turns cutting on (`afkconvoy cut --help`). It is read beside this one; older versions read only this one, and refuse nothing for it.
    ///
    /// No command writes this file: write it, commit it, and run `afkconvoy config`.
    ///
    /// ## What it answers
    ///
    /// {"base_branch", "gate_command", "gate_seconds", "ready_label", "epic_labels", "escalate_label", "lock_label"} — every key, the defaults filled in. `gate_seconds` is the gate limit: how many seconds a run of `gate_command` is given, by `afkconvoy gate` and by a landing turn alike, before it is ended and is red (default {const:GATE_SECONDS}). It is written nowhere else: neither command has a flag for it. A config that leaves it out is a finding of `afkconvoy doctor`. `lock_label` is the label of a fleet lock (default fleet-lock): while an open issue carries it, the fleet works lock issues only (`afkconvoy lock --help`). How many claims a fleet instance holds at work at once is no key: it is its checkout's, said on the host that works it (`afkconvoy run --concurrency`), and {const:CONCURRENCY} until that host says otherwise. A config that still names `concurrency` is read all the same, but for that key, which changes nothing: `afkconvoy doctor` reports it as `info`, since nothing is held up, with the file and key to delete. Where the checkout says none and the config names a different number than the one in force, its fix first gives the `afkconvoy run` that keeps that number on the host. `afkconvoy plan` says the concurrency in force, and who says it.
    ///
    /// A repo may declare host requirements in a separate block, which older releases read past:
    ///
    /// ```yaml gate-needs
    /// checks:
    ///   - name: compiler
    ///     command: test "$(compiler --version)" = "1.0"
    ///     seconds: 10
    /// ```
    /// A separate `yaml cut-needs` block uses the same `checks` form for cutting. Before claiming a due cut, the host runs these checks; a failure leaves the cut for another host and is named in the tick, plan and doctor. Output follows the same privacy rules.
    ///
    /// Each name is unique and nonempty, each command nonempty, and seconds positive. Each runs with `sh -e` in the checkout. The startup probe refuses a failed or timed-out check; a passing check stands for an hour. A check's stdout is discarded. On failure its last 20 stderr lines are stored in a user-only file in the host cache, overwritten by its next run; the refusal names the file, never its contents.
    ///
    /// A check may declare how to install what it checks, with `install` and `install_seconds` together (both or neither):
    ///
    /// ```yaml gate-needs
    /// checks:
    ///   - name: flux-checker
    ///     command: ./verify.sh pins
    ///     seconds: 10
    ///     install: ./verify.sh install
    ///     install_seconds: 5400
    /// ```
    /// Where such a check fails, the host runs its install with `sh -e` in the checkout, in the host's exclusive queue (`afkconvoy exclusive --help`) as a heavy command, within `install_seconds` — past it, it is ended and named as timed out — then the check again. A pass admits; a fail refuses naming the check, the install's status and the user-only host-cache file of its last 20 stderr lines; its stdout is discarded, as a check's is. A failed install is not run again for an hour unless the check's declaration changes, which is tried at once. A running fleet instance's hourly recheck installs the same way before it ends broken, and a due cut installs before it is claimed. The startup probe warns when an install ran and passed; `plan` answers each install noted on the host as `need_installs` [{kind, check, at, seconds, status, passed, stderr}]; and `doctor` says each as `was installed on this host` or `install failed on this host`, never its output.
    ///
    /// The contract of an install: it writes only under the user's home, never to system directories and never with sudo, and installs the pinned versions the repo already names, reading them from where the repo keeps them, not from a copy. The owner gives standing consent for exactly this — what a repo declares as a gate or cut need, at the version it pins, installed under the user's home (sunfmin/control#499); anything else still asks first.
    ///
    /// Where a budget is declared, the answer also has `gate_budget_seconds`. The last {const:SLOW_GATE_LANDINGS} recorded landings on one host must each exceed it to warn through the existing slow-gate issue and doctor finding. Without it the warning threshold stays {const:SLOW_GATE_PERCENT}% of `gate_seconds`. A warning neither ends nor fails a gate.
    ///
    /// A repo whose config names a team is a bound repo, and answers three keys more: {"team", "patrols", "intake_limit"} — the team's directory as owner/repo or owner/repo/path (a whole repo, or a subdirectory of one), which of its patrols run here, by name (default none; `afkconvoy check` holds them against the team's, and `afkconvoy tick --help` says when one runs), how many issues taken in by workers may be open (default {const:INTAKE_LIMIT}). A config names no harness: what each worker runs is its team's `roster`'s to say (`afkconvoy help roster`), within what the host kit holds (`afkconvoy kit --help`). A repo that names no team is unbound and answers the seven above and one legacy key: {"escalate_to"}, always `human`. Existing `consult` values still load, but every hand-off escalates for the host's watch to answer. Only a consult explicitly named by a bound repo's stage is asked before escalation (`afkconvoy help humans`).
    ///
    /// Where the file names cut steps, the answer has one key more: {"cut": {"set", "publish", "initial"?}} — each as the ```yaml cut block says it. Optional `initial: X.Y.Z` declares the first version to cut when the remote has no version tag; after the first tag it is ignored. Where it splits its gate, one more: {"gates": {"fast", "slow"}}, each {"command", "seconds", "budget_seconds"}.
    ///
    /// Errors: the file cannot be read; it has no ```yaml block; a ```yaml cut block has a key but `set`, `publish` and `initial`, lacks a required step, a step names nothing to run, or `initial` is not X.Y.Z; `base_branch` or `gate_command` is missing; a key is unknown or of the wrong type; `gate_seconds` is not a whole number from 1; `gate_budget_seconds` is not a positive whole number below `gate_seconds`, is declared twice, or its separate block has another key; the ```yaml gates block lacks `fast` or `slow` or one of their keys, has another key, a gate names nothing to run, has no seconds or a budget not below them, or the two gates' limits or budgets add up past the gate's; `team` is neither owner/repo nor owner/repo/path; a `harness`, which a config names no more — the error says the line to delete; `patrols` or `intake_limit` is set where no `team` is; `escalate_to` is neither consult nor human, or is set beside `team` — the error names the key. Without `--file`, outside a git checkout: "is not in a git checkout", or git's own words for a checkout it refuses.
    ///
    /// ## What trips you up
    ///
    /// - It reads the config only. Whether the team it names loads is `afkconvoy check`.
    /// - It reads the file as it is in this checkout, committed or not. A worker reads the one in its own worktree, cut from its target branch. The destination remains pinned even if that file names a different configured base.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Config {
        #[arg(long, help = format!("The config file [default: {} of this checkout]", config::FILE))]
        file: Option<PathBuf>,
        #[command(flatten)]
        at: At,
    },
    /// Read a team and its workflows: what was read, or every fault
    ///
    /// ## When to run it
    ///
    /// After every edit to a team's files, a role's skills or a repo's `team` key, and before a fleet is started on a bound repo: it says whether the team loads, and if not, every fault at once. Edit, run it, and repeat until it exits 0.
    ///
    /// ## Who runs it
    ///
    /// The operator, or whoever designs the team: in a checkout of a bound repo, or anywhere with `--team` or `--dir`.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing but a copy the fleet has not got yet (below). It reads a team's files, and of each role's directory which skills it holds and what each one's `SKILL.md` opens with, which harnesses it holds limits for, and whether a file of it would have a harness load other rules.
    ///
    /// What those files are, and every key of each: `afkconvoy help team`, `afkconvoy help workflows` and `afkconvoy help patrols`.
    ///
    /// Without a flag it reads the config of this checkout for `team`, and that directory from GitHub at the head of its repo's default branch — and holds the config's `patrols` against the patrols that team has.
    ///
    /// `--team owner/repo` or `--team owner/repo/path` names the team instead. `--dir <path>` reads a team's files from a directory as they are, committed or not. Every role is always read from GitHub, at the head of its repo's default branch. A directory says no repo, so a role it names as `./path` is a fault — unless `--team` beside `--dir` says which team on GitHub the files are an edit of: the role is then read from that repo.
    ///
    /// A repo on GitHub — the team's, and each role's — is asked for the commit at its head, and read from the fleet's copy of that commit: the one a worker's directory is taken out of, kept read-only under the cache directory (copies/<owner>/<repo>/<commit>): `afkconvoy help cache` says where that directory is on this host, and what moves it.
    ///
    /// Only a commit the fleet has no copy of yet is cloned, so a role pushed since the last run is read at its new head, and one that has not moved costs one question. A repo is asked and cloned once however many roles it holds, and a role in the team's own repo is read at the team's commit. The repos are read at once, not one after another.
    ///
    /// It reads this host's host kit (`afkconvoy kit --help`) for each role's range, and asks no harness anything: which models a harness knows is the host kit's to hold.
    ///
    /// ## What it answers
    ///
    /// {"team", "path", "commit", "files", "escalate_label", "rules", "host_user", "dsh_profile", "split", "roles", "workflows", "patrols"} — what it read.
    ///
    /// `team` is the team's repo and `path` its directory there, "" for a whole repo — with `--dir`, the directory read and null. `commit` is the team's commit read, null with `--dir`; `files` the definition files read, each from the team's directory. `rules` is whether the team has rules of its own: a `CLAUDE.md` in its directory, which every one of its workers is given.
    ///
    /// `host_user` is whether its workers are given the host user's own skills and rules beside their role's: true unless the team file says `host_user: false`, and a value that is not true or false is a fault (`afkconvoy help roles`). It is each role's default.
    ///
    /// `split` is whether stage briefs offer splitting: true unless the team file says `split: false`. Anything but true or false is a fault (`afkconvoy help team`). `dsh_profile` is `fleet` (default) or `host-user`: which profile DSH workers load, independently of host_user. A role may say its own choice; check answers its effective dsh_profile and refuses every other value.
    ///
    /// Each of `roles` is {"name", "repo", "path", "commit", "host_user", "dsh_profile", "skills", "above", "limits", "roster", "range", "left_out", "unmatched", "nothing_safe"}: `path` is the role's directory in `repo`, "" for a whole repo; `commit` the one the role's repo was read at; `host_user` whether its workers are given the host user's skills and rules — its own say, `{ dir, host_user, dsh_profile }`, or the team's; a key of that map but those three, or a `host_user` that is not true or false, is a fault; `above` each `CLAUDE.md` and `.claude/` a directory above the role holds in its repo, as a path from the repo's root — but for the team's own `CLAUDE.md`, where the team's directory is above the role: those are the team rules, which a worker is given; `limits` the harnesses the role holds limits for — a hook or a denied permission in its `.claude/settings.json` for claude-code, an entry of its `.pi/extensions/` for pi, a hook in its `.codex/hooks.json` for codex, a `.dsh/cordis.patch.yml` overlay for dsh, a hook or denied permission in its `.kimi-code/limits.toml` for kimi, a hook or denied permission in its `.grok/config.toml` for grok; `roster` its entry in the team's roster, {"run": [{"harness", "model", "good_at"}]} with null for what it leaves out, and null for a role it names none for; `range` its range on this host, [{"harness", "models"}] in the order a worker takes them — the host kit whole for a role with no entry, or whose entry matches nothing in the host kit, less each harness the role holds no limits for while it holds limits for another, and null where this host has no host kit (`afkconvoy help roster`); `left_out` each harness its entry and the host kit allow it and its range leaves out, [{"harness", "why"}] — why `no limits for pi` — and null where this host has no host kit; `unmatched` what its roster entry allows and the host kit holds, where the entry matches nothing in the host kit — the role then ranges over the host kit whole — and null otherwise; `nothing_safe` why no worker of it can be started on this host, where it holds limits for no harness the host kit holds — its stages wait — and null otherwise. Each of `workflows` is {"name", "flow_label", "max_rework", "max_consecutive_rework", "stages", "land"}, the defaults filled in; each stage is {"name", "by", "do", "writes", "escalate", "pass", "reject"}. `unmatched_models` lists each roster model matching nothing held, as {role, harness, model, holds, said}, with the nearest names or the two matches; a tick opens one issue for each in the team’s repo.
    ///
    /// Each of `patrols` is {"name", "by", "do", "every", "max_open", "max_runs_a_day", "max_landed_a_day", "runs_here"}: `every` in seconds, `max_runs_a_day` null where the file says none. `runs_here` is whether the config of this checkout names the patrol — false with `--team` and `--dir`, which read no config.
    ///
    /// A team with a fault: {"error", "faults"} and exit 1. `faults` holds every fault of the team, not the first: each {"file", "key", "problem", "see"}. `key` is a path from the top of the file's yaml, counting from 0 — `stages[1].on.reject` — and empty when the fault is the file itself. `see` is the command that prints the help topic describing that file, to run as it is: `afkconvoy help workflows` for a workflow's, `afkconvoy help patrols` for a patrol's, and `afkconvoy help team` for the team file — a role is named there, and that topic says what its directory holds — and for the config's keys that bind the repo. A fault in a file no topic describes has no `see`.
    ///
    /// A fault is one of: an unknown key, or one written twice; a wrong type; a duplicate stage name; a stage named `land`, `done` or `human`; a `pass` or `reject` that names no stage; a `reject` to its own or a later stage; a stage no path from the first reaches; a stage whose passes never come to `land` or `done`; a `pass: land` with no `land:`, or a `land:` no pass leads to; a role the team file does not name; a role named as none of owner/repo, owner/repo/path and ./path; a role's path with a `..`, or an absolute one; a role whose repo cannot be read, or whose path is no directory at its head; a role's directory that is the team's or contains it, or is inside the team's `workflows/` or `patrols/`; a role's directory that contains another role's or is inside one — two names for one directory are none; a symlink in a role in a subdirectory that leaves the role's directory; a file in a role's directory that a harness would load in place of `CLAUDE.md`, its rules under every harness — pi `AGENTS.override.md`, `AGENTS.md` or `AGENTS.MD`, Codex `AGENTS.override.md` or `AGENTS.md` — the fault naming the file and each harness that would load it, whatever harnesses the role may run; a role's limits that cannot be read; a skill its role's directory does not hold; a missing `by`, `do` or `on`; a `harness` or a `model` in a stage or a patrol, which name none any more — the fault says the line to delete and the roster entry to write in its place; in the `roster`, an entry for a name that is no role of the team, a `run` that is missing or empty, a `harness` that names no harness, or an allowance written twice; a flow label two workflows take, or one that is the team's escalate label, the repo's lock label or an epic label — under `label`, or the file where it is the `flow:<name>` it names none in place of; a patrol file whose name is not a patrol's; an `every` that is no duration; a missing `every` or `max_open`; and — in the config, {const:CONFIG_FILE}, under `patrols` — a name the team has no patrol for.
    ///
    /// A team that loads is then held to its harnesses, and these are faults too, each with its file and key, all at once:
    ///
    /// - A skill a worker is asked to perform whose file is not in the form every harness reads a skill in: its directory holds no `SKILL.md`, or one that opens with no frontmatter, or whose frontmatter says another `name` than the directory's, or no `description`. The key is the `do` that names the skill — `stages[1].do`, `stages[1].escalate.do`, `land.do`, a patrol's `do` — and each `do` that names it has the fault. The form is in `afkconvoy help skill-file`. A skill no `do` names is not held to it.
    /// - A roster entry that lets a role run a harness it holds no limits for, while it holds limits for another: the fault names the role and both harnesses, and its key is that `harness` of the entry (`roster.dev.run[1].harness`). A role with no limits at all is no fault. A role the roster names no entry for ranges over the host kit, less what it holds no limits for: that is no fault, and `left_out` names it.
    ///
    /// A warning, on stderr, for each workflow whose file says nothing above its yaml block: that prose is what `afkconvoy flows` answers as what the workflow is for, and whoever hands the fleet work chooses a workflow by it. The team loads all the same.
    ///
    /// A note, on stderr, for each role with something in `above`: a session opened by hand in the role's directory loads those rules too, and a worker, given the role's directory alone, does not. The team loads all the same, and it exits 0.
    ///
    /// A note, on stderr, for each worker on codex whose role holds an `.agents/skills/` or a `.codex/skills/` itself, the skill the worker is asked to perform not in either: Codex finds skills there and not in `.claude/skills/`, the fleet leaves what a role holds there as it is, and the worker would not be shown that skill. A role that holds neither is given a link to its skills, in the fleet's copy, and nothing is noted. The team loads all the same, and it exits 0.
    ///
    /// A note, on stderr, for a team whose directory holds a `.claude/`: the hooks and skills there are loaded by no worker. Of a team's directory a worker is given the rules, and a hook and a skill are a role's. The team loads all the same, and it exits 0.
    ///
    /// Other errors, as {"error": …}: the repo's config names no team and no flag was given; the config cannot be read, the error naming its file and the key — a `harness` there is one, which a config names no more; the team's repo cannot be read, or the team's path is no directory at its head.
    ///
    /// ## What trips you up
    ///
    /// - From GitHub it reads what is pushed to the default branch. An edit in a checkout of the team is checked with `--dir`; a role's is checked only once it is pushed, wherever the role is — in the team's own repo too.
    /// - A role's `range` is this host's: run `check` on the host the fleet runs on, after `afkconvoy kit`.
    /// - Only `check` holds a team to its harnesses, and a skill's file to its form: a tick loads the team and starts its workers without asking again.
    /// - Following `pass` alone from any stage must end. A loop back is made with `reject`, which the breakers count; a loop of passes nothing would ever stop.
    /// - A stage nothing leads to is reported only once every `pass` and `reject` names a stage: fix those first and run it again.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Check {
        #[command(flatten)]
        team: TeamArgs,
        #[command(flatten)]
        at: At,
    },
    /// Write the skeleton of a team with its roles, or of one role, or a whole example team, and create its repo on GitHub if asked
    ///
    /// ## When to run it
    ///
    /// To start a team from files `check` accepts as they are, instead of from an empty repo. A team and its roles, in one repo created on GitHub:
    ///
    /// ```
    /// afkconvoy scaffold team hospital --role fellow --role attending --repo acme/hospital
    /// afkconvoy check --team acme/hospital
    /// ```
    ///
    /// And to add a role to a team that is there, in a checkout of its repo:
    ///
    /// ```
    /// afkconvoy scaffold role roles/reviewer
    /// ```
    ///
    /// From there on a team is designed by editing its files and running `check`.
    ///
    /// And to see a finished team first, or to start from one — a whole team the binary holds, written as one repo holds it:
    ///
    /// ```
    /// afkconvoy scaffold team hospital --example hospital --repo acme/hospital
    /// afkconvoy help example
    /// ```
    ///
    /// Read that one first when the team is to have more than one stage — a review, a landing role of its own: a skeleton's workflow has one stage, and a skeleton holds no skill to review or to land with. The example holds each, to copy from.
    ///
    /// ## Who runs it
    ///
    /// The operator, or whoever designs the team: anywhere. It reads no config.
    ///
    /// ## What it reads and writes
    ///
    /// It writes in the directory named, which must not be there yet or be empty: nothing is ever overwritten.
    ///
    /// `scaffold team <dir> --role <name>…` writes the team's files, and each role's under `roles/<name>/`:
    ///
    /// - `team.md`: the team file, naming each role as `./roles/<name>`
    /// - `CLAUDE.md`: the team rules: a heading, what belongs there and what belongs in a repo's own rules or a role's, and that the host user's rules give way to all of them
    /// - `workflows/feature.md`: one workflow, `feature`: one stage the first role performs as its skill `implement`, then a turn that lands its PR
    /// - `roles/<name>/`: that role's skeleton, as `scaffold role` writes it — the first role's with the skill `implement`, every other's with no skill
    ///
    /// `--role` is given once for each role, and at least once. `--role <name>` is a role written here. `--role <name>=<owner/repo>` or `<name>=<owner/repo/path>` is one that is elsewhere — a role several teams share, in a repo of its own: `team.md` names it as given and nothing is written for it. The two forms mix. The example workflow is performed by the first role given; one that is elsewhere must hold a skill `implement`.
    ///
    /// A role after the first is written with its rules and its limits, and no skill: a reviewer does not implement. What it is to perform is yours to write, as `roles/<name>/.claude/skills/<skill>/SKILL.md`, with the stage that names it in `workflows/feature.md` — that file says so too.
    ///
    /// `scaffold team <dir> --example <name>` writes an example team whole in place of a skeleton: its team file, team rules, workflows and patrols, and every role it names under `roles/<name>/`, with each role's rules, skills and hooks. The one name it takes is `hospital`: seven roles, three workflows and four patrols, which `afkconvoy help example` goes through file by file. It takes no `--role` — the team file names its own, each as `./roles/<name>` — and is a team's alone. A hook, like any file written that starts with `#!`, is written executable. Skills by other authors that the example's roles load are not written, and one of its stages runs on pi: `afkconvoy help example-run` says what a host needs before a fleet works with it.
    ///
    /// `scaffold role <dir>` writes a role's three kinds of file, and takes no `--role`:
    ///
    /// - `CLAUDE.md`: rules to replace with the role's own
    /// - `.claude/skills/implement/SKILL.md`: one example skill, `implement`: rename it, and rewrite it, for a role that does not; its form is in `afkconvoy help skill-file`
    /// - `.claude/settings.json`: its limits for claude-code — hooks: none yet
    /// - `.codex/hooks.json`: its limits for codex — hooks: none yet
    /// - `.kimi-code/limits.toml`: its limits for kimi — hooks and denied permissions: none yet
    /// - `.grok/config.toml`: its limits for grok — hooks and denied permissions: none yet
    ///
    /// The rules and the skills are written once, whatever harness a stage runs the role on. The role's limits are written once for each harness, in that harness's own form: for claude-code, hooks and denied permissions in `.claude/settings.json`; for codex, hooks in `.codex/hooks.json`; for kimi, hooks and denied permissions in `.kimi-code/limits.toml`; for grok, hooks and denied permissions in `.grok/config.toml`; and for pi, extensions in `.pi/extensions/`, a directory this command does not write. For dsh, a role writes its own overlay in `.dsh/cordis.patch.yml`; this command writes none. A file that names no hook holds no limit: the skeleton is right for every harness. `check` refuses a stage on a harness its role holds no limits for while it holds some for another.
    ///
    /// The role's name is the directory's. Run in a checkout, it writes there and creates nothing on GitHub: commit and push it with the team.
    ///
    /// With `--repo <owner/repo>` it then makes the directory a git repo, commits the skeleton, and creates that repo on GitHub with the commit pushed to it: `gh repo create <owner/repo> --private --source . --push`, run there. `--public` creates it public instead. Without `--repo` it runs no `gh`, and of `git` only the question of which checkout the directory is in: a role's path in `team.md` is written from the root of the team's repo, so a team written in a subdirectory of a checkout names its roles from that checkout's root.
    ///
    /// A skill is added to a role afterwards with the skills installer, run in the role's directory and committed: this command installs none.
    ///
    /// ## What it answers
    ///
    /// {"scaffolded", "dir", "files", "repo", "team_line"} — "team" or "role"; the directory written in, as an absolute path; the files written, as paths in it; the repo created, null without `--repo`; and, of a role, the line to add under `roles` in `team.md`:
    ///
    /// - scaffold role roles/reviewer: "team_line": "reviewer: ./roles/reviewer"
    /// - scaffold role dev --repo acme/dev: "team_line": "dev: acme/dev"
    ///
    /// `team_line` is null of a team, and of a role nothing says the repo of: one written outside any checkout with no `--repo`, or as the root of one.
    ///
    /// Errors, as {"error": …} with a non-zero exit: the directory is not empty; a team with no `--role` and no `--example`, or one name given twice; a role with a `--role` or an `--example`; an `--example` with a `--role`; an `--example` there is no example of, which lists the names there are; a `--role` that is neither <name> nor <name>=<owner/repo[/path]>; a file that cannot be written; and, with `--repo`, a `git` or `gh` call that failed — the error says the skeleton is written and its repo is not created.
    ///
    /// ## What trips you up
    ///
    /// - `check` reads every role from GitHub: a team's skeleton passes it once it is pushed — `--repo` does that — and not before.
    /// - `check --dir <dir>` alone refuses a role named `./path`: a directory says no repo. Say the team's beside it — `check --dir <dir> --team <owner/repo>` — as the skeleton's own files do.
    /// - A role that is elsewhere must be on GitHub, with its skill on its default branch, before the team that names it passes `check`.
    /// - After a failed `--repo` the directory is no longer empty. Create the repo yourself from it with the `gh` line above; do not run this again on it.
    /// - `--repo` commits as whoever git is set up for on this host, and creates the repo as whoever `gh` is logged in as. Every host of a fleet must be able to read it.
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Before opening an issue the fleet is to work, and again whenever the team gains a workflow: it says which workflow takes which kind of issue, what label puts an issue in it, and which labels to keep off — and it creates the labels that takes, so the line it gives works. Run it once for all the issues you open, not once per issue.
    ///
    /// ```
    /// afkconvoy flows
    /// ```
    ///
    /// ## Who runs it
    ///
    /// Whoever hands the fleet work: a human, or the agent a human is talking to, with work decided on and issues to open for it. In a checkout of the repo the issues go to, bound or unbound. A worker opening a blocker for another repo of the host names that repo's checkout: `afkconvoy flows <target-checkout>`. It takes the issue in there with `afkconvoy intake`.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the config of the checkout and the labels its tracker has. The tracker is the one this host's checkout list says for the checkout, or for the checkout it is a worktree of — the checkout's own repo, until its host says another (`afkconvoy run --tracker`). Where the list holds no entry of it — another account on a shared machine, a fresh clone — the tracker is not said, and it is refused: name it with `--tracker <owner>/<repo>`, the checkout's own repo where that is its tracker. On an entry, a `--tracker` the entry does not say is refused, naming both. In a bound repo it also reads the team as `afkconvoy check` does — the team's files and every role's repo, from GitHub at the head of each default branch — and, to count, the comments of every open issue that carries a flow label.
    ///
    /// It writes one thing: each label its lines name that the tracker lacks. In a bound repo that is the flow label of every workflow of the team — `flow:<workflow>`, or the one its file's `label` names; in an unbound one, the ready label; and, where `--tracker` names another repo off the list, the repo label. It puts no label on any issue and opens none. Everything is read first: a team with a fault creates nothing.
    ///
    /// With `--delivery <identity>`, it first validates an existing same-repo Jira delivery. Its `line` keeps the tracker and repo label but omits the runnable flow or ready label. Run that line, then its `intake_line` with the returned issue number: intake attaches the delivery before adding the selected workflow's label (or the unbound repo's ready label). Both steps run in this code repo's checkout. The answer includes `delivery`; each workflow, or the unbound answer, includes `intake_line`. An invalid or cross-repo delivery refuses before creating labels. Without --delivery, today's ordinary creation line is unchanged.
    ///
    /// ## What it answers
    ///
    /// In a bound repo:
    ///
    /// {"team", "workflows", "keep_off", "intake", "created"}
    ///
    /// With --delivery: also `delivery`, and each workflow's `intake_line` to run after its non-runnable creation line. In an unbound repo, `delivery` and `intake_line` are top-level keys.
    ///
    /// - `team`: the team's repo
    /// - `workflows`: every workflow of the team, each {"name", "flow_label", "description", "first", "ends", "line"}:
    ///
    ///   - `description`: what the workflow is for and what an issue of it holds: the prose of its file, workflows/<name>.md, above the yaml block. Empty when the file says nothing there — `afkconvoy check` warns of that. Choose the workflow by it
    ///   - `first`: the stage an issue enters at, {"stage", "by", "do"}: the role that performs it and the skill — `by` is `human`, and `do` null, where a human's decision is waited for
    ///   - `ends`: where its passes end: `land`, a landing turn — its issues end in a PR landed on their target branch — `done`, nothing more to do, or both, by different stages
    ///   - `line`: the line that opens an issue into it, in the tracker. Give it a title and a file holding the body
    /// - `keep_off`: the labels that keep an issue from being worked as one of a workflow, {"epic_labels", "escalate_label", "lock_label"}: an issue carrying an epic label is never dispatched; the escalate label is the fleet's to put on, when it hands an issue to a human; and an open issue carrying the lock label is a fleet lock, which stops all other work (`afkconvoy lock --help`)
    /// - `intake`: {"open", "limit"}: the open issues workers took in, against the repo's `intake_limit`. An issue a human opens with a flow label is outside both: nothing refuses it
    /// - `created`: the flow labels this run created on the repo; empty when it had them all
    ///
    /// In an unbound repo, which has no workflow:
    ///
    /// {"team": null, "ready_label", "line", "keep_off", "created"}
    ///
    /// - `ready_label`: the label an issue is worked under
    /// - `line`: the line that opens an issue carrying it
    /// - `keep_off`: as above; the escalate label is the config's
    /// - `created`: the ready label, when this run created it; else empty
    ///
    /// Errors, as {"error": …} with a non-zero exit: "the tracker is not said" — the checkout is on no checkout list of this host, and no `--tracker` names it; a `--tracker` its entry does not say; the team has a fault — {"error", "faults"}, as `check` answers it; the team's repo cannot be read; the config cannot be read; "is not in a git checkout", or git's own words for a checkout it refuses; GitHub refused to list or create a label.
    ///
    /// ## What trips you up
    ///
    /// - `gh issue create --label` refuses a label the repo lacks, and nothing but this command creates a flow label ahead of the first issue. Run it before the line.
    /// - Opening an issue with `gh`, or labelling one, wakes no fleet: the fleet takes it up at its next tick — within {const:TICK_BUSY_SECONDS} seconds while it has work, within {const:TICK_IDLE_MINUTES} minutes once it has been idle {const:TICK_IDLE_AFTER} ticks in a row. `afkconvoy wake <checkout>`, on the fleet's host, starts that tick now: run it once, after the last issue you open.
    /// - An issue follows one workflow: put one flow label on it. With several it follows none, and no stage of any is performed on it.
    /// - The flow label is what hands the issue over. Leave it off an issue that is not ready to be worked, and put it on later: `gh issue edit <n> --add-label <label>`.
    /// - Where the tracker is another repo, the line opens the issue there, `--repo <owner>/<tracker>`, with this repo's repo label, `repo:<owner>/<repo>`: an issue there without it is no issue of this repo, and its fleet never works it. Name the tracker in every `gh issue` command (`--repo`): run in the checkout, `gh` acts on the checkout's own repo.
    /// - A workflow whose `first` is `by: human` starts by waiting for a human: the fleet does nothing on its issue until one declares a verdict (`afkconvoy verdict --help`).
    /// - It answers the team as pushed to its default branch. A workflow added in a checkout of the team is not answered, and its label not created, until it is pushed.
    /// - A label of a workflow the team no longer has is left on the repo: this command creates labels and removes none.
    #[command(next_help_heading = HANDS_WORK, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Once, after the last of the issues you open for the fleet with `gh`, or label: nothing `gh` does wakes the fleet, which otherwise takes them up at its next tick (`afkconvoy flows --help` says how soon).
    ///
    /// Once, as a worker's last command: after its PR is on its target branch, or its verdict is posted on the issue. A worker's prompt names the exact command. Every command that changes fleet state wakes the `run` as it ends, so after `land` or `verdict` this one is redundant, and harmless.
    ///
    /// ## Who runs it
    ///
    /// Whoever hands the fleet work: a human, or the agent a human is talking to, after opening issues with `gh`, in a checkout of the repo on the fleet's host. And a worker, in its own worktree. Any worktree or checkout of the repo on the fleet's host reaches the same fleet instance.
    ///
    /// ## What it reads and writes
    ///
    /// It speaks to this host's runner, on its socket in the cache directory (host-runner.sock): one signal for the fleet instance it holds of this repo, naming its checkout, itself, and the issue the worker is on if its environment says one. It waits {const:WAKE_ANSWER_SECONDS} seconds at most for the answer. The signal ends that fleet instance's sleep, and no other's; it carries nothing else, and it writes nothing to GitHub: the tick it starts reads the outcome from GitHub itself.
    ///
    /// ## What it answers
    ///
    /// {"woke": true, "tick"} — no tick was in progress: tick `tick` starts now. The fleet instance's tick log says "tick N: started (woken by wake #12)" of it.
    ///
    /// {"woke": true, "after_tick"} — tick `after_tick` is in progress: the next starts as it ends. The fleet instance's tick log says "woken by wake #12 during tick N: tick N+1 follows" of the first such wake.
    ///
    /// {"woke": false, "detail"} — no fleet instance heard it. With no host runner on this host, `detail` is "no host runner runs on this host, so no fleet instance heard it: start one with `afkconvoy run <checkout>`". Otherwise the host runner holds no fleet instance of this repo, holds it broken — waiting to be started again — or did not answer within {const:WAKE_ANSWER_SECONDS} seconds: `detail` says which, with the line that fixes it — `afkconvoy run <checkout>`, or ask `afkconvoy doctor`.
    ///
    /// It exits 0 with each of them. Errors: only "is not in a git checkout", or git's own words for a checkout it refuses. Failing to wake anybody is not an error, and costs at most the {const:WAKE_ANSWER_SECONDS} seconds it waits.
    ///
    /// ## What trips you up
    ///
    /// - It reports nothing. A worker's outcome is its landed PR or its verdict comment: without one of them on GitHub, a wake only makes the fleet look sooner at a worker that stopped with nothing.
    /// - `woke: false` needs no second `wake`: the fleet finds the outcome, or the issue, at its next tick anyway.
    /// - It reaches a fleet instance of this host's runner only: run it on the host whose `run` works the repo. A fleet driven by `tick` hears nothing.
    #[command(next_help_heading = HANDS_WORK, verbatim_doc_comment)]
    Wake {
        #[command(flatten)]
        at: At,
    },
    /// Append one after-the-fact entry to the daybook: what an agent did or decided outside any issue
    ///
    /// ## When to run it
    ///
    /// When an agent — an interactive session, the watch, the asking session or a worker — did or decided something no issue holds: a host fix or install, a ruling made without a ticket, a change made at the operator's word. Record it and carry on: the operator reads the daybook when they choose, and nobody waits on it. What belongs to an issue goes on the issue, not here.
    ///
    /// ## Who runs it
    ///
    /// The operator, or any agent acting on the host for them: the agent a human is talking to, the watch, the asking session, or a worker, in its own worktree. Each says what it did in its own entry; none reads the daybook to decide what to do next.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the tracker's discussion categories and its latest discussions of `Daybook`, and this host's checkout list for the tracker.
    ///
    /// One comment on today's discussion of the tracker, in its discussion category `Daybook`: one discussion a day, titled with the host's local date, `YYYY-MM-DD`, created by the first entry of the day. Two entries that both find no discussion of the day both still go to one: the discussion of the day with the lowest number is the day's, and a run that created a second deletes its own.
    ///
    /// Every entry has one shape: the repo it is of and the kind of session that wrote it — `interactive`, `watch`, `asking session` or `worker`, told by its environment — then the headings `Did`, `Why`, `Undo`, `Lesson` and `Links`, each holding what was said for it. A heading said of nothing is left out. The text given alone goes under `Did`.
    ///
    /// A **lesson** (`--lesson`) is one sentence the agent, at the moment, thinks should outlast the day: "the gate on this host needs X", "this kind of edit breaks Y". Writing it does not interrupt the work: it is raw material for the weekly distillation, which carries each lesson to its lasting home — a check, a hook, a rule, a doc. An entry with a lesson also rebuilds the lesson index at the foot of the day's discussion body — each lesson of the day and its entry's link — from the day's entries, so `--lessons` lists a span's lessons from the days' bodies without fetching an entry.
    ///
    /// `--lessons --since <YYYY-MM-DD> [--until <YYYY-MM-DD>]` writes nothing: it lists the lessons of the days from `--since` to `--until`, both included (to today where `--until` is not given), from the days' indexes.
    ///
    /// The **distillation** is the host's watch's: at most once a week, when a span of days holds a lesson or two entries without one — enough for a fault to recur — a watch round reads the span's lessons and entries, decides each lesson's lasting home and proposes it there through that home's normal path: a repeated mistake first as a mechanism, a ticket to the repo that owns it; one project's decision or term, a ticket for its ADR or GLOSSARY; a cross-project one, a change to the control repo's; how to use a tool, a ticket for its own skill; the operator's global principles, a draft PR they read and no fleet merges. It keeps nothing of its own: the proposal is the record, and the entries are left to age out. `--distill` asks the watch for one at its next look without waiting out the week; it still runs only on a span that holds something, from the day after the last distillation's span to today.
    ///
    /// `--distilled <file>` is how the round records it: the JSON file says {"since", "until", "carried": [{"entries": [entry URL…], "lesson", "home", "link"}]}, `home` one of `ticket`, `doc` (a control ADR or GLOSSARY change), `draft` (a draft PR on the global principles) or `covered` (its home holds it already, `link` saying where). Each lesson of the days `since` to `until` is carried exactly once, by its entry's URL; its `lesson` may be left out. A pattern recurring across entries without a lesson names two entries at least and says its `lesson`. The file is checked against the span's lesson indexes before anything is written, then one entry is written whose `Did` maps each lesson to where it went.
    ///
    /// The repo is the one the checkout's remote names, or `--repo`. The tracker is the one this host's checkout list says for the checkout, its own repo where it says none, or `--tracker`. Run in no checkout, both flags are needed.
    ///
    /// It is not fleet state: nothing reads it to decide what a fleet does.
    ///
    /// ## What it answers
    ///
    /// {"url", "discussion", "day", "created"}
    ///
    /// - `url`: the entry's comment
    /// - `discussion`: the day's discussion
    /// - `day`: its title, the host's local date
    /// - `created`: whether this run created it
    ///
    /// With `--lessons`: {"lessons": [{"day", "url", "lesson"}], "since", "until"}, oldest day first.
    ///
    /// With `--distilled`: the entry's answer, and "since", "until", "lessons" (how many the span holds) and "carried" (how many lessons and patterns the file carried). With `--distill`: {"asked", "at"}, the epoch second the watch was asked at.
    ///
    /// Errors, as {"error": …} with a non-zero exit, each before anything is written but the last: an entry that says nothing; a distillation file that carries a lesson of its span nowhere or twice, or a pattern with no `lesson` or fewer than two entries; no repo or tracker to name, run in no checkout; the tracker has no `Daybook` category — the error names the page that creates one, as GitHub creates no category through its API, and the line that turns its discussions on where they are off; GitHub refused to read or create the discussion, or to add the comment.
    ///
    /// ## What trips you up
    ///
    /// - The category is created by hand, once, in the tracker: https://github.com/<owner>/<tracker>/discussions/categories/new, named `Daybook`.
    /// - An entry is written once: a second run writes a second entry. To correct one, edit its comment on GitHub; to correct its lesson in the index too, edit the index line in the day's discussion body, or write the day's next lesson, which rebuilds the index from the entries.
    /// - Two lessons written at once can each rebuild an index the other's is missing from. Each run checks the index holds its own lesson and rebuilds it until it does, and the day's next lesson restores any other; an index that still lacks a run's lesson is an error naming the entry, which was written.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Once for each agent of your own that is to learn of afkconvoy: it prints the skill file to install into it. The file says when to reach for afkconvoy, and sends the agent to `afkconvoy --help`. It repeats nothing the help says, so it has nothing to go stale: a newer release asks for no newer file.
    ///
    /// Where to put it, for Claude Code — for a user, on every project of theirs:
    ///
    /// ```
    /// mkdir -p ~/.claude/skills/afkconvoy
    /// afkconvoy skill > ~/.claude/skills/afkconvoy/SKILL.md
    /// ```
    ///
    /// and for a project, from its root, committed for everybody who works in it:
    ///
    /// ```
    /// mkdir -p .claude/skills/afkconvoy
    /// afkconvoy skill > .claude/skills/afkconvoy/SKILL.md
    /// ```
    ///
    /// ## Who runs it
    ///
    /// Whoever sets an agent up: a human, or the agent a human is talking to. In any directory: it is asked nothing about a repo.
    ///
    /// ## What it reads and writes
    ///
    /// Nothing. The file is part of the binary, and it is printed: where it goes is yours to say, with a redirection as above.
    ///
    /// ## What it answers
    ///
    /// The file's text and no JSON object: a frontmatter of `name` and `description`, as a Claude Code skill needs, and under it the one instruction. It exits 0.
    ///
    /// Errors: none of its own.
    ///
    /// ## What trips you up
    ///
    /// - It installs nothing. Printed to a terminal, the file is nowhere an agent reads it.
    /// - A worker needs none of it: a worker is told what to run by its prompt and its role.
    /// - The directory is named as the skill is: `afkconvoy`, holding `SKILL.md`.
    #[command(next_help_heading = SETS_UP, verbatim_doc_comment)]
    Skill,
    /// Draw a team's workflows as graphs, or say where one issue is in its workflow — in an unbound repo, on its one path
    ///
    /// ## When to run it
    ///
    /// After `check` accepts a team, to review a workflow as a whole: where an issue enters, which role performs each stage, and where every `pass` and `reject` leads. Paste it into a GitHub comment inside a code block to have it reviewed there.
    ///
    /// With `--issue`, to ask where one issue is in its workflow: how it came in, its position, what follows it, and the rework it has left before a breaker stops it.
    ///
    /// ```
    /// afkconvoy show --issue 7
    /// ```
    ///
    /// Or `afkconvoy show 7`: a workflow is not named by a number, so one given is an issue's.
    ///
    /// In an unbound repo, to ask where an issue stands on the one path every issue there takes — ready, held back, claimed, landing, waiting on a human, or closed. An unbound repo has no workflow to draw: `show` with no issue there is refused.
    ///
    /// ## Who runs it
    ///
    /// The operator, or whoever designs the team: in a checkout of a bound repo, or anywhere with `--team` or `--dir`. With an issue, also the operator in a checkout of an unbound repo.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing. It reads a team as `check` does, with the same `--team` and `--dir`: the team's files, and every role's repo from GitHub. See `afkconvoy check --help`. With a workflow's name it draws that one; with none, every workflow of the team.
    ///
    /// With `--issue` it also reads, in a checkout of the issue's repo, the issue's labels and its comments. The workflow is the one the issue's flow label names — for an escalated issue, whose flow label was swapped out, the one its escalation's comment says it left. The position is read off the issue's stage records — the comments `afkconvoy verdict`, `rule` and `send` write — and off nothing else: not its `stage:` label, which the fleet writes for humans to filter by and never reads. How it came in is read off its intake record, the comment `afkconvoy intake` writes.
    ///
    /// In an unbound repo, with no `--team` or `--dir`, it reads no team: it reads what `afkconvoy plan` reads — the open issues and their labels, blockers and comments, the open PRs, and the claims and heartbeats on the remote — as of no fleet instance, every claim a peer's.
    ///
    /// ## What it answers
    ///
    /// Not JSON: a drawing, as plain text. One workflow:
    ///
    /// ```
    /// feature — flow:feature — max_rework 3, max_consecutive_rework 2
    ///
    /// ┌───▶ plan      dev: plan
    /// │       ↓ pass → build
    /// │ ┌─▶ build     dev: implement · escalate → reviewer: rule
    /// │ │     ↓ pass → review
    /// │ └── review    reviewer: review · writes nothing · reject → build
    /// │       ↓ pass → sign-off
    /// └──── sign-off  human · reject → plan
    ///         ↓ pass → land
    ///       land      dev: land
    ///
    /// roster
    ///
    ///   dev       claude-code opus (most work) · claude-code fable (hard or wide work)
    ///             cast: Hard or wide work on fable; the rest on opus.
    ///   reviewer  the host kit whole
    /// ```
    ///
    /// The first line is the workflow, its flow label and its two breakers. Then one row per stage, in the order its file lists them — an issue enters at the first — with who performs it, as `<role>: <skill>` or `human`, and what else its file sets. Under each stage is where its `pass` leads. A `reject` is said on its stage's row and drawn as the line that leaves that row to the left and comes back in, at the arrow, to the stage it leads to. The last rows are the ends: `land` with who lands, and `done`. Under the drawings, once, is the team's roster: a row per role, each harness and model it may run in order of preference — `any model` where the entry names none — with what it is good at; `the host kit whole` for a role the roster names no entry for (`afkconvoy help roster`). What a role's range is on this host is `afkconvoy check`'s to answer.
    ///
    /// With `--issue`, JSON:
    ///
    /// {"issue", "workflow", "intake", "position", "follows", "rework", "consecutive_rework", "records", "refusals"}
    ///
    /// Also `delivery` (null for an ordinary task) and its resolved `target_branch`.
    ///
    /// - `intake`: how the issue came in. {"by": "worker", "role", "from", "url"}: a worker in that role, on the issue `from`, took it in with `afkconvoy intake`, and `url` is the comment that records it; `patrol` in place of `from` where a patrol's worker did. {"by": "human"}: its flow label has no intake record for it
    /// - `position`: where the issue is: {"is", "stage", "by", "do", "harness", "reason"}, as `afkconvoy rehearse --help` spells out
    /// - `follows`: the position a `pass` declared now would lead to; null where none can be declared: the issue is landing, ended, escalating or with a consult
    /// - `rework`: {"had", "left"}: its rework so far, and what `max_rework` still allows
    /// - `consecutive_rework`: for each stage that may reject, {"had", "left"}: its rejections in a row, and what `max_consecutive_rework` still allows
    /// - `refusals`: the harnesses that refused this issue, each with `harness` and the refusal line as `evidence`; every later start skips them at every stage
    /// - `records`: its stage records, oldest first: {"stage", "verdict", "reads", "by", "claim", "role_commit", "team_commit", "harness", "url"}
    ///
    /// Of a record, `by` says who left it: `worker` or — asked to rule on the stage's `escalate` — `consult`, under the `claim` and the two commits it names, as a session of the `harness` it names, or `human`, for whom all four are null. `verdict` is what it states: one of the five verdicts, `ruling`, or `sent`. And `reads` says how it counted:
    ///
    /// - `verdict`: it is the verdict of the stage its claim was dispatched to, or what a human declared or ruled at the stage the issue was at
    /// - `sent`: a human sent the issue to its stage
    /// - `other-stage`: it names a stage the issue was not at: it counts for nothing
    /// - `claim-spent`: a verdict was already declared under its claim: it is not one
    /// - `unknown-stage`: it names a stage the workflow no longer has: the issue escalates
    ///
    /// In an unbound repo, with an issue, JSON:
    ///
    /// {"issue", "workflow": null, "stands", "claim", "pr", "delivery", "target_branch"}
    ///
    /// - `stands`: {"is", …}, where the issue is on its one path, from the same facts a tick acts on. `is` is `ready`: a worker can be started on it now; `held`, with the `reason` `plan` gives it in `excluded` — no ready label, an epic label, open blockers, a fleet lock, an open linked PR not the fleet's; `claimed`: a fleet instance holds it, its worker at work; `landing`: a fleet instance holds it and its PR is the fleet's, which waits to land in a turn; `waiting_on_human`, escalated or a failed run's report, with the `why`, `url` and `answer` of its row in `plan`'s `waiting_on_human`; or `closed`, with GitHub's `state_reason` and `closed_by`
    /// - `claim`: {"instance", "live"} while a fleet instance holds the issue: which one, and whether its heartbeat is within the lease — a claim that is not is work for a live fleet instance to take. Null while none does
    /// - `pr`: the open PR that closes the issue, {"number", "branch", "fleet"}, `fleet` whether its branch is one the fleet recorded on the issue: only such a PR is landed by the fleet. Null while there is none
    ///
    /// Errors, as {"error": …} with a non-zero exit: the team has a fault — {"error", "faults"}, as `check` answers it; the team has no workflow of that name, or none at all; the issue has no position — it carries several flow labels, one that names no workflow of the team, or none with no escalation that says which it left; there is no such issue (`no such issue: #7 (HTTP 404)`), or GitHub says it was deleted or transferred; and every error of `check`.
    ///
    /// ## What trips you up
    ///
    /// - It draws only a team that loads. Run `check` first: a fault is reported, not drawn.
    /// - `blocked`, `already-satisfied` and `escalate` are not drawn: they mean the same at every stage. `afkconvoy rehearse` shows where each leads.
    /// - `--issue` reads the team as it stands now, not as it stood when a record was written: a workflow changed since can leave records that name no stage of it.
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// When a team has no roster yet, or the host kit has changed and its roster is to follow: a team's roster is drafted rather than written from nothing. Run it on a host whose host kit holds what the team's workers are to run (`afkconvoy kit`).
    ///
    /// ## Who runs it
    ///
    /// The operator, or whoever designs the team, in a checkout of it: the team's working tree. The fleet never runs it, and never changes a team's roster.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the team's files from `--dir` — this directory unless it says another — as `afkconvoy check --dir` does, each role from GitHub, and this host's host kit without the harnesses paused on it. A role named as ./path is read from the repo `--team` names: give it for a team whose roles are in its own repo.
    ///
    /// 1. It sends no prompt. `good_at` comes from the host kit's research.
    /// 2. It drafts the team file's `roster` (`afkconvoy help roster`): an entry for each role, its `run` every placed harness and model of the host kit in the host kit's order, each with the `good_at` from the host kit. It writes no `cast` line. A harness the role holds no limits for, while it holds limits for another, is left out of its entry and named in a comment there, so that the draft passes `check` as written; a role left nothing is named in a comment alone. A roster `cast` key already there is not read.
    /// 3. It writes the draft into the team file, team.md, in place of the `roster` there and everything under it, or at the end of its yaml block where it has none. Nothing else of the file changes, and nothing is committed.
    ///
    /// Where the team file holds a roster, it says on stderr each line the draft would take out (-) and put in (+), and writes only once the human at the terminal confirms, or with `--yes`. With no terminal and no `--yes` it writes nothing.
    ///
    /// ## What it answers
    ///
    /// {"path", "roster", "replaces", "removed", "added", "unsaid", "written"}: `path` the team file, `roster` the draft as written, `replaces` whether the team file held a roster, `removed` and `added` the lines the draft takes out and puts in, `unsaid` what the host kit's research says nothing of — each also a warning on stderr, to write in by hand — and `written` whether the team file was written. A roster the same as the draft is not written again.
    ///
    /// Errors: a team that does not load, with every fault, as `check` answers it; a host kit that cannot be read or holds no harness.
    ///
    /// ## What trips you up
    ///
    /// - Read the draft before committing it: model strengths come from the host kit's sourced research. Edit them, take out what a role is not to run, and order what is left as the team prefers — `afkconvoy check --dir` then says whether the team loads.
    /// - A team file whose roster has a fault does not load: delete the roster, and draft again.
    /// - The draft is the host kit of the host it runs on. A harness another host holds and this one does not is not in it.
    /// - Drafting sends no prompt.
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// While designing a workflow, to test it without starting a worker or spending anything: name the workflow and what is declared on an issue, in order, and it says where the issue would be after each and where it ends. Use it to see that a rejection leads back where you meant, and how many it takes to trip a breaker.
    ///
    /// ```
    /// afkconvoy rehearse feature pass pass reject pass pass
    /// ```
    ///
    /// ## Who runs it
    ///
    /// The operator, or whoever designs the team: in a checkout of a bound repo, or anywhere with `--team` or `--dir`.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing and starts nothing. It reads a team as `check` does, with the same `--team` and `--dir`: see `afkconvoy check --help`. No issue is read: the issue is one that has just entered the workflow, at its first stage.
    ///
    /// After the workflow's name comes what is declared, each by whoever the issue is with at that point. The five verdicts:
    ///
    /// - `pass`: follows the stage's `on.pass`: to a stage, `land` or `done`
    /// - `reject`: follows the stage's `on.reject`, back to an earlier stage
    /// - `blocked`: parks the issue; the same stage runs again afterwards
    /// - `already-satisfied`: ends the workflow
    /// - `escalate`: goes to the stage's consult if its `escalate:` names one, else to a human
    ///
    /// and `ruling`: what a consult, or a human on an escalated issue, decides. The stage that asked runs again under it.
    ///
    /// ## What it answers
    ///
    /// {"workflow", "max_rework", "max_consecutive_rework", "enters", "steps", "ends"}
    ///
    /// - `enters`, `ends`: the position before anything is declared, and after everything
    /// - `steps`: one per thing declared, in order: {"declared", "stage", "valid", "breaker", "position", "rework", "consecutive_rework"}
    ///
    /// Of a step, `stage` is the stage it was declared in, null once the issue is landing or ended, and `position` where the issue is after it. A position is {"is", "stage", "by", "do", "harness", "reason"}:
    ///
    /// - is: perform: a worker is to perform `stage`, as the skill `do` of the role `by`
    /// - is: waiting: `stage` is a human's (`by: human`): it waits for their verdict
    /// - is: consult: a worker is to rule on the `escalate` of `stage`, as `do` of `by`
    /// - is: parked: parked at `stage`, which runs again when its blockers close
    /// - is: landing: the workflow's `land` lands in a turn, as `do` of `by`
    /// - is: ended: nothing more to do
    /// - is: escalating: handed to a human, with the `reason`; under their `ruling` the `stage` runs again
    ///
    /// `harness` is the harness the worker of the position is a session of, null where no worker acts: the stage's — the one it names, or the default of this checkout's config — for a worker that performs it, and the default for a consult and a landing.
    ///
    /// `valid` is false for what may not be declared where the issue is: a `reject` from a stage with no `on.reject`, a `ruling` nobody asked for, anything but `ruling` or `escalate`, `close` or `park` from a consult. An invalid verdict escalates the issue at a stage. Landing accepts `blocked` and `escalate`; a ruling returns an escalated issue to Landing. Other declarations at landing, anything declared on an ended issue, and anything but a `ruling` on one that is escalating change nothing.
    ///
    /// `rework` is the issue's rework in total: its rejections and its consults' rulings. `consecutive_rework` is, for each stage that may reject, its rejections in a row: a `pass` of that stage starts its count over. A human's `ruling` starts every count over. `breaker` is null, or the breaker this step tripped — "max_rework" or "max_consecutive_rework": the rework that would have passed it is refused and not counted, and the issue escalates at the stage that would have run again.
    ///
    /// Errors, as {"error": …} with a non-zero exit: a word that is none of the six; the team has no workflow of that name; the team has a fault — {"error", "faults"}, as `check` answers it; and every error of `check`.
    ///
    /// ## What trips you up
    ///
    /// - It rehearses only a team that loads. Run `check` first.
    /// - A count never passes its breaker. With `max_rework: 3` the fourth rework is the one refused: the step says `"breaker": "max_rework"` and `"rework": 3`.
    /// - Running again after `blocked` is no rework.
    #[command(next_help_heading = DESIGNER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// When something looks wrong, and before anything else: a fleet that seems stuck, an issue nobody works, a worker that sits there — or when you are asked "is it working?". Run it before you file a bug, too, and put its answer in the report. It is one command for what `scan`, `takeover --list`, `plan`, `show` and orca each say a part of, and it is the only one that says what is wrong.
    ///
    /// With `--all`, for every repo of this host at once: each checkout on the host's checkout list, as `doctor` in it answers. A session that looks after every repo of the host runs it, beside `afkconvoy checkouts`, and keeps no list of its own.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the fleet runs on. No `--instance` is needed: it finds the fleet instance of this host's `run` itself. With `--all`, anywhere on that host.
    ///
    /// ## What it reads and writes
    ///
    /// It reads this host (the commands the fleet needs, in a login shell; orca), the log this host's `run` keeps of its ticks, the open issues, PRs and stage records on GitHub, whether GitHub says the tracker is public, the claims, heartbeats and recorded gate runs on the remote, and orca after the worker of each claim. It writes nothing on GitHub and nothing on the remote, and makes no startup probe: it is safe beside a live fleet, as often as you like. It fixes nothing either: each finding names the command that does.
    ///
    /// A harness whose first login answer reads logged out is asked once more in that same login shell setting before `tools` reports `logged_out`; ready on the second answer is ready. A ready harness is still started at most once per doctor. A tool finding's evidence carries the last answer's exit status and the first 500 characters it printed, with credential-shaped text redacted.
    ///
    /// With `--all` it reads the checkout list (checkouts.json in the cache directory) and the same of each checkout on it, all at once.
    /// It also reads every tracker that list names once for repo-labelled work, including repos absent from this host. A repo nobody works has an open ready issue with no open blocker or claim, ready for more than an hour since the later of its dispatch label being applied and its last blocker closing, and no remote heartbeat within the 1800-second lease. Ready means the repo's ready label, or its team's flow labels. Only an old enough unblocked issue, or an existing report, asks that repo's heartbeat remote. `host.unworked_repos` holds the waiting issues and their seconds, newest heartbeat's host and time (or null), report and opening host, recovery host, and read error. An unreadable remote proves nothing about whether a fleet works it. Doctor opens and closes nothing; the host runner reconciles these reports at its look.
    ///
    /// ## What it answers
    ///
    /// {"host", "fleet", "claims", "issues", "waiting_on_human", "refusals", "candidates", "findings", "deliveries"}
    ///
    /// `targets` lists open tracker tasks with issue, tracker_issue, delivery (null when unattached), target_branch, configured_base and pinned; an invalid record or missing branch adds error rather than selecting the configured base. A failed listing is {error}.
    ///
    /// `deliveries` lists each Jira delivery with its final PR or pending reason, whether recorded delivery evidence certifies the current delivery head, final-target head and what each attached task came to, and any settlement. Reading writes nothing.
    ///
    /// - `host`: what this host has of what the fleet needs:
    ///
    ///   - `tools`: each command needed — a harness only where a worker launch command the fleet could be asked for runs it by name: {name, state} — state is ready, missing or logged_out; of orca also not_running, knows_no_checkout (the checkout worktrees are cut from was never added to it) or unasked
    ///   - `worker_commands`: the host kit commands and their callers. The first eligible kit harness is selected for each role. A worker's model is its model label where followed, otherwise that harness's user default.
    ///   - `host_kit`: {path, refreshed, harnesses, paused, worker_health}: where the kit is kept; when it was last written, in epoch seconds — by `kit`, by `run`, or by a tick's daily refresh (`afkconvoy kit --help`); each harness in order, first first, with its full worker launch command, its models in order as {model, names}; `default_model` (it has no placed model, so a worker runs its login's default, with no model flag) and `research_pending` (it holds a model awaiting research). good_at, settled, sources and researched_at stay on the kit and on `plan`'s `host_kit` (`afkconvoy plan --help`); a look does not repeat them. `paused` is each harness paused on this host: {harness, since, why, probed, said}. `worker_health` is the book a worker-health finding is read from. {path, error} when unreadable. `afkconvoy kit` writes it again.
    ///
    ///     {const:KIT_RESEARCH_STATES}
    ///
    ///     A harness with `models` [] or no placed model runs on its default model with no model flag, and is no fault. Automatic command discovery, a switch, failed research or a hand-written kit can leave one. `afkconvoy kit` lists them; the watch supplies research through `afkconvoy kit --research <file>`. A tick lists an empty Codex entry without waiting for the daily refresh, with an hour between unsuccessful tries. A roster model or a model label that matches nothing placed is still reported as such.
    ///   - `settings`: the host settings (`afkconvoy run --help`): {file, said, in_force} — each by its key, the name of the flag of `run` that names it (`hot_cores` for `--hot-cores`), in that flag's units: said is what the host says, null where it says none and the default holds; in_force what holds for this repo's fleet on this host, each default worked out for it — the panel's port {const:PANEL_PORT} where the host says none. {file, error} where the file cannot be read
    ///   - `admission`: whether a tick would start a new worker on this host now, the thresholds in force and what the host reads: as `plan` answers it (`afkconvoy plan --help`), or {error} where a share the host says is wrong. A tick that admission held back is no "started no worker" finding.
    ///   - `exclusive`: the host's exclusive queue as the one observation `afkconvoy queue` answers — {observed_at, status, paused, worker, repos, running, waiting} — at doctor's observation time, from the host runner this look asked once: worker capacity, each repo lane, and every request with its `lane`, durations and scheduling exactly as `afkconvoy queue --help` documents. A request not yet started is in `waiting`, with its scheduling state; it is not marked `waiting_for_capacity`, as a landing turn's answer is. Each `command` is the one line `afkconvoy queue --help` describes: the program and the arguments that say what is running. A script is not shown as the script: a `sh -c` test script is named by its `cargo nextest` line, or by its `afkconvoy exclusive` invocation. The line carries no `NAME=value`. `status` is `no_runner`, with empty arrays, where no host runner listens; `paused` is null where an older host runner does not say it. {error} when the host runner cannot say it, never an empty queue. `afkconvoy queue` reads just the local queue, without this full diagnosis
    ///   - `resources`: resource episodes observed by the host runner, with stable ids, active conditions, since, episode, worker or host readings and evidence. Admission and pressure retain their identities during five minutes of recovery. A resource finding's watch_id is that episode identity, covered by a running or reported watch.
    ///   - `watch`: this host's watch (`afkconvoy run --help`, WATCH): {directory, stock, running, last, unavailable, association}. directory is where the next watch runs and stock whether it uses afkconvoy's instruction. running and last are each {id, directory, stock, items, started, harness, model, terminal, transcript, did, reported, report, attempts, ended, outcome}. items says why it started; transcript holds the session's exit status, not its printed output. reported is whether it called `watched`; report is its complete {held_back, did, verified, human, pending} record, null if it did not report; did is that report's actions. attempts holds each session whose harness gave out: {harness, model, terminal, why, did, screen}; screen is terminal evidence for continuation, never a report. outcome is ended (reported), not_reported (exited without reporting), closed, time_limit or no_harness. running and last are null with none. unavailable says why no harness could start or continue the last requested watch, null where one could. Earlier rounds are left out; `--rounds` adds `rounds`, each the same shape as `last`. `last` keeps the last round's report. `doctor --all` says each watch log line once, as `events` on this object — {at, event, cause, instance}, newest first — and leaves those lines out of every checkout's `fleet.events`.
    ///   - `asking`: this host's resident asking session (`afkconvoy run --help`, ASKING SESSION): {session, status, current, pending, unavailable, association}. session is {id, directory, harness, terminal, status}, null when no question has started one; status is absent, standby, asking, unavailable or uncertain. current is the question being delivered or discussed, with its condition episode, watch, human question and answer, and its delivered, withdrawn and put-aside flags; pending holds those waiting, including questions put aside. association shows the titled Terminal's launch and delivery proof. The session is durable across host runner restarts and upgrades.
    ///   - `installed`: the release installed where the host runner was started from, where the host runner moves onto it or did not: {path, version, from, since, wait_expired, held_back, failed} — the file; the release it says, null where it says none; the host runner's own release; when that file answered, in epoch seconds; whether the {const:MOVE_WAIT_SECONDS}s wait expired; what holds the move back, each in words ("/Users/me/Developments/hkj is in a tick", "1 exclusive command(s) running"), [] once nothing does, or the last blockers when the wait expired; and why the move failed — it did not start, or does not say its version — null until one did. null with none: nothing newer is installed there, or no host runner runs (`afkconvoy run --help`)
    ///   - `published`: a release newer than the host runner's, published to the Homebrew tap it was installed from, which it has not moved onto yet: {formula, version, installed, from, since, failed} — the formula with its tap (`sunfmin/tap/afkconvoy`); the newest release the tap publishes; the release Homebrew has installed where the host runner was started from; the host runner's own; when a look first found it, in epoch seconds; and why the last `brew upgrade` failed, null until one did. While it stands, a finding names the installed release and the published one. null with none: the host runner runs the newest, it was installed from no tap, or no host runner runs (`afkconvoy run --help`)
    ///   - `other_accounts`: the host runner of every other account on this machine, each the host runner of a host of its own: [{account, uid, pid, program, cores, memory_bytes, processes}] — the account and its uid; the host runner's pid and the file it was started from; and what every process of that account uses of the machine now: cores, their %CPU summed over 100; memory_bytes, resident; and how many processes. Read from the process table alone, which any account may read: nothing of the other account's files, socket or checkouts. [] where there is none; {error} where `ps` could not say. Where this account has no host runner of its own, each is also a finding beside `exclusive`'s `no_runner`
    /// - `fleet`: the `run` of this host for this repo:
    ///
    ///   - `alive`: whether one is running now: this host's runner, asked on its socket naming this checkout, holds a fleet instance of this repo
    ///   - `instance`, `host`: its fleet instance id: the one its log names, else the one the claims stamped with this host name; null when this host never ran one
    ///   - `heartbeat_age`: seconds since its heartbeat; null: it has none
    ///   - `heartbeats`: {found, live}: how many heartbeats the remote has, whoever stamped them, and the instance ids of those within the lease. A `run` told to stop removes its own, and a start removes the dead ones that hold nothing: more than one found and not live is fleets nobody started again
    ///   - `log`: the file its ticks are logged in, or "no log": no `run` of this version ran here
    ///   - `last_tick`: {count, ended, next_due, sleep, took, line, did, error}: when its last tick ended and the next is due, in epoch seconds, the seconds it slept after it and took from its start to its end (null where its log has no start of it), the line `run` printed, and what `tick` answers as `did`; error, when the tick failed whole. null with no log
    ///   - `failed_steps`: the last {const:DOCTOR_FAILED_SHOWN} failures, newest first: {tick, ended, do, issue, patrol, error, count}. One cause that stopped several steps of a tick is one failure: count is how many, and do, issue and patrol the first's
    ///   - `events`: the last {const:DOCTOR_EVENTS_SHOWN} events that reached a `run` of this host, newest first, whichever instance: {at, event, cause, instance} — event being wake (between ticks, or during one), start, stop (told to, by SIGTERM or SIGINT), broken, back, moved or not_moved (the host runner moved onto a newer afkconvoy, with both releases, or that one did not start: `afkconvoy run --help`), host_wake, watch (a thing a watch did, and how it ended: `afkconvoy run --help`) or cut (a cut started, and how it ended: `afkconvoy cut --help`); cause says what caused it, in words. [] with no log
    ///   - `broken`: {why, since, next, instance}: its fleet instance is broken (`afkconvoy run --help`) — why, as it last broke; since when, and when the host runner starts it again, in epoch seconds, null where no host runner runs on this host; and the fleet instance that broke, "" where none had started. null: it is not broken
    ///   - `concurrency`: {in_force, said_by}: how many claims its fleet instance holds at work at once on this host, as `plan` says it — said_by `checkout` or `default`. {error} where the config or the checkout list cannot be read
    ///   - `tracker`: the repo its fleet reads and writes its issues in, as `afkconvoy checkouts` says it: null where the checkout says none, and its own repo is its tracker. {error} where the checkout list cannot be read
    ///   - `github_api`: {host, login, core, graphql, secondary, for_rest_reads, for_graphql_reads}. Each quota has {remaining, reset}; null remaining means unknown and reset is Unix seconds. Selection respects primary exhaustion until reset and shared secondary cooldowns; fallback requires equivalent facts. {error} when the observation cannot be read. [`afkconvoy help troubleshooting`](help:troubleshooting) explains capability refusals and write receipts.
    /// - `claims`: each claim of that instance: {issue, title, status, pr, stage, role, held_for, worker, worktree, gate} — status as `plan` gives it; stage and role in a bound repo; held_for in seconds; worker is {state, quiet}, state being busy, stopped or gone, and null when orca could not say; a session that says busy while orca reports the agent done says disputed instead, with session: "busy", and raises a finding once orca has said so for the grace period. A claim left alone by a fleet lock instead says {state: "waiting_on_lock", locks: [issue numbers]} and raises no stopped or missing worker finding; the lock's own worker keeps its findings. gate is {green_at} when a green gate run is on record for the tree the worktree is at, else null
    /// - `issues`: as `plan` answers them: {frontier, excluded, escalating, locks, free_slots, patrols} — what a worker can be started on, what waits and why, and what is being handed to a human and why. null when they could not be read
    /// - `refusals`: issue number → the harnesses that refused that task, each with `harness` and `evidence`, read from GitHub. Later starts skip them across every stage; the harness stays healthy for other issues.
    /// - `candidates`: the remote candidate chains for every target branch of this repo, each in order and identified by `target_branch`. Each candidate is {phase, commit, issue, pr, predecessor, waiting, target_branch, id, cause, cancelled_by, round}. `round` is the id of the candidate whose slow gate it waits on: the tip of the round now slow-gating, for each candidate of that round and the tip itself; null for one behind it or with no round running, which waits for the next round. `waiting` is `constructing_worktree`, `validating`, `waiting_for_round` (fast-green, until a round's slow gate lands it), `slow_gating` (the round's tip, running the slow gate), `waiting_for_predecessor`, `confirming_process_exit`, `gate_red`, `cancelled` or `landed`. A finished candidate — phase `landed`, `red` or `cancelled` — carries no gate excerpt. A candidate gating now (phase `validating` or `slow`) may still carry `excerpt`. An error reading the chain is reported as `{error}`.
    /// - `codex_workers`: this checkout's Codex sessions, each with issue, worktree, harness, model, effort, model_source, effort_source and codex_home (its private session directory). Automatic starts read the model from your `~/.codex/config.toml` (`user_selection`) unless a model label is followed (`model_label`; a stored `casting` is read as that), and no user preference leaves the model null for Codex's native default (`codex_default`). Every start reads the effort from your own Codex config, or uses Codex's model default. Sources stay attached to the session after your config changes; a saved `/model` choice is shown with its session's private config as the source. [`afkconvoy help worker-launch`](help:worker-launch).
    /// - `waiting_on_human`: as `plan` answers it, and as the panel lists it under "With a human": every open issue nobody holds that waits for a human — escalated, at a stage that is a human's, past the end of its workflow and still open, or reporting a patrol's failed run — each as {issue, title, why, url, since, answer}: `why` is what kind of wait, then the reason the issue’s comment gives, in its first sentence; `url` is that comment (the report itself for an older failed patrol or cut with no reason comment), or null where no comment explains the wait; `answer` is the line that answers it, run as printed apart from what stands between < and >, naming the checkout as that command takes it so the line runs from any directory. An escalation with no position gets the team's only flow label filled in; with several workflows, `why` lists the flow labels to choose `<flow label>` from. null when the issues could not be read
    /// - `findings`: what is wrong, most severe first — none: []. Each is {kind, severity, what, evidence, fix, see, watch_id}:
    ///   - `kind`: what it is a finding of, in snake_case: the troubleshooting row it is headed by (`no_run`, `step_unrecovered`, `health_overdue`)
    ///
    ///   - `severity`: how much it matters, one of those below
    ///   - `what`: the fault, in a sentence
    ///   - `evidence`: what was read that says so
    ///   - `fix`: one command line, run as printed apart from what stands between < and > — it names the checkout as the command takes it, so the line runs from any directory, and `afkconvoy doctor --all` is run from anywhere — or, for a fault in a team's or a role's file, or a key the repo's config leaves out, `<file>: <key>` to edit. For an escalation with no position and several workflows, `what` lists the flow labels to choose `<flow label>` from
    ///   - `see`: the command that prints the help topic explaining it: `afkconvoy help humans` where what it reports waits for a human, else `afkconvoy help troubleshooting`, which has a row for each finding
    ///   - `watch_id`: the condition's identity, made where it is judged: the same while the condition lasts, whatever `what` says of it. `checkout:` is one of this checkout, `host:` one of the host; a resource's is its episode identity. The host's watch acts on each finding but an `info` one, or its own, by `kind` and `watch_id`, never by its words
    ///
    /// A finding's `severity` is one of, most first:
    ///
    /// {table:severity}
    ///
    /// It exits 0 with no critical finding, and non-zero with one — printing the same object, not {"error"} — so that a script can gate on it. Errors: "is not in a git checkout", or git's own words for a checkout it refuses. Whatever else cannot be read is itself a finding.
    ///
    /// With `--all`:
    ///
    /// {"host": {"host_kit", "exclusive", "watch", "notify", "daybook", "resources", "installed", "other_accounts"}, "findings", "checkouts": [{"checkout", "repo", "doctor"}], "until"}
    ///
    /// - `host`: what of `host` is the host's alone, as above, said once
    /// - `findings`: the host's findings, said once however many checkouts found them, most severe first: a command missing or logged out, orca not running, a harness paused, the host runner not moved onto a newer afkconvoy, a worker health observation overdue by the pace of every fleet instance on the host. A checkout's own findings add, under `--all`, its fleet instance not alive, and, with `--since`, each tick of the interval that failed with what no unrecovered step's finding says
    /// - `checkouts`: each checkout on the list, in its order — [] with none — with the repo its remote `origin` names, null where it is no git checkout any more, and its `doctor`: what `doctor` answers in it — `waiting_on_human` among it — but for the host's, which are above. Each is asked as the fleet of the host its `run` named, unless `--host` names one. {"error", "see"} where it could not be asked
    /// - `until`: this look's boundary in epoch seconds, captured before reading any checkout. Pass it as the next `--since`. Null when a checkout or its interval could not be read: keep the previous boundary and try again.
    ///
    /// With `--all --since <time>`, each checkout's `doctor.watching` adds what happened on this host and on GitHub. Time is epoch seconds or UTC `YYYY-MM-DDTHH:MM:SSZ`. The interval includes `since` and excludes `until`: events stamped in the current second are left for the next look, so adjacent looks lose no step and repeat none. Without `--since`, no interval is read and the state and findings are answered as usual.
    ///
    /// - `since`, `until`: the interval's boundaries, in epoch seconds.
    /// - `issues`: each issue that moved, and each issue with an open PR, as {issue, title, pr, steps, last_step}. `steps` are oldest first, each {at, source, step, detail}; source is tick_log or github. Exclusive queue counts, waits, exit codes and run times are in detail, as logged. `last_step` adds `ago`, seconds since the last step, including a step before `since`; null if neither source has one. A queued, running or red gate therefore explains a PR quiet on GitHub.
    /// - `ticks`: ticks that acted, held admission back or failed, oldest first, as {instance, first, last, count, line, failed, did, errors}. Consecutive successful ticks with the same line, steps and errors are folded, count saying how many; every failed tick is separate, including a failure said only in its line.
    /// - `events`: other tick log lines, oldest first, as {at, instance, line}: commands, starts, stops, hot workers and what watches did among them. A command that names no issue stays here.
    /// - `ended`: GitHub's issue_closed, pr_merged, pr_closed and intake steps in the interval, each {at, issue, pr, step, detail}. Fork PRs and another repo's issues in a shared tracker are left out.
    /// - `log`: {file, first, retained_only}: the local log, its oldest retained timestamp (null with none), and true because older lines may have been cut back. The interval can only report local steps still retained. GitHub reads include closed issues and PRs, and their intake and stage records. {error, see} in place of watching means that interval could not be read; the top-level until is null and the command exits non-zero.
    ///
    /// It exits non-zero where any finding is critical, the host's or a checkout's, or a checkout could not be asked — printing the same object. Errors: the checkout list cannot be read.
    ///
    /// The findings: a command the fleet needs is missing or logged out; orca cannot be asked, or does not know the checkout; the worker launch command of a harness the fleet could be asked for cannot start a worker — said with the harness and what calls for it; no `run` is alive and claims are held; the fleet instance of this checkout is broken — with why, since when and when the host runner starts it again; the same step failed in each of the last three ticks; the team has a fault at its head; a fleet lock stands on an issue that cannot be worked (all critical) — a step failed and has not recovered, including a worker that failed to start, with its error and the line to try again; a claim has no worker; a worker is silent with its claim still at work, as the tick judges it — its `fix` the command the tick runs next for it; the frontier is not empty, slots are free and the last tick started nobody; a flow label names no workflow of the team; an issue has been at a stage longer than the repo's p95 for completed performances of that stage, or, with fewer than 20, longer than its own earlier record gaps of at least a minute; an issue is one rework from a breaker; a patrol is failed or full; an issue has waited for a human for more than an hour, its `fix` being the line that answers it; the gate is slow — with the host, the fixed duration budget when declared, the termination gate limit and the {const:SLOW_GATE_LANDINGS} runs, each a commit and its seconds, as `afkconvoy plan` has them, and the issue open for it when there is one; the host runner did not move onto the newer release installed where it was started from — it did not start, does not say its version, or waited past the release move limit — with the file, both releases and why, its `fix` that file's `--version` (all warning) — the repo's config names `concurrency`, which nothing reads any more — with the value it names and the concurrency in force: nothing is held up, even where the values differ. Its `fix` is deleting the key alone where the checkout says its own or the config names the number already in force; otherwise `afkconvoy run <checkout> --concurrency <value>` keeps the config's number on this host before deleting the key; the role or the team a stage in flight started under has moved since; and the repo's config declares no gate limit, so the gate has the default's {const:GATE_SECONDS} seconds before anything ends it: `fix` is the file and the key, `gate_seconds`; and a worktree on this host is kept with no claim on its issue — said with what it holds that the remote does not (uncommitted changes, commits only it has, or nothing), its `fix` the line that removes it, pushing first whatever only it holds (all info). A checkout that names a harness no worker of this checkout starts on is said with the checkout, the harness and why — the role holds no limits for that harness, or the harness cannot start — and no other harness is started in its place.
    ///
    /// Moved: the role's directory holds something else at the head of its repo than at the commit the stage started under — a file added, removed or changed. A new commit is not it: in a repo of several roles a push gives every role a new commit, and moves the ones whose directory it changed. The team has moved when its directory has, without what a worker is not given of it: the directories of its roles, and the .claude/ at its top. So a push that changes only a role moves no team, one that changes only the team's rules — the CLAUDE.md at its top, which every worker is given — moves the team, and one that changes a workflow moves no role. A role in a repo of its own moves with every push that changes a file there. The two are compared in the copies this host keeps (copies/ under the cache directory); where the one the stage started under is no longer there, a different commit is all there is to go by, and it is said to have moved. `evidence` gives both commits.
    ///
    /// ## What trips you up
    ///
    /// - A warning may be the fleet mid-step: a claim has no worker between the tick that took it and the one that starts its worker. Run it again after a tick before you act on one; a critical finding does not go away by waiting.
    /// - It says nothing of ticks nobody logged. A `run` logs under your cache directory (`afkconvoy help cache` says where it is) on the host it runs on: run `doctor` there, as the same user. The log is a log: nothing reads it to decide anything, and deleting it loses only what `doctor` could say of the ticks before.
    /// - It makes no startup probe, so it looks for the fleet's refs where this repo's last `plan`, `tick` or `run` found the remote keeps them. In a clone none ever ran in, it looks under refs/convoy/.
    /// - `fix` is the first thing to try, not a promise: read `afkconvoy <command> --help` of the command it names before you run it.
    ///
    /// A human wait taken by another host has `taken_by` naming that host and an info finding. It starts no watch round here, is absent from this host's human report entries and sends nothing through `--notify-command` and asks no question. `taken_by` is absent when the wait is available here or held here. Reading doctor changes no claim; the host runner takes available waits before queueing its look.
    ///
    /// ## Worker proxy bypass
    ///
    /// `--worker-no-proxy [HARNESS=]HOSTS` shows what a start with those flags would give each harness, under `worker_no_proxy`: [`afkconvoy help worker-proxy`](help:worker-proxy).
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Before `run`, to see what the fleet would start on; and beside a running fleet, to see where every claim stands and why an issue is not on the frontier.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It makes the startup probe — which checks, as a start does, the worker launch command a `run` started now would give every harness the fleet could be asked for (`afkconvoy run --help`) — then the rebuild: the open issues and PRs on GitHub, the claims and heartbeats on the remote, and their landing turns. It changes nothing on GitHub. On the remote it pushes its startup probe and deletes it again, together with what the probe sweeps: a probe an earlier start left behind, and recorded gate runs too old for a landing to believe.
    ///
    /// WHEN IT PROBES. Not every time: a startup probe that passed on this host in the last hour stands, and `plan` then makes none — it says so on stderr, repeats that probe's warnings, and pushes nothing. It probes when no probe stands: none passed on this host within the hour — `run` makes one at every start, `tick` and `plan` when they find none — or the config's `base_branch` or `team` changed since, or the remote's URL did, or the note of where the remote keeps the fleet's refs — or no check that found the tracker the checkout says now private stands: one stands for the hour, of that tracker alone. The probe is noted in the repo's git dir (`convoy-probe`), and the check of the tracker beside it (`convoy-tracker`); delete either file to have the next `plan` probe.
    ///
    /// The rebuild asks GitHub twice, side by side with one listing of the remote, however many issues are open: one query for the open issues with their comments (comments beyond the first hundred or blockers beyond the first ten are read in full through their paginated endpoints), one for the open PRs. In a tracker another repo than the checkout's, GitHub answers only the open issues that carry this repo's repo label, so it asks as often however many repos' issues the tracker holds. It asks once more — for the issues that carry the label `slow-gate` — only while the recorded gate runs of the base branch's landings say the gate is slow.
    ///
    /// {const:REMOTE_READS}
    ///
    /// ## What it answers
    ///
    /// {"frontier", "excluded", "waiting_on_human", "mine", "peer_live", "stale", "stale_closed", "free_slots", "now", "locks", "concurrency", "harness", "tracker", "admission", "host_kit", "worker_no_proxy", "need_installs"} — "held_back" while admission holds; and in a bound repo "intake", "left_out", "unmatched" and "nothing_safe" where they name some, "patrols" where it names some; and "slow_gate" while the gate is slow; and "cut" in a repo that turns cutting on, or while a cut is going or on the base branch
    ///
    /// `targets` lists open tracker tasks with issue, tracker_issue, delivery (null when unattached), target_branch, configured_base and pinned. Invalid destinations report error per task without falling back to the configured base.
    ///
    /// `deliveries` lists each Jira delivery with its final PR or pending reason, whether recorded delivery evidence certifies the current delivery head, final-target head and what each attached task came to, and any settlement. Reading writes nothing.
    ///
    /// - `host_kit`: {path, refreshed, harnesses}: this host's kit file; when it was last written, in epoch seconds — by `kit`, by `run`, or by a tick's daily refresh (`afkconvoy kit --help`); each harness in order of preference as {harness, command, models, default_model, research_pending}, each model {model, names, good_at, settled, sources, researched_at}. `default_model` is true when the harness has no placed model, so a worker runs its login's default with no model flag; `research_pending` is true while it holds a model awaiting research. good_at is what research says a model is good at, or a research marker; settled is a reason research found it unsuitable. The research states are defined in `afkconvoy kit --help`. sources holds dated URLs and researched_at the research time, absent where none succeeded. With an unreadable kit, {path, error}. `afkconvoy kit` writes it again.
    /// - `frontier`: issues due to dispatch, with their stage and cast {harness, model, why}. A checkout that names none takes the host kit's first eligible harness within role eligibility. One that names a harness starts on that harness, and the host kit orders that harness's models. Model labels only match within the harness in force. `selection` lists kit order, eligible candidates, skipped candidates and health evidence.
    /// - `need_installs`: each install this host last ran for a gate or cut need the config declares, as {kind, check, at, seconds, status, passed, stderr}: `gate-needs` or `cut-needs`, the check's name, when it ran in epoch seconds, how long it took, the install's status, whether the check then passed, and the user-only file of its last 20 stderr lines — never its output (`afkconvoy config --help`). Empty where none ran.
    /// - `codex_workers`: started Codex sessions with their actual model, effort and sources, as `doctor` answers them. A frontier issue whose harness is Codex also has `codex`: its next-launch model and source from a followed model label (`model_label`), the current user preference (`user_selection`), or Codex's native default (`codex_default`), plus the effort read now from your own `~/.codex/config.toml` (`codex_default` with no setting). Existing sessions keep their launch evidence or their own saved `/model` choice. [`afkconvoy help worker-launch`](help:worker-launch).
    /// - `excluded`: every other open issue, with why: {number, title, reason}. An escalated issue says so — "escalated: waits for a ruling" — and not that it lacks the label its escalation took off. A name that matches nothing never excludes an issue: it runs the user default, and the label is said once. A bound repo's issue at a stage whose role has no harness on this host it holds limits for waits, "<role> has no harness on this host it holds limits for: ", naming what the role holds limits for and what the host kit holds, and is dispatched once the host kit holds one. No issue is held for a caster.
    /// - `left_out`: in a bound repo whose team and host kit can be read: each role, in the team's order, with what its range on this host leaves out: {role, left_out: [{harness, why}]} — a harness its roster entry and the host kit allow it and it holds no limits for, while it holds limits for another, why `no limits for pi` (`afkconvoy help roster`)
    /// - `unmatched`: in a bound repo, where some role's roster entry matches nothing in the host kit: {role, why} — the role ranges over the host kit whole, less what it holds no limits for
    /// - `nothing_safe`: in a bound repo, where some role has no harness on this host it holds limits for: {role, why} — its stages wait, and every other stage goes on
    /// - `unmatched_models`: each roster model matching nothing held, {role, harness, model, holds, said}, with the nearest names or the two matches
    /// - `casting`: each open issue, {number, needed, why, stages, running}. why is "its model label" where every stage followed one, "the user's default" where none did, or both where the stages differ. needed is always false. stages lists {stage, harness, model, followed, why, skipped}; followed says whether the one `model:` label was followed, model is that id or null for the user default, and skipped is empty. running is {harness, model} recorded at the existing worker's launch — model null for the user default — or null. Doctor answers the same under issues.casting, and the panel shows it under Model selection. There is no caster.
    /// - `waiting_on_human`: every open issue nobody holds that waits for a human, lowest number first: {issue, title, why, url, since, answer} — `why`: what kind of wait, then the reason the issue’s comment gives, in its first sentence; `url`: that comment (the report itself for an older failed patrol or cut with no reason comment), or null where no comment explains the wait; since when in epoch seconds where GitHub said, and the line that answers it, run as printed apart from what stands between < and >, naming the checkout as that command takes it so the line runs from any directory. An escalation with no position gets the team's only flow label filled in; with several workflows, `why` lists the flow labels to choose `<flow label>` from. They are: an escalated issue, one whose records call for a human, one at a stage that is a human's, one whose workflow ended and that is still open, and the report of a patrol's failed run
    /// - `escalating`: in a bound repo, when there are any: the unclaimed issues a tick hands to a human, with why: {number, title, reason}
    /// - `mine`: the claims of `--instance`: {number, title, status, pr, declared, idle} — `declared` is true while a verdict declared under the claim waits to be acted on: the claim takes no slot meanwhile. `idle` is true of a landing claim whose worker is not changing the tree — gone or idle, or its turn queued, waiting on a predecessor or last answering a wait: it takes no slot either, and is held until its PR lands. In a bound repo with `due`, the stage a worker is to be started on once the claim's stage has declared, where a role performs one. status is one of those below
    /// - `peer_live`: other instances' claims within the lease — never touched
    /// - `stale`: other instances' claims past the lease, on open issues — a tick takes them
    /// - `stale_closed`: the same on closed issues — a tick deletes them (each of the three: {number, instance, sha})
    /// - `free_slots`: how many more claims the instance may take
    /// - `concurrency`: {in_force, said_by}: how many claims the fleet of this checkout holds at work at once on this host — what `free_slots` counts from, beside the claims a fleet lock leaves alone and those whose worker declared — and who says it: `checkout`, where `afkconvoy run --concurrency` said it of this checkout, which keeps it on the checkout list; `default`, where it said none ({const:CONCURRENCY}). The repo config says none: a `concurrency` it names is read no more
    /// - `harness`: {in_force, said_by}: the harness workers of this checkout start on, and who says it. `checkout`, where `afkconvoy run --harness` named one, which keeps it on the checkout list, and `in_force` is that harness even when it cannot start. `host_kit`, where it named none, and `in_force` is the host kit's first eligible harness, or null where none can start. A role that holds no limits for the harness in force is excluded with the checkout, the harness and that it holds no limits. Where it cannot start, `plan` and `doctor` say `account fault`, or `model fault` once no placed model is left, or that its launch cannot start, and no other harness is started in its place
    /// - `tracker`: the repo the fleet of this checkout reads and writes its issues in, `<owner>/<repo>`, where `afkconvoy run --tracker` said one of it, which keeps it on the checkout list; null where it said none, and the checkout's own repo is its tracker. With one, the frontier, `excluded` and every other issue here are the open issues there that carry this repo's repo label, `repo:<owner>/<repo>`; one that carries the repo label of another repo too is excluded, "repo labels of more than one repo"
    /// - `now`: the time the plan was made at, in epoch seconds
    /// - `locks`: the fleet locks standing: the open issues that carry the lock label, lowest number first — none: []. While there is one, the frontier holds lock issues only, and every issue a tick would otherwise dispatch or hand to a human is under `excluded` with the reason "a fleet lock stands (#n)"
    /// - `admission`: whether a tick would start a new worker on this host now (`afkconvoy run --help`, ADMISSION): {admits, held_back_by, shares, thresholds, reading}. held_back_by is each threshold the host is past, as a tick's line says it: none, and it admits. shares are the percentages in force — load_percent_of_cores, memory_free_percent, disk_free_percent — and said_by_host the host settings that said one, by key (admit_load, admit_memory, admit_disk, admit_inspect_after); thresholds what they come to on this host — load, memory_free_percent, disk_free_bytes; reading what the host reads now — cores, load, memory_free_percent, disk_free_bytes, disk_size_bytes, each null where it could not be read
    /// - `worker_no_proxy`: every harness, with {harness, addition, no_proxy, NO_PROXY}: the host list added and the resulting login shell values, with this command's --worker-no-proxy flags. A null addition leaves the inherited spellings alone
    /// - `held_back`: while admission holds: each threshold the host is past, as data: {by: "load", load, limit} in hundredths, {by: "memory", free, floor} in percent, {by: "disk", free, floor} in bytes. The frontier is still what a tick would dispatch once it admits
    /// - `intake`: in a bound repo: {"open", "limit"} — the open issues taken in by workers, counted from their intake records, against the repo's `intake_limit`. With `open` at `limit`, `afkconvoy intake` refuses. An issue a human labelled is not among them
    /// - `findings`: open blockers no fleet works, each {severity, what, evidence, fix, see}: the waiting issue, the blocker's repo and number, why no checkout serving the tracker works it, and the transfer and label lines. A consult or declared hand-off proceeds if these are its only blockers; build work waits
    /// - `patrols`: in a bound repo that names patrols: each, in the order the config names them: {name, standing, every, last_run, open, max_open} — `every` in seconds, `last_run` when its last recorded run started (null: it never ran), `open` the open issues it opened. A patrol whose file says `max_runs_a_day` has it too, and `runs_in_day`: how many of its runs started in the last 24 hours. A patrol with `max_landed_a_day` reports it and `landed_in_day`: the issues it opened that closed as completed in the last rolling 24 hours. One a human asked to run now (`afkconvoy patrol-now --help`) has `asked`: when the ask was made, until a tick starts the run. standing says what a tick does about it:
    ///
    ///   - `due`: its interval has passed, or a human asked for it: a tick runs it
    ///   - `not_due`: its interval has not passed since `last_run`
    ///   - `spent`: `runs_in_day` has reached `max_runs_a_day`: it is held until `until` — when the oldest of those runs is 24 hours old
    ///   - `landed`: `landed_in_day` reached `max_landed_a_day`: held until `until`, when enough completions are 24 hours old to fall below the bound. A human's ask respects this bound too
    ///   - `locked`: a fleet lock stands: no patrol runs
    ///   - `full`: `open` has reached `max_open`
    ///   - `failed`: its last run failed, and the issue that reports it — `failure`, in this case only — is still open
    ///   - `running`: a run of `--instance` is still going
    ///   - `peer_running`: a live peer's run is still going
    ///   - `stale_run`: the run is claimed by a peer presumed dead: a tick deletes that claim
    ///
    /// - `cut`: in a repo that turns cutting on (`afkconvoy cut --help`): {standing, newest_tag, initial, past, oldest_landing_age_seconds, busy_in_seconds, failure} — `newest_tag` the newest version tag on the remote (none: it has none), `initial` the declared first version, present only without a version tag, `past` how many commits the base branch holds past its newest tag (0 without one), `oldest_landing_age_seconds` how long its oldest landing has waited, `busy_in_seconds` how long until a busy cut may start (zero at the bound; both ages null without a landing past a tag), `failure` the open issue that says the last cut failed. standing says what a tick does about it:
    ///
    ///   - `due`: the base branch holds commits past `newest_tag`: the next idle tick starts a cut, or the next busy tick once `busy_in_seconds` is zero, whatever claims or frontier remain. An untagged remote with `initial` declared starts on an idle tick only
    ///   - `tagged`: the base branch holds nothing past `newest_tag`: nothing to cut
    ///   - `untagged`: the remote has no version tag and the repo declares no `initial` version: nothing is cut
    ///   - `failed`: the last cut failed, and `failure` is still open: no cut starts until it is closed
    ///   - `locked`: a fleet lock stands: no cut starts
    ///   - `running`: a cut `--instance` started is still going
    ///   - `peer_running`: a live peer's cut is still going
    ///   - `stale_run`: it is claimed by a peer presumed dead: a tick deletes that claim
    ///   - `off`: the repo turns cutting off, and a cut started before it did is on the base branch
    /// - `slow_gate`: while the gate is slow: {gate_seconds, gate_budget_seconds?, host, runs, issue}. On `host`, the last {const:SLOW_GATE_LANDINGS} landings on the base branch that have a recorded run of the config's `gate_command` with a time all exceeded the fixed `gate_budget_seconds` when declared, or took more than {const:SLOW_GATE_PERCENT}% of the termination gate limit, `gate_seconds`, otherwise. runs are those {const:SLOW_GATE_LANDINGS}, newest first: {commit, seconds}. issue is the open issue that says so — the lowest open one carrying the label `slow-gate` — or null: a tick opens one (`afkconvoy tick --help`)
    ///
    /// A claim's `status`, under `mine`, is one of:
    ///
    /// {table:claim_status}
    ///
    /// The probe's warnings go to stderr. Errors: a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run; `claude` is among them when no custom provider is set, as `run` would then start its workers with it); the repo fails the startup probe (the remote has no base branch, its protection refuses the fleet's push, its tracker is public — the error names it, with the line that says a private one — or the remote takes none of the fleet's refs); the config cannot be read; GitHub or the remote cannot be read.
    ///
    /// ## What trips you up
    ///
    /// - Without `--instance` nobody's claims are its own: `mine` is empty, a running fleet's claims show under `peer_live`, and `free_slots` is the whole concurrency. To see a running fleet as it sees itself, pass its instance id — `takeover --list` shows every id the remote remembers.
    /// - A lock that stands is not thereby worked. A lock issue is on the frontier only when it would be without the lock: with the ready label in an unbound repo, a flow label in a bound one, no epic label, no open blocker, no claim. `excluded` says which of these keeps it off — and the lock stands until the issue is closed.
    /// - `frontier` is what the rebuild read, and `plan` reads nothing more. An issue that gains an open blocker after it is still listed; a tick reads each issue's blockers once more just before it claims it, and skips the issue then.
    /// - A patrol is due or it is not: one that missed ten intervals runs once. And a run still going takes one slot of the concurrency, like a claim on an issue.
    /// - With a lock standing, `free_slots` counts as free the slot of every claim of `--instance` the lock leaves alone: a lock issue does not wait for them.
    /// - `free_slots` counts as free the slot of every claim of `--instance` that is `declared`: `mine` may hold more claims than the concurrency, and the frontier does not wait out the grace period of a worker that already declared.
    /// - Fleet state is unchanged. When it probes it still pushes to the remote, and deletes there: it needs push access, and it is not a read-only command. To read without writing anything, run `afkconvoy doctor`.
    /// - A gate gone slow is read off the recorded gate runs the remote still has: a record is swept a day after its run, a landing with no record or with a record that states no time is passed over, and {const:SLOW_GATE_LANDINGS} landings spread over hosts none of which has {const:SLOW_GATE_LANDINGS} say nothing. After an issue for a slow gate closed, only the landings made since count.
    /// - A probe that stands is trusted, not checked: a command uninstalled or logged out since, or a rule the remote gained, shows as the error of whatever runs into it — or, for the ref layout, not until a probe runs. A noted ref layout moves when two probes that run, one after the other, each see it rejected and the other taken; one rejection leaves the note, and a probe that takes the noted layout clears that streak. A checkout that has noted no layout still settles on hidden refs, or on branches where the remote rejects those. `run` probes at every start.
    ///
    /// ## Worker proxy bypass
    ///
    /// `--worker-no-proxy [HARNESS=]HOSTS` shows what a start with those flags would give each harness, under `worker_no_proxy`: [`afkconvoy help worker-proxy`](help:worker-proxy).
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// To have a repo worked: after `config` answers and `plan` shows the frontier you expect. Run it again after the host runner was stopped or killed, or the host restarted: it starts every checkout on the list, each where its fleet instance left off. An upgrade of afkconvoy (`brew upgrade afkconvoy`) needs none: the host runner moves onto the new version itself between ticks, every checkout on the list with it (below).
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo or naming one. It returns once that checkout's fleet instance has ended its first tick; the host runner ticks on, in the background, until it is stopped.
    ///
    /// ## What it reads and writes
    ///
    /// The checkout list: checkouts.json in the cache directory, kept on this host and never pushed. A checkout goes on it once its fleet instance's first tick ended, with the `--instance`, `--host` and `--worker-no-proxy` its `run` named; one refused before its first tick is not put there, and one there already stays there, broken (below). A `run` of a checkout on the list whose fleet instance does not run starts it with those, but for each that `run` names itself.
    ///
    /// The checkout's concurrency, with its entry: how many claims its fleet instance holds at work at once on this host. `--concurrency 6` says it — a whole number, at least 1 — and `--concurrency default` changes it back to the default, {const:CONCURRENCY}; a `run` that names none leaves it as it is. Until the checkout says one, its fleet instance holds {const:CONCURRENCY}. It is said here and nowhere else: the repo config's `concurrency` is read no more, and `afkconvoy doctor` reports it as `info`, with the key to delete, first giving the `run` that keeps the config's number only where the checkout says none and the numbers differ. Unlike the `--instance`, `--host` and `--worker-no-proxy` it named, which hold from the start of its fleet instance, it is read at each tick, as the host settings are: a change holds at the next tick, and the fleet instance is never started again for it. Lowered below the claims held, it ends no worker: those claims are worked to the end — the tick's line counts those past it, as `holding 2/2 · 1 past the bound` — and no new one is taken until fewer are held. A checkout the list does not hold yet goes by it from its first tick, which puts it on the list with it. A host runner started again keeps it; `plan`, `doctor` and `checkouts` say the one in force and who says it.
    ///
    /// The checkout's tracker, with its entry, read at each tick as its concurrency is: the repo its fleet reads and writes its issues in — the issues, their labels, comments, blockers, escalations and closing — while PRs, branches, the gate and their landing turns stay the checkout's own repo's. `--tracker me/issues` says it, and `--tracker default`, or the checkout's own repo, says none; a `run` that names none leaves it as it is. Until the checkout says one, its tracker is its own repo. A tracker another repo than the checkout's holds the issues of many repos: the fleet works the open issues there that carry its repo's repo label, `repo:<owner>/<repo>`, and the ready label — in a bound repo, a flow label; every issue it or a worker opens there carries the repo label, and `intake` and `lock` give it to one that lacks it; and a PR or a landing commit names its issue `me/issues#7`, never `#7`, which is the checkout's own repo's. `run` makes the repo label, and the ready, escalate and lock labels of the repo config, in the tracker where it lacks them. It refuses to change the tracker while the checkout's fleet holds a claim on an issue of the tracker it had, or a legacy line still carries a PR of one, naming them, and changes nothing: run it again once they are done. The issues of the tracker it had stay there. A tracker GitHub says is public is refused, and nothing is made in it or changed: whoever can comment on an issue there writes what the fleet reads as its state and what a worker reads as its task. So is the checkout's own repo, where it is public and the checkout says no tracker, or `default`: every start of its fleet is refused until it says a private one, `--tracker <owner>/<private repo>`. A `run` that starts the host runner — where none runs, or where it moves one of an older release (below) — says the tracker of a checkout on the list with its entry before it starts it, as that host runner starts every checkout on the list, as its entry has it, before it is handed this one: it asks GitHub and makes the labels before an older host runner is stopped, which changes nothing where it refuses, and keeps the tracker once that host runner has let the host go. So its checkout's fleet instance starts under the tracker this `run` names, never under the one its entry had, and a checkout of a public repo is taken at the first `run --tracker <owner>/<private repo>`, whichever host runner was there. `plan`, `doctor`, `checkouts` and the panel say the tracker.
    ///
    /// The host settings, in the same file: what this host says for every fleet instance on it — the limits past which a worker is hot (`--hot-cores`, `--hot-memory`, `--hot-written`), the shares admission holds back at (`--admit-load`, `--admit-memory`, `--admit-disk`, `--admit-inspect-after`), the panel's port (`--panel`), the operator's directory a watch starts in and how often the host runner looks (`--watch-dir`, `--watch-every`; WATCH, below), and the command that prints the TypeSafe key Jev's worker-health judgments are asked with (`--jev-key-command`), and the shell command that sends human decisions (`--notify-command`; none by default), and the agent setup given to every harness the host kit holds: its instructions file, its skill list and its guard list (`--agent-instructions`, `--agent-skills`, `--agent-guards`; none by default; `afkconvoy kit --help`, The agent setup). A `run` that names one — by its flag, or by the variable of its environment that stands for it, the flag winning: AFKCONVOY_HOT_CORES, AFKCONVOY_HOT_MEMORY, AFKCONVOY_HOT_WRITTEN, AFKCONVOY_ADMIT_LOAD, AFKCONVOY_ADMIT_MEMORY, AFKCONVOY_ADMIT_DISK, AFKCONVOY_ADMIT_INSPECT_AFTER, AFKCONVOY_WATCH_DIR, AFKCONVOY_WATCH_EVERY, AFKCONVOY_JEV_KEY_COMMAND, AFKCONVOY_NOTIFY_COMMAND, AFKCONVOY_AGENT_INSTRUCTIONS, AFKCONVOY_AGENT_SKILLS, AFKCONVOY_AGENT_GUARDS — changes it for the host before it hands its checkout over. Every fleet instance on the host reads them as it goes — admission at each tick, the hot limits at each sample, the watch's before each look, the key command at each worker-health judgment — so a change holds for all of them at once, and none is started again for it; the host runner reads the panel's port as it starts. A `run` that names none changes none, and nothing else changes them: `tick`, `plan` and `doctor` read them as they are, and a host runner started again keeps them. `default` changes one back to its default (`--hot-cores default`). `afkconvoy doctor` says those in force, as `host.settings`.
    ///
    /// The host runner: one process of the host, holding a fleet instance for each checkout on the list, each ticking as a fleet instance always has. `run` asks it on its socket in the cache directory (host-runner.sock) — the one socket of the host: `stop`, `wake`, `panel --open`, `exclusive` and `doctor` ask it there too, each naming its checkout, and no fleet instance or repo has one of its own; one nobody listens on, which a host runner killed with no goodbye left, is removed by whoever finds it — and starts it where none answers: in a process group of its own, so the terminal that ran `run` can close, in /, with a fresh login shell's environment — the names your login shell (`$SHELL -lic`) exports, and none of those of the shell that ran `run`. A variable exported only in that shell, or one a session set for itself (Claude Code's, orca's), reaches neither the host runner nor any worker it starts: put it in your login shell's rc files. It keeps the cache directory and the config directory of the `run` that started it. Started again, it starts every checkout on the list, each with what its `run` named, under the host settings the file keeps; a checkout whose fleet instance cannot start then is broken (below), and stays on the list.
    ///
    /// Before anything else, `run` asks the host runner for its version. One of an older release than this `run`'s — after `brew upgrade afkconvoy` — is stopped as SIGTERM stops it: each fleet instance finishes the step it is in, and the checkout list is kept. Once it has let the host go, `run` starts a host runner of its own version, which starts every checkout on the same list, and then adds its own — saying its checkout's tracker with its entry there first, where it names one (above). What the old one held for every fleet instance goes with it: a watch goes on in its terminal with its original time limit, and an `exclusive` command waiting in the old one's queue waits in the next one's, from when it first asked (`afkconvoy exclusive --help`). One of a newer release refuses this `run`, which changes nothing. A build past a release (`afkconvoy --version` says `0.6.0+11.g9f58035`) is of that release, and one of the same release as the host runner's is handed the checkout as ever.
    ///
    /// The host runner moves onto a newer release by itself, with nothing run by hand. It watches the file it was started from — the path the `run` that started it was run as, such as the link Homebrew keeps, /opt/homebrew/bin/afkconvoy — and within {const:INSTALLED_LOOKED_SECONDS}s of a change there, as `brew upgrade afkconvoy` makes, asks it `--version`. Once a newer release answers, it pauses new ticks and new exclusive commands, lets active ticks and the running exclusive command finish, and moves between ticks. A command waiting in the exclusive queue holds nothing back: it rejoins the new runner with when it first asked. Claims stamped with this host, workers in their terminals, PRs waiting to land, a cut and a watch all carry over; no worker is stopped. It waits at most {const:MOVE_WAIT_SECONDS}s for that moment. If it still cannot move, it lets ticks and the queue run again on the old release, and `afkconvoy doctor` gives a finding with what held it, as `host.installed.held_back`; it keeps trying the move automatically, while the finding stays visible. `host.installed.since` is when that file answered, in epoch seconds; `wait_expired` says that this wait expired. Then each fleet instance stops between two ticks, keeping its `stop` line, and the host runner starts the newer afkconvoy's in its place, with the same checkout list and host settings, and lets the host go to it. That one starts every checkout on the list, each where its fleet instance left off; `checkouts` and the next `run` answer its pid and version. It never moves onto an older release, nor onto a build of its own release. One that does not start, or does not answer within {const:MOVED_WITHIN_SECONDS}s, leaves the old host runner holding the host and starting every checkout on the list again; `afkconvoy doctor` then gives a finding naming it, as it does for one there that does not say its version, and the host runner tries that file again only once it changes. The move, with both releases and what carried over, is a line of the host runner's log and of each checkout's tick log — `moved`, or `not_moved` with why it did not start — which `doctor` lists among its events.
    ///
    /// A host learns of a release it did not cut by itself, too. Only the host that cuts a release installs what it publishes (`afkconvoy cut --help`); every other host's Homebrew would learn of it only at a `brew update` nobody ran. So a host runner started from a Homebrew install from a tap looks at that tap's formula on GitHub as it starts and every {const:PUBLISHED_LOOKED_MINUTES} minutes after. Where it publishes a release newer than the host runner's, the look after the one that found it runs `brew update` and `brew upgrade <tap>/afkconvoy` — one look later, so that it never runs beside the cut's own install on the host that cuts — and the move above follows. A release published while a host runner runs is installed within twice {const:PUBLISHED_LOOKED_MINUTES} minutes of its publishing, and the host runner moves onto it within {const:MOVE_WAIT_SECONDS}s more, or `doctor` says what held it. Until it has moved, `afkconvoy doctor` gives a finding naming the release Homebrew has installed and the one published (`host.published`). What `brew` said is in brew-upgrade.log in the cache directory; an upgrade that failed is tried again at the next look. A host runner started from no Homebrew install, such as a build, looks at no tap.
    ///
    /// The host runner's log: host-runner.log in the cache directory, moved aside to host-runner.log.1 once past a MiB. One line, stamped in UTC with its zone named (`2026-10-30T13:56:16Z host runner started: …`), as it starts and as it stops, as it replaces a host runner of an older release (`host runner upgraded: from afkconvoy 0.5.13 (pid 4100) to afkconvoy 0.6.0 (pid 4242)`), as it finds another release installed where it was started from, as nothing holds the move back any more, as it moves onto it (`host runner moved: from afkconvoy 0.6.0 (pid 4242) to afkconvoy 0.6.1 (pid 4300), installed at /opt/homebrew/bin/afkconvoy`) or that one does not start (`host runner not moved: …`, with what it said as it ended), as each fleet instance starts and ends, as one breaks and is back, as a checkout is added, refused or removed, as anything in it panics (`panicked at <file>:<line>: <what it said>`), where it serves the panel, as it starts (`panel: http://127.0.0.1:7070/`), and each line a fleet instance says outside its tick log, after its checkout: its warnings and the `start:` lines after its first:
    ///
    /// ```
    /// fleet instance broken: /Users/me/Developments/hkj: it panicked: <what it said> · since 2026-10-24 09:12:00 UTC · started again at 2026-10-24 09:13:00 UTC, in 60s
    /// fleet instance back: /Users/me/Developments/hkj: me-h1-5e6f7a8b, broken since 2026-10-24 09:12:00 UTC, for 75s
    /// ```
    ///
    /// Two kinds of checkout are refused, with an error: one whose fleet instance is refused before its first tick — it fails the startup probe, say — which is not put on the list, and stays on it, broken (below), where it was on it already, whichever start was refused — the host runner's, as it starts or moves, or this `run`'s; and a second checkout of a repo already on the list, a clone or a worktree of it: a host runs one fleet instance per repo, and the error names the checkout to run it from.
    ///
    /// A fleet instance is broken when its loop panics or ends in an error, or when a start of it ends before its first tick while its checkout is on the list — the host runner's, from the list or after it broke, or a `run`'s: its startup probe no longer passes, say. It harms no other: the host runner and every other fleet instance go on, and its checkout stays on the list. The host runner starts it again after a backoff: {const:BACKOFF_FIRST_MINUTES} minute after it first broke, twice the wait before at each break in a row — one within {const:BACKOFF_MOST_MINUTES} minutes of the start before it — and {const:BACKOFF_MOST_MINUTES} minutes at the most. A `run` of its checkout starts it at once, with what that `run` names laid over what the list kept; a start of it that ends before its first tick is then refused to that `run`, which is told so, and it stays on the list, broken. Only `stop` takes a checkout off the list. It is back once a start of it again has ended its first tick. Broken — with why, since when, and when it is started again — and back are lines of the host runner's log and of its tick log, and `afkconvoy doctor` in its checkout says it is broken, why, and when it is started again. A tick that fails is no break: it is a line of its own, and the loop goes on.
    ///
    /// A repo can declare gate needs (`afkconvoy config --help`): the startup probe runs each, and a running fleet instance checks again within an hour. A failure breaks the instance. The refusal and `doctor` name the check, its status, the private host-cache file holding its last 20 stderr lines, and the line to fix it. Check output never appears in the tick log, `plan`, `doctor` or an issue; stdout is discarded.
    ///
    /// Then everything a fleet instance does. It makes the startup probe — at every start, whatever probe passed before — takes every claim stamped with this host — on an issue, on a patrol's run or on a cut — removes the heartbeats that are dead and hold nothing (those of the instances it took from, this host's past the lease, anyone's older than a day) — then ticks: each tick rebuilds the working set from GitHub and the remote and makes every transition it calls for — claims and heartbeats on the remote, workers in orca worktrees, comments, labels and blocked-by edges on issues, status boards, the landing turn's worker. Between ticks it sleeps; a worker's `wake` ends the sleep, and a tick that left the next one a worker to start is followed with no sleep. Before each tick it asks GitHub again whether its tracker is public, once the check that found it private is an hour old — or at once, of a tracker said anew. Public, it ends before the tick, as its startup probe would refuse it, and is broken: it holds no move onto a newer release back, and each start of it again is refused until the tracker is private again or another is said (`--tracker`), which `afkconvoy doctor` names with that line.
    ///
    /// A fleet never puts its host to sleep: an idle host stays awake until something else puts it to sleep. Idle is holding no claim — on an issue, a patrol's run, a cut — and an empty frontier; an escalated issue and a patrol not yet due leave a fleet idle. A cut starts when idle or landed work reaches its age bound, and keeps it from idle while it runs. Every fleet on this host is a fleet instance of its one host runner, which holds how each stands, as each says it as every tick of its begins and ends: in a tick, or between two, at work or quiet. No fleet instance asks another.
    ///
    /// ADMISSION. A tick starts no new worker while its host is busy: its 1-minute load average, less the estimated share of running exclusive queue process trees, past 200% of its cores. The estimate apportions total load by the trees’ share of observed CPU; if the queue or process table cannot be read, it subtracts nothing. `doctor` says total, queue and other load. Its estimate uses current CPU and may differ from the minute average; memory and disk include everything. It also holds if, less than 10% of its memory free (what the kernel counts as available: `sysctl kern.memorystatus_level`, or MemAvailable in /proc/meminfo), or less than 1% of the disk under the checkout free (`df`). The issue stays on the frontier for the next tick, and so do a stale claim and a due patrol. Workers already running are left alone, and so is the worker a declared verdict starts. The host says its own shares, each a whole percent, in its host settings: `--admit-load` (of its cores), `--admit-memory` and `--admit-disk` (free) — `--admit-load 400` lets the load reach four times the cores, and `--admit-disk 0` never holds for disk. A value that is no whole percent is refused, and `default` changes a share back. What cannot be read holds nothing back. `plan` and `doctor` say the thresholds in force and what the host reads.
    ///
    /// When admission holds back work in a checkout for one reason — load, memory or disk — at every tick for ten minutes (`--admit-inspect-after` sets the host's span), doctor names a finding that starts the host's watch at its next look. The finding names the held work. Reasons that change start none. Memory pressure (macOS's level at least 2) held for the same span is its own finding, even while admission admits, and never makes a worker hot. Each episode ends after five minutes of recovery. The watch receives the condition, how long it held, admission's reading and shares, and host evidence: for disk, each fleet's worktrees and caches and the largest directories outside them; for load or memory, the fleet's worker process trees and, where `hostvitals` is on the host runner's `PATH` and `hostvitals now` answers, its whole-host findings first, then its rankings of processes and Apps and its remaining readings. Otherwise it receives the fleet's hottest processes outside those trees, with one line saying why hostvitals could not answer. The evidence names its source. hostvitals is optional and is never installed as a dependency; hot-worker sampling stays the fleet's own. It checks admission or pressure itself after acting. Resource readings and scheduled looks continue during a watch; new episodes queue for a later round, and an episode whose watch reported starts nothing again, including across a host runner move. A watch that ends without reporting leaves the episode eligible at the next look that still finds it.
    ///
    /// WATCH. Every host watches itself. Where its host settings name an operator's directory — `--watch-dir ~/Developments/control`, known to orca — the watch runs there under that directory's own rules, with none of the stock instruction added. Otherwise the stock watch runs in the fleet's own directory, `stock-watch/` under the cache directory (`afkconvoy help cache` says where it is), made, kept and registered with orca when it starts. Its short instruction carried in the binary says: find what holds the fleets' work back, fix it with the watch's authority, check that the work goes on and say what it did. Every watch leaves spending money or needing an account, a credential or a person's hands; what cannot be undone; taking a secret or private data off the host; and changing what a product is for or who it is for with a human, acts on none and lists each in its command report, which `doctor` shows as `report`. Unplaced host kit models are low priority: a watch started for other work takes them too, and they alone start a watch at most once a day, across runner restarts. The watch researches them with its web tools and places them through `afkconvoy kit --research <file>` (`afkconvoy kit --help` gives the checked file format). Its host runner takes a look every {const:WATCH_EVERY} seconds (`--watch-every`): `afkconvoy doctor --all --since <until of its last look>`, the fleet's own code, which runs no model, keeping `until` itself. A look that holds something an operator would act on — an issue waiting on a human, or a finding that is not `info` and not of the watch itself: a fleet instance not alive or broken, a failed tick or step, a worker whose health observation is overdue among them — starts one watch. Each condition is one finding of the look, and the watch acts on it by its `kind` and `watch_id`, never its words: a finding said in other words is the same episode. Doctor finds an observation overdue only past a tick's chance to make it: past its time by more than the sleep and the time of the last tick of the fleet instance whose claim holds the worker's worktree, as that instance's `doctor.fleet.last_tick` says them (`sleep`, `took`), or the longest any instance on the host says, for a worktree no claim holds. A fleet that stopped ticking, or a tick that hangs, leaves it missed past that grace. Its episodes stay covered while it runs and after it reports; an episode lasts while each look still holds it. An unreported watch leaves its episodes and daily research reservation eligible at the next look that still finds the work, including after a restart. A watch retains one resident interactive Agent session in the watch directory across independent watch rounds. Each round has its own ID, delivery receipt, report and execution hour, excluding queueing, standby and earlier rounds. It is given its brief as a file in the cache directory — what the look answered, why it started, and what each earlier watch on the same items did — with authority over the fleets of this host and an hour, its continuations together. It runs on the harness this host last started a worker on that took its brief, then the host kit's order, never one paused or with a fault on record, started trusted with prompts skipped, interactively in its terminal titled "afkconvoy watch", with the brief as its first message, as a worker is. A human may watch it and type to it. Its last step is the exact `afkconvoy watched --watch <id> --text-file <report.json>` line at the end of its brief (`afkconvoy watched --help`): the report records what held work back, each action, what it verified, each human question and answer line, and what remains pending. Reporting completes only that round and leaves the healthy Agent and Terminal standing by for the next message. Standby without pending work sends no prompt and runs no model. Only an exit, proven harness or context failure, or explicit reset replaces the owned executor, after confirming it ended. A watch ended without that command says it did not report. Printed output is no record. Worker health observes its own terminal; a stopped watch with no report is nudged. A watch whose harness gives out — a usage limit, capacity stop, refusal or failed exit — is ended and continued on the next harness that can start it, with the same look and terminal evidence of its work so far, one session at a time; the fault is recorded (`host_kit.worker_health`), and that harness is skipped until its hourly probe answers. Looks continue while a round runs. The host runner durably queues new episodes and merges repeated observations of queued, executing and reported episodes. Before the next round, a fresh look removes recovered conditions. Delivery waits for the previous report and the same Terminal's input readiness; busy input is left untouched. An observed recovery and return is a new episode, even during an older round. A host runner started again — after moving onto a newer release, say — keeps the watch it finds running. Its start, its end and each thing it did are lines of every tick log on the checkout list and of the host runner's log; `doctor` says the watch running and the last one, as `host.watch`, and a critical finding where no harness could start or continue it. `--watch-dir default` returns to the stock watch; a running watch and its continuations keep their starting directory and rules. `doctor.host.watch.directory` says the directory the next watch runs in, and `stock` whether it is the stock one, even before a watch starts; each running or last watch says its own directory and `stock` too.
    ///
    /// In a repo worked by several hosts, each human wait is answered by one host's watch. Before queueing it the host runner takes a claim on that repo's remote; the other hosts leave it alone. Its holder keeps a heartbeat published while the wait stands, even with no worker claim. Once the heartbeat is past the lease, {const:LEASE_MINUTES} minutes, another host's next look takes the wait over. The holder's next look removes the claim when the issue no longer waits on a human. Only the holding host carries it into a round and its report outboxes. Each sending through `--notify-command` and each asking-session question re-reads the claim immediately before sending; a host that lost it drops the outbox item and logs the holder's name. Host conditions remain their own host's.
    ///
    /// ASKING SESSION. Every human entry of a recorded watch report becomes a durable question in a second resident interactive Agent and Terminal beside the watch, in the watch's directory, titled "afkconvoy asks". It starts when the first question is pending and does no watch's work: it runs no watch round, takes no look and reports nothing through `afkconvoy watched`. What the operator tells it in the session (a host fix or install, a ruling, a hand-back, a ticket) it carries out at once, as the operator acting, within the watch's bounds. Its questions use the same episode identities as `--notify-command`: a human wait's `waiting_on_human` identity, or a host condition's finding identity, once per episode across reworded reports, host runner restarts and upgrades. One question is delivered at a time, only when input is ready, with ADR-0040's request key and session-turn proof. Each delivery is marked as the host runner's and names the condition, issue link, watch question and answer line, and how many more wait. Recovery before delivery removes a question; recovery afterwards tells the session it is withdrawn. The session ends its asking turn with the question first, at most 180 characters, so Orca's phone push shows it and a tap opens this Terminal. It clarifies, investigates and consults with the operator, then acts with their authority through the same commands as a human. A settled question is reported by `afkconvoy asked` with the link to the operator's quoted words and their time on its issue; the session's own words establish no human intent. Only then is the next delivered. A question put aside by the operator stays pending and is listed on request. Standby without pending work calls no model; the resident is replaced only on exit, a proven harness fault, an unusable context or an explicit reset. `doctor.host.asking` and the panel show its state, current question and those waiting.
    ///
    /// With `--notify-command <COMMAND>`, the host runner runs that shell command afresh for each human entry of a recorded report, with one JSON object on stdin: `host`, `watch` (the round id), `condition` (the identity from the round's look), `question`, `answer` (the answer line), `url` (the issue source URL, null for a host condition) and `panel` (the URL actually served, null if unavailable). The question and answer leave the host only through the command the operator chose; no channel is built in. With none named nothing is sent; `--notify-command default` removes it. A successful sending is kept beside the reports across runner restarts and upgrades. The same condition is sent once per observed episode, even when later reports reword its question and answer or omit it. A human wait uses the identity of `waiting_on_human` (its issue and source); a host condition uses its finding identity. Only observed recovery ends an episode. A failed command, or one past {const:NOTIFY_SECONDS} seconds, is tried again at the next look, on a separate sender that holds back no look, watch round or tick. {const:NOTIFY_FAILURES} consecutive failures for one entry are a doctor finding. `doctor.host.notify` says the command in force and the last sending: watch, condition, answer, command, start and end times, and outcome.
    ///
    /// A repo nobody works is reported across hosts: every look reads the trackers its checkout list names for ready repo-labelled issues, including repos absent from that list. After more than an hour with no open blocker or claim, counted from the later of readiness and the last blocker closing, and with no remote heartbeat within the 1800-second lease, the host runner opens one tracker issue per repo carrying its repo and escalate labels. A tracker claim settles simultaneous looks; only the opening host raises the finding and keeps its watch episode until recovery. The report names the waiting issues, wait and newest heartbeat host and age, or none. A later claim or live heartbeat closes it and names the taking host. Read failures are findings, never evidence that nobody works a repo. The watch must ask which host should take it and must never add the repo to its own checkout list.
    ///
    /// WATCH RECOVERY. The fixed Terminal name is "{const:WATCH_TITLE}". The host runner records a launch intent before creation and the exact Terminal handle and public pane identity before sending its brief. Normal checks use that exact ID first; a restarted or upgraded runner reuses the confirmed session, its input and unfinished progress. If the ID is missing or stale, it queries Orca's public Terminal list and accepts only a unique candidate proven by its saved pane identity or its own startup acknowledgement. The name alone proves no ownership or uniqueness. Uncertain creation is never repeated. Each round's prompt intent saves a UUID request key before delivery; recovery replays the exact Terminal submission with Orca's supported --retry-request protocol and retains its receipt. An unproven or unsupported delivery pauses rather than sending fresh input; another harness or runner restart cannot bypass that pause. The existing command, model, directory rules and watch guard remain in force. A confirmed missing Terminal ends its watch after the marked executor is confirmed stopped; the next look may start eligible work as usual.
    ///
    /// `doctor.host.watch.association` shows {title, status, session, watch, directory, terminal, identity, request, receipt, problem}: the fixed name, launch status, watch ID, starting directory, exact handle, public Orca metadata and recovery problem. status is unassociated, creating, ready, sending, running, idle, ended or uncertain; absent fields mean no association has been saved. The association record is `watch/terminal.json` under the cache directory; briefs, exit logs, startup acknowledgements and execution claims remain beside each watch. For an unverified or ambiguous candidate, inspect `orca terminal list --worktree path:<watch-directory> --json` and `orca terminal show --terminal <handle> --json`. Rename unrelated same-name sessions with `orca terminal rename --terminal <handle> --title <title> --json` and let the runner check again; those sessions stay open and unassociated. For an uncertain creation or first prompt, inspect the retained session and record a complete report with `afkconvoy watched --watch <id> --text-file <report.json>` (see its help). State what remains pending; this settles only the named round. The healthy executor stays; unresolved association still pauses further delivery. Keep its files and execution claim. A live Terminal, an idle Agent and printed output do not finish a watch; its explicit report does. The resident session is shown separately as host.watch.session with its id, Terminal, status (active, standby or unavailable) and current round. host.watch.pending lists queued episode/item pairs; host.watch.rounds lists retained rounds and their reports; each round includes delivery (its archived request, receipt and execution evidence) and deadline (null before execution); looked and until retain the look interval. An idle resident is never nudged for an absent report.
    ///
    /// The watch operates for every repo on the checkout list (`afkconvoy checkouts`), bound or unbound. It may trust a worktree for a harness, restart, nudge, continue or switch a worker and change host settings. Once the cause of a fleet fault is gone it hands the escalated issue back with its current `doctor.waiting_on_human.answer` line, naming that repo's tracker, wakes the checkout after a `gh` change and checks that the next tick dispatches or continues it and its worker takes its brief. The watch answers escalations with `rule`, handing back, `send`, closing or changing blocked-by, using the same commands as a human and starting rework counts over. Both watch directories leave only the four human kinds above, listing each question and answer line in the watch's record; `doctor` lists what it did.
    ///
    /// A lasting fix may be code in any listed repo, even one whose fleet cannot start workers. The watch makes it in a linked worktree of its own from that repo's configured base branch, named `{const:WATCH_CHECKOUT}` or `{const:WATCH_CHECKOUT}-<digits>`, on its own branch; never in another worker's or watch's checkout. It commits and pushes, opens the PR against that repo's base branch on no issue, then takes a turn with `afkconvoy land --watch` to merge, gate, record green and land. This reuses a watch's landing path and attribution, with no intake or flow label, and respects fleet locks. It records the repo and PR.
    ///
    /// When the fix must reach an installed tool, the watch waits for landing and has that repo's fleet cut a version through its declared cut steps (`afkconvoy cut --help`): it wakes the checkout, reads `plan.cut` and fixes what holds back the next idle tick. A mechanically failed cut is continued with its report's `cut --from <step>` line once its cause is fixed. It checks publication and installation; for afkconvoy, it lets the host runner move onto the version itself, reading `doctor`'s `host.installed` and `checkouts`'s host runner version, never killing or restarting it. Then it repeats the check that showed the fault and checks that the work goes on. Its report records each fix, PR, cut, version and evidence, the four human kinds left alone and anything still pending; landing or installing alone proves no recovery. All fits within the watch's one hour: anything unfinished is recorded as pending.
    ///
    /// It samples each worker every ten seconds. A hot worker becomes a doctor finding and starts the host's watch once per episode. Its look carries the worker, issue, worktree, terminal, condition, duration and the evidence taken down as it turned hot: process tree, five-second sample, resident memory, disk writes and largest directories. The watch takes no slot of concurrency. There is one watch per host, with one hour shared by its continuations.
    ///
    /// Each fleet instance also keeps a log on this host, its tick log, in a file under the cache directory (runs/; `afkconvoy help cache` says where it is and what moves it): one line when it starts, one as each tick starts — with why, and the command whose wake started it — and one as it ends — what `tick` answers for it — and one of each event that reached it: a wake heard during a tick, the host waking, being told to stop. Every command that changes fleet state writes a line of its own to the same file as it acts (its `--help` says so under "What it logs", and [`afkconvoy help tick-log`](help:tick-log) says what the line holds). It keeps every line of the last {const:LOG_KEPT_HOURS} hours, and never fewer than {const:LOG_KEPT_LEAST} lines nor more than {const:LOG_KEPT_MOST}. A line is written whole as it happens, so killing the host runner loses none, and none a command appends while the fleet instance cuts the file back is lost. Nothing reads the log to decide anything; `afkconvoy doctor` reads it to say what the last ticks did, which steps failed, and the wakes, starts, stops and host wakes of late.
    ///
    /// ## What it answers
    ///
    /// Before a first start: a checkout off the checkout list must name `--tracker <owner>/<repo>` or `--tracker default` (its own repo, which must be private). Missing answers refuse with a non-zero exit and write nothing: {"error", "asks", "see"}. Each `asks` item has `setting`, `question`, `default`, `recommend`, and `options`, each with `option` and `line`; `see` prints [`afkconvoy help first-start`](help:first-start). Settle these questions with the user before running the chosen lines. Existing checkouts and saved host settings run as before.
    ///
    /// Once its checkout's fleet instance has ended its first tick, with exit 0:
    ///
    /// ```json
    /// {"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj", "added": true,
    ///  "instance": "me-h1-1a2b3c4d",
    ///  "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
    ///                  "panel": "http://127.0.0.1:7070/", "started": true, "replaced": null},
    ///  "tick_log": "<cache>/runs/hkj-0123456789abcdef.jsonl",
    ///  "first_tick": "tick 1: dispatched #12, #15 · holding 2/6 · frontier 4 · next in 30s",
    ///  "concurrency": {"from": {"in_force": 3, "said_by": "default"},
    ///                  "to": {"in_force": 6, "said_by": "checkout"}},
    ///  "tracker": {"from": null, "to": "me/issues"},
    ///  "harness": {"from": {"in_force": "claude-code", "said_by": "host_kit"},
    ///              "to": {"in_force": "kimi", "said_by": "checkout"}},
    ///  "settings": {"hot_cores": {"from": null, "to": 4.0}},
    ///  "warnings": []}
    /// ```
    ///
    /// - `repo`: the repo the checkout is of, as its remote `origin` names it.
    /// - `added`: false when the checkout was on the list already. Where its fleet instance runs, the flags of this `run` that are its fleet instance's own change nothing, and the answer is that instance, with its own first tick.
    /// - `host_runner`: the one that answered — its pid, its version, its log and where it serves the panel, null where it serves none — and whether this `run` started it. `replaced`: the host runner of an older release this `run` stopped to start it, as {"pid": 4100, "version": "0.5.13"}; null where it stopped none.
    /// - `tick_log` and `first_tick`: the fleet instance's tick log, and the line that ended its first tick. `<cache>` stands for the cache directory, whole in the answer.
    /// - `concurrency`: what this `run` changed of its checkout's concurrency, as {from, to}, each the one in force before and after, as `plan` says it: {in_force, said_by} — said_by `checkout` or `default`. null where it named none, or named the one in force.
    /// - `tracker`: what this `run` changed of its checkout's tracker, as {from, to}, each the repo named, `<owner>/<repo>`, or null for the checkout's own. null where it named none, or named the one it has. A `run` that kept it with its checkout's entry before it started the host runner says it in an error the host runner answered too: the tracker stays said.
    /// - `harness`: what this `run` changed of its checkout's harness, as {from, to}, each the one in force before and after, as `plan` says it: {in_force, said_by} — said_by `checkout` or `host_kit`. null where it named none, or named the one in force.
    /// - `settings`: what this `run` changed of the host settings, each by its key — its flag's name, as `hot_cores` for `--hot-cores` — as {from, to}, in its flag's units; null is the default. {} where it named none, or named each as it was.
    /// - `warnings`: what its fleet instance warned of until its first tick ended — the startup probe's warnings among them — also on stderr; empty where this `run` waited on no start.
    ///
    /// Each fleet instance's lines — `run` prints none of them. Its tick log holds them, and `afkconvoy doctor` reads them back, but for the `start:` lines after the first — the claims its start took — which are lines of the host runner's log, after the checkout (`/Users/me/Developments/hkj: start: took #4, #7 stamped with this host`):
    ///
    /// ```
    /// start: run as tester-h1-1a2b3c4d on h1, its workers started with `claude`
    /// start: took #4, #7 stamped with this host        only when a start took or lost claims
    /// start: took #4, the run of the patrol sweep stamped with this host
    /// start: took #4 stamped with this host · swept 1 dead heartbeat(s) that held nothing
    /// stop: holding no claim, it took its heartbeat with it
    /// stop: it left its heartbeat with the claims it holds: the next start on this host takes them
    /// tick 1: started (start)
    /// tick 1: dispatched #12, #15 · holding 2/3 · frontier 4 · next in 30s
    /// tick 2: started (timer)
    /// woken by park #46 during tick 2: tick 3 follows
    /// tick 2: nothing to do · holding 2/3 · frontier 4 · next in 60s
    /// tick 3: advanced #12 → review · dispatched #12 · holding 2/3 · frontier 0 · next in 90s
    /// tick 4: advanced #12 → land · holding 1/3 · frontier 0 · next now
    /// tick 5: dispatched #12 · holding 2/3 · frontier 0 · next in 90s
    /// tick 6: escalated #15 (workup: consult let it through) · holding 1/3 · frontier 0 · next now
    /// tick 7: dispatched #71 · skipped #68 (blocked by #71) · holding 2/3 · frontier 0 · next in 90s
    /// ```
    ///
    /// Every tick has a line as it starts, before it does anything, naming why: `timer` (its sleep ran out), `woken` (a wake ended the sleep, or was heard during the tick before: `woken by` the command that sent it, and what that acted on, where it named them), `start` (the fleet instance's first) or `follow-up` (the tick before left a worker to start, or could not reach GitHub, so it runs with no sleep). A wake heard during a tick is a line as it is heard — the first of each tick — and the next tick starts as that one ends: ticks never overlap. The host waking from a sleep is a line, as the fleet instance notices it — a rest that took a minute past its length — and being told to stop:
    ///
    /// ```
    /// tick 4: started (woken by verdict #121)
    /// tick 4: started (woken)                          a wake that named nothing
    /// tick 5: started (follow-up)
    /// host woke: after 3600s asleep
    /// ```
    ///
    /// The host runner keeps two lines of its own in a fleet instance's tick log: as it breaks, and as it is back.
    ///
    /// ```
    /// broken: it panicked: <what it said> · since 2026-10-24 09:12:00 UTC · started again at 2026-10-24 09:13:00 UTC, in 60s
    /// back: broken since 2026-10-24 09:12:00 UTC, for 75s
    /// ```
    ///
    /// A tick's line names what it did — lost, escalated, released, left, settled, ended, advanced, unlocked, reclaimed, dispatched, skipped (a frontier issue found blocked just before its claim, with what blocks it), nudged, waiting on its own background work (with since when: `#7 (since 2026-10-20 23:03 UTC, 12m ago)`), continued, restarted, asked for a turn again for (a landing claim whose last turn answered a wait, asked again with no harness started) — then, when admission held back a worker it would have started, which threshold did (`admission holds back new workers: the load 31.20 is past 28.00, 200% of its 14 cores`), then the claims held out of the concurrency, and the frontier left. The claims a fleet lock has it leave alone take no slot and are counted apart (`holding 1/3 · 2 left alone`), and so are those whose worker declared a verdict not acted on yet, or that are landing with their worker not changing the tree (`holding 3/3 · 1 taking no slot`), as are those past the bound — a start took them, or the checkout's concurrency was lowered under them (`holding 3/3 · 1 past the bound`). A step that failed is in the line as `FAILED <step> #<issue>: <why>`, and a tick that could not start as `tick N: FAILED: <why>`; the loop goes on either way. One cause that stopped several steps is said once: `FAILED 7 steps: <why>`.
    ///
    /// `advanced #12 → review`: the tick read a verdict that left #12 at `review`. Where a role performs that stage the same tick starts its worker, and says so: `dispatched #12`. An issue advanced to no stage — its workflow ended, or it was parked — is named alone.
    ///
    /// `escalated #15 (workup: consult let it through)`: the tick handed #15 to a human, whichever step did, with why in a few words — the stage first, where it was at one. It is named as nothing else. The comment on the issue says why in full and ends with the line that answers it; `afkconvoy doctor` lists it under `waiting_on_human`.
    ///
    /// Between ticks a fleet instance sleeps {const:TICK_BUSY_SECONDS} seconds while it has work, or {const:TICK_IDLE_MINUTES} minutes once {const:TICK_IDLE_AFTER} ticks in a row were empty. Commands that change fleet state wake it sooner, as does `afkconvoy wake`. A fleet on this host that closes an issue or sees it closed wakes every other fleet on the same tracker whose open issues it blocked, across repos. The wake names the closed issue and the fleet instance that saw it, in the woken fleet's tick log. Even with another blocker still open the fleet is woken; that next tick reads GitHub and takes up only what is unblocked. A change made on GitHub, or by another host's fleet, is read at the next tick: no webhook reaches the host. Where the tick acts on such a change, its line says what changed since the tick before, and who changed it where that is known:
    ///
    /// ```
    /// tick 8: lost #46 to instance beta-1a2b3c4d on host h2 (takeover) · holding 1/3 · frontier 0 · next in 90s
    /// tick 9: dispatched #45 (labelled ready-for-agent since tick 8) · holding 2/3 · frontier 0 · next in 90s
    /// tick 9: released #231 (the issue was deleted) · holding 1/3 · frontier 0 · next in 90s
    /// tick 9: left #97 (closed by @alice) · holding 1/3 · frontier 0 · next now
    /// tick 9: skipped #320 (blocked by #379, added since tick 8) · holding 1/3 · frontier 0 · next in 90s
    /// ```
    ///
    /// `lost #46`: a claim this instance held is another's now — taken by a takeover, or `(stale: past the lease)`, as a stale claim, this instance's heartbeat being past the lease. On another host, the tick closes its local worker's terminal and adds `stopped its worker`, keeps the worktree and pushes nothing of its branch. A failed close is an error under `stop_taken`, retried next tick. `released #231`: its issue was deleted, `(the issue was transferred to acme/other#12)`, where GitHub says it moved, or `(there is no such issue)`, a 404 with no move. `left #97`: a claim with no PR whose issue was closed, by whoever GitHub says closed it (`closed on GitHub` where it says no one), is settled — nothing of the fleet's landed. `labelled …` and `added since …` say what the tick before saw otherwise. A change the tick does nothing about is in no line; nor is one made before the fleet instance's first tick, which has no tick before it, nor anything of the kind in what `afkconvoy tick` answers.
    ///
    /// `next now` in place of `next in <n>s`: the tick settled an issue, or advanced one without starting what follows, and left a slot free, so what follows — a consult, the landing of a PR, an issue the landing unblocked — is the next tick's to start, and that tick starts with no sleep.
    ///
    /// So does the tick after one that failed on a read GitHub could not be reached for (`GitHub could not be reached: …`): nothing says the next read fails too. At most {const:BACK_TO_BACK} ticks run back to back; then it sleeps, whatever the last one left.
    ///
    /// The host runner serves the panel from its start until it ends, on the panel's port of the host settings — {const:PANEL_PORT} where they say none, or any free port where that one is taken: every fleet of the host as one web page — the host first, then what waits for a human across every repo, then each repo with its workflows as lines of their stages and its landing turns — brought up to date in the browser after every tick of any fleet instance, by itself. `afkconvoy panel --help` says what the page shows, and `afkconvoy panel --open` where it is served. Its address is a line of the host runner's log:
    ///
    /// ```
    /// panel: http://127.0.0.1:7070/
    /// ```
    ///
    /// `--panel <port>` sets that port, a host setting, which the host runner reads as it starts: one that runs already serves the panel where it does, and the next one — after `afkconvoy stop --all`, or an upgrade — on the port set.
    ///
    /// Errors, before the first tick, as {"error": …} with a non-zero exit: the host settings cannot be read or written; a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run); the repo fails the startup probe; its tracker — the one `--tracker` names, or the one the checkout says — is public (the error names it, with the line that says a private one); the config cannot be read; the host kit cannot be read; the worker launch command of a harness the fleet could be asked for resolves to nothing, or does not take the flags the fleet adds for its role or worktree trust; another checkout of the repo is on the checkout list; the checkout has no remote `origin`; the login shell did not answer within 20 seconds; the host runner is of a newer release than this `run` (the error names both versions, and nothing was changed); the host runner did not say its version within 10 seconds; the host runner of an older release, told to stop, still held the host after 30 seconds; no host runner answered within 35 seconds; the host runner ended before it answered, or is stopping; the checkout's fleet instance is stopping, told by `afkconvoy stop`; no first tick within 30 minutes; the checkout list cannot be written. Each error a host runner gave says which, as `host_runner`.
    ///
    /// ## What trips you up
    ///
    /// - Stop the host runner with `afkconvoy stop --all`, or SIGTERM to its pid, `host_runner.pid` in any `run`'s answer: each fleet instance finishes the step it is in, makes none of the steps left in its tick, and ends — within a second when it sleeps — and the host runner exits once every one has, keeping the checkout list: the next `run` starts every checkout on it. No claim is released on the way out and no worker is stopped: the workers work on, and the next start on this host takes the claims back. Holding no claim, a fleet instance removes its heartbeat from the remote; holding one, it leaves it, and that next start removes it. A second signal ends the host runner at once, as SIGKILL does, with no goodbye: what that leaves, a later start on this host removes once it is past the lease. A patrol's run it takes back is ended by its first tick once its worker is gone or has stopped.
    /// - Ctrl-C while `run` waits ends that `run` alone: the host runner is in a process group of its own, and goes on with the checkout.
    /// - Of a `run` whose checkout's fleet instance runs already, the flags that are that instance's own — `--instance`, `--host`, `--worker-no-proxy` — change nothing: `afkconvoy stop` it, and run again with those you want. `afkconvoy stop --help` says how to stop one checkout and how to stop them all. The host settings it names change all the same, for that instance and every other on the host, and so do its checkout's `--concurrency` and `--harness`, for that instance alone, from its next tick. The running instance is kept.
    /// - A checkout's concurrency is that checkout's, on this host: another host works the repo at its own, and a checkout that says none holds {const:CONCURRENCY}. How many workers a repo gets at once is what the host can carry — its cores, its memory, its logins — and no fact of the repo, so the repo config says none: a `concurrency` it still names is read no more. Raising it for an afternoon, or lowering it while the host is needed for something else, is one `run`, and no commit.
    /// - A checkout's harness is that checkout's, on this host, like its concurrency. `--harness kimi` names it; `--harness default` follows the host kit again. The next tick reads it and does not change a running instance. Workers start on the named harness with the host kit's command and that harness's model order; a `model:` label selects only a model of that harness. A role that holds no limits for it starts no worker, and no other harness is started in its place. Where that harness cannot start — `doctor` says `account fault`, or `model fault` once no placed model is left, or that its launch command cannot start — no worker starts on the next harness. A worker already at work on another harness is continued onto the named one, and no second writer starts until the old one has ended. A worker already on it is left. Clearing with `default` continues a worker that is not on the host kit's first eligible harness, by that same rule. `switch` does not change the name and does not continue that checkout's workers onto the kit's new first. The host's watch is not a worker of the checkout. A word that is not a harness, or a harness the host kit does not hold, is an error and writes nothing.
    /// - Without `--instance` each start mints a new instance id, named for this host: the answer says it as `instance`, and `afkconvoy doctor` as `fleet.instance`, for `plan --instance` and the commands that make one transition by hand.
    /// - A host setting is the host's, not a `run`'s or a repo's: `--hot-cores 4` named in one checkout holds for every fleet instance on the host until a `run` names it again, and `--hot-cores default` changes it back. The variables that stand for the flags are read from the environment of a `run` and of nothing else — not of the host runner, nor of `tick`, `plan` or `doctor` — so one exported in your shell's rc files is named again by every `run` started from that shell.
    /// - The panel is served to this host only, and shows a tick, not this second: between two ticks it changes nothing but its countdown. It costs GitHub no read of its own.
    /// - Every worker launch command comes from the host kit: run `afkconvoy kit` to write it again. It is $XDG_CONFIG_HOME/afkconvoy/kit.json, or $HOME/.config/afkconvoy/kit.json, kept on this host and never pushed. `--worker-command` is gone. On a host with no host kit, `run` writes one as `afkconvoy kit` does, at a terminal or not, asking nothing: it says on stderr where the file is, what it holds and what was left out, and goes on to its first tick. At each worker launch the latest kit is read; a known harness with no entry or command is discovered in the login shell and its checked stock command merged into the kit without model probes. Explicit commands are respected, including errors. A checkout that names none starts on the host kit's first eligible harness. One that names a harness starts on that harness; the host kit still orders that harness's models. A start checks the named harness alone, and does not refuse the checkout because some other harness cannot start.
    /// - The fleet starts every worker launch command under `nice`, with a `TMPDIR` of the worktree's own, adding its harness's model flag where the worker runs a model — and, in a bound repo, what puts the worker in its role, under the guard: [`afkconvoy help worker-launch`](help:worker-launch) says, harness by harness, the line each worker is started with, what each part is for and what loads from where.
    /// - In a bound repo whose checkout names no harness, before any worker is started, the command of every harness the fleet could be asked for is checked: the first harness in the host kit's order, each harness of each role's range (`afkconvoy help roster`). One that cannot be started refuses the start, named with its harness and the range that calls for it, and no worker at all is started: no harness is ever started in another's place. A range leaves out each harness the role holds no limits for while it holds limits for another, so no worker runs a harness it holds no limits for; `plan` and `check` name each harness left out.
    /// - A role that holds no limits for the harness in force starts no worker of that role, and no other harness is started in its place. `plan` and `doctor` name the checkout, the harness and the missing limits. Where that harness cannot start — `doctor` says `account fault`, or `model fault` once no placed model is left, or that its launch command cannot start — no worker of this checkout starts on the next harness; `plan` and `doctor` say why. A role whose range on this host is empty still refuses no other role: no worker of it is started, its stages wait, and every other stage goes on.
    ///
    /// ## Worker proxy bypass
    ///
    /// A start on this host reaches some hosts past the proxy with `--worker-no-proxy [HARNESS=]HOSTS`, never a key of the repo's config: [`afkconvoy help worker-proxy`](help:worker-proxy) says what each worker is given.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Run {
        /// The fleet instance to run as [default: a new one, named for this host]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        /// How many claims this checkout's fleet instance holds at work at once on this
        /// host, from its next tick: the checkout's own, kept with it on the checkout list
        /// and never pushed; `default` changes it back [default: {const:CONCURRENCY}]
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
        /// host setting; `default` changes it back [default: a quarter of this host's memory]
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
    ///
    /// ## When to run it
    ///
    /// To stop working one repo on this host and leave the others worked: `afkconvoy stop` in its checkout, or naming it. To stop every repo — to start every fleet instance again with flags of its own, say — `afkconvoy stop --all`: the next `run`, in any checkout, starts every checkout on the list again. An upgrade needs none: the host runner moves onto the new release by itself between ticks (`afkconvoy run --help`).
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout on the list or naming one; with `--all`, anywhere on this host.
    ///
    /// ## What it reads and writes
    ///
    /// The checkout list: checkouts.json in the cache directory. `stop` takes the checkout off it; `--all` leaves it as it is.
    ///
    /// The host runner, spoken to on its socket in the cache directory (host-runner.sock), naming the checkout. Its fleet instance of the checkout — every one, with `--all` — stops as SIGTERM has it stop: it finishes the step it is in, makes none of the steps left in its tick, and ends — within a second when it sleeps. No claim is released and no worker is stopped: the workers work on. Holding no claim, it takes its heartbeat off the remote; holding one, it leaves it with the claims, and the next start on this host takes them back. Its tick log keeps its `stop:` line, as ever. With `--all`, the host runner ends once every fleet instance has, and `stop` answers once it has ended.
    ///
    /// The host runner's log has a line of each, beside the line of each fleet instance that ended:
    ///
    /// ```
    /// checkout removed: /Users/me/Developments/hkj: by `afkconvoy stop`, its fleet instance stops
    /// fleet instance ended: /Users/me/Developments/hkj: {"instance":"me-h1-1a2b3c4d","stopped":true,"ticks":12}
    /// host runner told to stop by `afkconvoy stop --all`: it keeps the checkout list
    /// host runner stopping: told to stop, each fleet instance ends its step
    /// host runner stopped: 2 checkouts kept on the list
    /// ```
    ///
    /// With no host runner running, `stop` takes the checkout off the list itself, and says so in that log; `--all` changes nothing.
    ///
    /// ## What it answers
    ///
    /// Once its fleet instance has stopped, with exit 0:
    ///
    /// ```json
    /// {"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj", "removed": true,
    ///  "instance": "me-h1-1a2b3c4d", "stop": "stop: holding no claim, it took its heartbeat with it",
    ///  "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
    ///                  "panel": "http://127.0.0.1:7070/"}}
    /// ```
    ///
    /// - `removed`: false where the checkout was not on the list. Where no fleet instance of it ran either, nothing changed, and that is all it answers: a checkout not on the list exits 0.
    /// - `instance` and `stop`: the fleet instance that stopped, and the `stop:` line its tick log keeps; null where none ran — the host runner could not start it, or no host runner runs.
    /// - `host_runner`: the one that answered — its pid, its version, its log and where it serves the panel — or null where none runs. `<cache>` stands for the cache directory, whole in the answer.
    ///
    /// With `--all`, once the host runner has ended:
    ///
    /// ```json
    /// {"stopped": [{"checkout": "/Users/me/Developments/hkj", "instance": "me-h1-1a2b3c4d",
    ///   "stop": "stop: it left its heartbeat with the claims it holds: the next start on this host takes them"}],
    ///  "checkouts": 2,
    ///  "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
    ///                  "panel": "http://127.0.0.1:7070/"}}
    /// ```
    ///
    /// - `stopped`: each fleet instance it stopped, with its checkout, its id and its `stop:` line; [] where no host runner runs.
    /// - `checkouts`: how many the list keeps: the next `run` starts each.
    ///
    /// Errors, as {"error": …} with a non-zero exit: a second checkout of a repo on the list, naming the checkout to stop it from; the checkout list cannot be read or written; the host runner ended before it answered; its fleet instance did not stop within 10 minutes — it is off the list, and still finishing the step it is in; the host runner did not end within 30 seconds of answering `--all`. Each error a host runner gave says which, as `host_runner`.
    ///
    /// ## What trips you up
    ///
    /// - SIGTERM to the host runner's pid is `stop --all` but for the answer, and the line that says who told it. A second signal ends it at once, as SIGKILL does, with no goodbye.
    /// - `stop` of the last checkout on the list leaves the host runner running, with none: the next `run` hands it its checkout.
    /// - A `run` of a checkout whose fleet instance is stopping is refused until it has stopped. `stop` answers once it has: run it after.
    ///
    /// ## Releasing its claims: `--release`
    ///
    /// Plain `stop` is the default: there is no drain. The claims stay on the remote, the workers keep working, and the next start on this host takes the claims back. An orphaned claim is still recovered by continuation, and never released.
    ///
    /// `stop --release` is the drain an operator names. It is read while the checkout is still on the list, so the tracker is the one its entry says. While the fleet instance — `--instance`, or the one this host's runner holds of the checkout, else the newest its tick log names — still ticks, what it holds is read: a cut it holds is an error, and a claim whose worker declared a verdict, or whose issue is to be settled, is named in `kept` and keeps the checkout on the list, with nothing released. Otherwise the fleet instance is stopped as plain `stop` stops it, and then, in one push, each `claim/<owner>/<tracker>/<n>` it holds and each claim on its patrols' runs is deleted, each only while it is still that instance's, leased to the sha just read. One that moved since it was read is named in `kept`, nothing is released, and the checkout is back on the list: `afkconvoy run` starts its fleet instance again. Once they are gone, each worker is ended: its terminal is closed, and its worktree removed only where it holds no commit past its target and a clean tree; one that does stays, named in the answer. A patrol's checkout is removed as a run's end removes one, unless it left a commit or a change. The issues keep their labels, an open PR stays open, and an issue still ready is back on the frontier. Then the checkout leaves the list, and holding nothing, the fleet instance takes its heartbeat with it. Another host's claims are not touched.
    ///
    /// ```json
    /// {"checkout": "/Users/me/Developments/hkj", "instance": "me-h1-1a2b3c4d", "removed": true,
    ///  "released": [{"issue": 7, "claim": "refs/convoy/claim/acme/tracker/7",
    ///                "worktree": "/Users/me/orca/hkj/acme-tracker-7-fix", "worktree_removed": false,
    ///                "worktree_kept": "it holds an uncommitted change"}],
    ///  "kept": [], "heartbeat_removed": true, "stop": "stop: …", "host_runner": null}
    /// ```
    ///
    /// Errors of `--release`, as {"error": …} with a non-zero exit, each with nothing released and the checkout on the list as it was: the checkout is not on the list, so its tracker is not said; no fleet instance of it is known on this host — name one, `--instance`; the fleet instance holds the cut, which is a publish and no release ends; the fleet could not be read.
    ///
    /// `--all` takes no `--release`: stopping the host runner keeps the list. Off the list the tracker is not said: `scan` and `release` then exit non-zero, saying so, while the remote holds claims named for a tracker.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// To see every repo of this host at once: which checkouts its host runner works, whether each one's fleet instance runs, and whether it has work. A session that looks after every repo of the host asks this — and `afkconvoy doctor --all` for what is wrong with each — instead of keeping a list of its own: the checkout list is the one home of which checkouts there are.
    ///
    /// ## Who runs it
    ///
    /// The operator, or a session of theirs that looks after every repo of the host: anywhere on this host, since it is about no one checkout.
    ///
    /// ## What it reads and writes
    ///
    /// The checkout list: checkouts.json in the cache directory (`afkconvoy help cache` says where it is). The host runner, asked on its socket which checkouts it holds a fleet instance of: where none runs, the list alone answers. Each checkout's tick log, for whether its fleet instance is broken and for its last tick, and its entry, for its concurrency, its tracker and its harness. It writes nothing, asks neither GitHub nor the remote, and makes no startup probe: it is safe beside a live fleet, as often as you like.
    ///
    /// ## What it answers
    ///
    /// ```json
    /// {"host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
    ///                  "panel": "http://127.0.0.1:7070/"},
    ///  "checkouts": [{"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj",
    ///   "instance": "me-h1-1a2b3c4d", "state": "running", "broken": null,
    ///   "ticked": 1767225600, "idle": false, "holding": 2, "frontier": 1,
    ///   "concurrency": {"in_force": 6, "said_by": "checkout"},
    ///   "harness": {"in_force": "kimi", "said_by": "checkout"}, "tracker": "me/issues",
    ///   "tick_log": "<cache>/runs/hkj-0123456789abcdef.jsonl"}]}
    /// ```
    ///
    /// - `host_runner`: the one running — its pid, its version, its log and where it serves the panel — or null where none runs. `<cache>` stands for the cache directory, whole in the answer.
    /// - `checkouts`: each checkout on the list, in its order; [] with none:
    ///
    ///   - `checkout`: its root. `repo`: the repo its remote `origin` names, as `run` answers it; null where it is no git checkout any more
    ///   - `state`: `broken` — its fleet instance is broken (`afkconvoy run --help`), as its tick log says, whether or not a host runner runs to start it again; else `running` — the host runner holds its fleet instance; else `stopped` — no host runner runs, or it holds none of it. `afkconvoy run` in the checkout starts a broken or stopped one at once
    ///   - `broken`: as `afkconvoy doctor` answers it under `fleet`: {why, since, next, instance} — why, as it last broke; since when, and when the host runner starts it again, in epoch seconds, null where no host runner runs; and the fleet instance that broke, "" where none had started. null where it is not broken
    ///   - `instance`: its fleet instance id: the one running, null until its start is logged; else the last one its tick log names, null where it names none
    ///   - `ticked`: when that fleet instance's last tick ended, in epoch seconds; null with none
    ///   - `idle`, `holding`, `frontier`: as that tick ended: whether the fleet instance was idle — it held nothing, nothing of its was on the base branch, and its frontier was empty — the claims it held, and the frontier issues it left undispatched (`afkconvoy tick --help`). null with no tick, or one that failed whole; `idle` null too of a tick logged by an older release
    ///   - `concurrency`: {in_force, said_by}: how many claims its fleet instance holds at work at once on this host, and who says it, as `afkconvoy plan` answers it: `checkout` (`afkconvoy run --concurrency`) or `default`. {error} where its config cannot be read, or it is no git checkout any more
    ///   - `harness`: {in_force, said_by}: the harness its workers start on, and who says it, as `afkconvoy plan` answers it: `checkout` (`afkconvoy run --harness`) or `host_kit`. {error} where the host kit cannot be read, or it is no git checkout any more
    ///   - `tracker`: the repo its fleet reads and writes its issues in, `<owner>/<repo>`, as its entry says it (`afkconvoy run --tracker`); null where it says none, and the checkout's own repo is its tracker
    ///   - `tick_log`: the file its fleet instance logs its ticks in, as `run` answers it; null where it is no git checkout any more
    ///
    /// It exits 0 whatever the checkouts' states. Errors, as {"error": …} with a non-zero exit: the checkout list cannot be read; the host runner cannot say which checkouts it holds — it is of an older afkconvoy, which `afkconvoy run` in any checkout moves onto this one; it ended before it answered; or it answered nothing within 10 seconds.
    ///
    /// ## What trips you up
    ///
    /// - `idle`, `holding` and `frontier` are of the last tick, not of now: those of a stopped or broken checkout are as its fleet instance last ticked, which `ticked` dates. `afkconvoy doctor`, in the checkout, reads GitHub as it is now.
    /// - A checkout `run` is adding is on no list until its first tick ends, and is not answered until then; one `stop` took off is not answered.
    /// - A broken checkout the host runner starts again is `running` once its tick log keeps that start: it is back once the start's first tick ends, and broken again if it ends before. `afkconvoy doctor` says the same of it.
    /// - `host_runner.panel` is where the host runner serves the panel: the address `afkconvoy panel --open` answers, with no need to ask it. Where no host runner runs it is null, as `host_runner` is whole; where the host runner serves none, it alone is null, and `panel --open` says why. `afkconvoy panel --help` says what the page shows.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Checkouts,
    /// Read this host's exclusive queue and explain each wait
    ///
    /// ## When to run it
    ///
    /// Run `afkconvoy queue` from any directory on the host, as the host runner's user, to see who occupies its worker lanes and why each request waits. No checkout, login or running fleet is required. It accepts no checkout or host selector: it observes this user's local host runner across all its repos.
    ///
    /// ## Who runs it
    ///
    /// The operator, or their agent, on the host whose exclusive queue they want to read.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the host settings once and asks the host runner once on its local socket, allowing {const:QUEUE_SECONDS}s for its response, including partial lines. Settings absent use the defaults; unreadable settings fail. It reads no GitHub, git, harness, tick log or recorded gate outcomes, makes no probe or full diagnosis, sends no wake and changes no fleet, queue or settings. A stale socket is left in place. A temporary link used to reach a long socket path is removed afterwards. The runner's lock is checked only if its socket is absent or refuses the connection.
    ///
    /// ## What it answers
    ///
    /// Exactly one JSON object, exit 0:
    ///
    /// - `observed_at`: epoch seconds captured after the local response. Every duration uses this one time; clock differences clamp negative durations to zero. Settings and the queue are a bounded observation, not an atomic snapshot of both files and process.
    /// - `status`: `available` when the runner answered, including a healthy empty queue; `no_runner` when no runner listens or holds the host. No runner is a successful observation with empty arrays and `host_runner: null`, not evidence of commands running outside the queue.
    /// - `host_runner`: the answering runner's `pid` and `version`, plus the identity fields it supplies: `log` (whole path), `panel` (address or null), and `foreground` where applicable. Null with no runner.
    /// - `paused`: whether the runner has paused new grants for a release move; false with no runner; null where an older runner does not say it, which pauses nothing.
    /// - `worker`: `{capacity, used, free}`. Capacity is the host setting `gate_lanes`, {const:GATE_LANES} by default; used counts running worker requests, free is capacity minus used, clamped to zero. Eligible reservations do not count as used.
    /// - `slow`: `{capacity, used, free}` of the slow lanes, where a round's slow gate runs: the host setting `slow_gate_lanes`, 1 by default (`afkconvoy run --help`).
    /// - `fast`: `{capacity, used, free}` of the fast lane, which only a worker's fast gate takes (`afkconvoy gate --fast`). Capacity is the host setting `fast_lanes`, {const:FAST_LANES} by default; used counts running fast gates. It is independent of worker capacity: no slow gate, turn or other command holds a place in it.
    /// - `repos`: each repo lane with a running or waiting request, sorted by its whole identity, as `{repo, capacity, used, free}`. Each has capacity 1, independent of worker capacity. `repo` is the shared git directory supplied by the ticket, never inferred from a worktree's name.
    /// - `running`, `waiting`: requests in scheduler order within each array: cuts first, then ordinary commands; within each group ascending `asked`, then `number` to break ties. Cut priority never interrupts running commands. There is no priority by lane or repo. A request identifies `number` (runner-local ticket), `pid` (asking process), `command` (one line: the program and the arguments that say what is running; `afkconvoy exclusive -- ./gate.sh` stays as given; a script is named by the program it runs, its `cargo nextest` line or its `afkconvoy exclusive` invocation, and is not shown as the script; no `NAME=value`), `dir` (whole worktree or outside directory), `issue` (number or null; no lookup), `repo` (shared git directory or null for older/outside tickets), `repo_lane` (repo lane identity or null for worker), `is_turn` (whether this is a landing turn), `cut` (whether it has cut priority, false for older tickets), `asked` (request epoch seconds), `ran` (start epoch seconds or null), `gate` (`fast` or `slow` for a gate a worker runs alone, `gate --fast` or `--slow`; null otherwise), `kind` (`fast_gate`, `slow_gate`, `turn` or `command`: a landing turn is a `turn`, whichever gate it runs) and `lane` (`worker`, `fast` or `repo <identity>`).
    /// - Each request adds `elapsed_seconds` (time since `ran`, null while waiting), `waiting_seconds` (time from `asked` to `ran` when running, otherwise to `observed_at`), and `scheduling`: `{number, state, reasons}`. State is `running`, `eligible`, or `waiting`; reasons is empty for running or eligible requests.
    /// - `candidate` is true for independent candidate validation. `validation`, null for ordinary or unreported requests, is the turn's display copy of its remote candidate: {id, predecessor, commit, phase, waiting, cancelled_by, cause}. Phase follows building, validating, green, slow, landed, red, cancelling or cancelled. Waiting distinguishes constructing_worktree, validating, waiting_for_round, slow_gating, waiting_for_predecessor and confirming_process_exit; a waiting queue request without a candidate awaits capacity. Cancellation names its invalidating predecessor and cause. These copies explain current work and authorize no publication; reading queue makes no remote lookup.
    /// - Each reason has `kind`, `requests`, `capacity` and `explanation`. Kind `same_repo` means a running or earlier eligible request of this repo excludes this request in its lane, or across lanes when both are turns. `worker_capacity`, `fast_capacity` or `repo_capacity` means all places in that lane are occupied or reserved, with the limit in `capacity`. `paused` means new grants are paused. Other reasons have `capacity: null`. `requests` names each relevant `{number, state}`: `running` holds a place now, `eligible` reserves one earlier in FIFO order; it is empty for a pause. `explanation` states the constraint in words.
    ///
    /// Earlier eligible requests reserve capacity in scheduler order, with cuts before ordinary waiters and FIFO within each group. Blocked requests reserve nothing: with three worker lanes, ordinary requests of a running repo can leave two free while waiting, and a different repo can be eligible. Independently owned candidate worktrees of the same repo may validate together, and so may fast gates of the same repo, never with `same_repo` as a reason; ordinary heavy commands and serial barriers retain same-repo exclusion across worker and repo lanes. Older tickets lacking `repo` use their whole `dir` as the scheduler's fallback identity. `eligible` means allowed by this observation, not guaranteed to start next. Ordering is no fixed future start order or ETA; requests can leave and settings can change.
    ///
    /// Errors: unreadable host settings, an inaccessible socket, a runner holding the host but not listening, a closed connection, no response within the bound, a runner error, or an unsupported or unreadable response (including missing queue or runner identity fields). These print `{"error": ...}` with exit 1 and a next action: inspect the host runner log and run the command again; `afkconvoy doctor --all` provides a full diagnosis. An unsupported response needs a host runner of this release. A failed observation never becomes an empty queue.
    ///
    /// ## What trips you up
    ///
    /// `afkconvoy doctor` shows this same observation as `host.exclusive`, and the panel's command lanes are its lanes: the worker lane, the fast lane and each repo lane, with the same running and waiting requests, each named by its kind. It reports current queued commands only. For historical gate outcomes, use `afkconvoy doctor --all --since <time>`; `afkconvoy doctor --help` explains its interval boundaries. Commands that bypass the exclusive queue are absent. Request numbers belong to this runner and may change after it is replaced.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Queue,
    /// Hold every checkout on this host's checkout list, as `run` starts it in the background
    #[command(hide = true)]
    HostRunner,
    /// Print the panel — every fleet of this host as one web page — as one rebuild of each checkout shows it now, or say where the host runner serves it live
    ///
    /// ## When to run it
    ///
    /// To see every fleet of this host at a glance where no host runner serves it: the host, what waits for a human across every repo, then each repo — its workflows as lines of stages, the issues at work, queued and held back at each. Save it and open it in a browser:
    ///
    /// ```
    /// afkconvoy panel > panel.html                       every checkout on the checkout list
    /// afkconvoy panel ~/Developments/hkj > hkj.html      that checkout alone
    /// ```
    ///
    /// To watch the fleets, do not run this in a loop: the host runner serves the same page from its start, live — brought up to date after every tick of any fleet instance, by itself. Ask where:
    ///
    /// ```
    /// afkconvoy panel --open
    /// ```
    ///
    /// and open the address it answers. Run it again whenever you want that address back.
    ///
    /// ## Who runs it
    ///
    /// The operator, anywhere on the host. `--open` asks the host runner, whatever directory it is run in.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing, on GitHub or on the remote, and makes no startup probe. For each checkout it makes the rebuild `plan` makes — the open issues and PRs, the claims and heartbeats, the base branch’s commits, and in a bound repo the team and every stage record — then asks orca after the worker of each claim of its fleet instance on this host, and reads its tick log on this host for the workers that are hot and whether its fleet instance is broken. Of the host it reads admission's reading, resource findings and the host watch, the exclusive queue, the host settings and the host kit.
    ///
    /// With `--open` it reads nothing itself, and starts nothing: it asks this host's runner, on its socket, where it serves the panel. The host runner serves it from its start, to this host only, on the panel's port of the host settings — {const:PANEL_PORT} where they say none, or any free port where that one is taken (`afkconvoy run --help`).
    ///
    /// ## What it answers
    ///
    /// Not a JSON object: one HTML document on stdout, complete in itself, with no address in it that a browser would fetch. The same fleet state gives the same document. It has three parts, each made by one rendering, whether the host runner serves the page or this prints it:
    ///
    /// - The host, once: admission — whether it holds back new workers, why, and what it read of the host; the workers that are hot, in every repo, with why and what each held at the most; the watch running, and what the last watch did; the commands run alone on the host and those waiting their turn (`exclusive`); the host kit — each harness in order with its models, each one paused, and what the stock consult runs
    /// - With a human, across every repo: each fleet instance that is broken, with why, since when and when the host runner starts it again; then every open issue that waits for a human, labelled with its repo, with what kind of wait and the first sentence of the reason its comment gives, a Reason link to that comment (the report itself for an older failed patrol or cut), and the line that answers it: what `doctor` answers as `waiting_on_human`.
    /// - Each repo: a row at the top names every checkout, each with whether its fleet instance is idle, its claims held of its bound and its frontier, and links to its part. Each part then shows:
    ///   - Claims held: the claims of its fleet instance that take a slot, of the concurrency
    ///   - `Taking no slot`: shown while there are any: the claims whose worker declared a verdict not acted on yet, or that are landing with their worker not changing the tree, which take no slot
    ///   - a lane: one per workflow; an unbound repo has one, `backlog`, with the two stages `work` and `land`
    ///   - a stage: the number in its stop is the workers at it; under it the issues at work — each with its worker's state: busy, stopped and for how long, or no worker — then the queued, which are the frontier, and in a bound repo those held back there, with why
    ///   - `Patrols`: in a bound repo that names patrols: each, with what it is doing or what holds it back — one held by its `max_runs_a_day` says until when, as a time of day in UTC and how long from now
    ///   - left alone: every other open issue, with the reason `plan` gives under `excluded`
    ///   - Jira deliveries, when the repo has any: each delivery's reference, its identity, and whether it is ready for a human, not ready, or settled, as `doctor` answers under `deliveries`
    ///
    /// Errors, as {"error": …} with a non-zero exit: the checkout list cannot be read; of a checkout named — or of the one it is run in, where the list holds none — the config, GitHub or the remote cannot be read. A checkout on the list that cannot be rebuilt is said so in its part, with why, and every other is shown.
    ///
    /// With `--open`, one JSON object:
    ///
    /// - `panel`: the address to open in a browser of this host: http://127.0.0.1:<port>/
    /// - `host_runner`: the host runner that serves it: its pid, its version, its log, and that address as `panel`
    ///
    /// Its errors, as {"error": …} with a non-zero exit, none of which changes anything:
    ///
    /// - no host runner runs on this host: `afkconvoy run` starts one, which serves the panel; or print the page once, without `--open`
    /// - the host runner did not answer within 10 seconds: its log says why
    /// - the host runner serves no panel: it is of an older afkconvoy, which `afkconvoy run` moves onto this one; or it could not serve it, as its log says — or, for a fleet instance held in the foreground, as it warned
    ///
    /// ## What trips you up
    ///
    /// - Without `--instance` each checkout is seen as the fleet instance its tick log on this host names last — the one the host runner holds, or held — as the live page sees it; one whose fleet never ran on this host is seen as nobody's: its claims show as "held by <instance>", with no worker's state. `--instance` names the fleet instance of every checkout printed: name the checkout with it. `takeover --list` shows the ids.
    /// - A worker's state is this host's orca's to say. A claim whose worker runs on another host shows "no worker" here.
    /// - The panel is served at one address for as long as its host runner runs. `afkconvoy run --panel <port>` changes the port the next host runner serves it on, not the running one's: `afkconvoy stop --all` and then `afkconvoy run` moves it.
    /// - A checkout a `run` adds is on the live page at once, as starting, and filled in by its first tick. One whose fleet instance broke shows what its last tick showed, said broken, until it is back.
    /// - `--open` takes no checkout, no `--instance` and no `--host`: the page is the host runner's own.
    /// - The page the host runner serves is not this one made again and again: each repo's part is made from what its fleet instance's tick already read, and adds what that tick did. It costs GitHub nothing.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// For exactly one reconciliation pass that exits: to step a fleet by hand and read what each pass did, or where a long-lived process cannot be kept. `run` is this in a loop.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// What one tick of `run` does: the startup probe, the rebuild, then every transition the working set calls for — on the remote, on GitHub and in orca. It does not wait for the workers it starts, and keeps nothing on this host a later tick needs. Like `plan` it makes no probe while one that passed on this host in the last hour stands, and says so on stderr (`afkconvoy plan --help`) — one that checked the worker launch commands this tick starts its workers with: a changed host kit is probed for. What is checked of each harness's command: `afkconvoy run --help`; the line each harness's worker is started with: [`afkconvoy help worker-launch`](help:worker-launch).
    ///
    /// A stopped candidate's worktree is removed on this pass when no process still has it as its working directory. A worktree a process is still in stays for a later tick. The candidate chain is not rewritten. `afkconvoy land --help`.
    ///
    /// Settling a claim removes that worker's worktree. Directories the gate left under it that the owner cannot write do not stop the removal: removal gives the owner write on the directories it has to delete, and clears no other mode bit. A removal that leaves the directory is not finished. A later tick removes it, including one git has already unregistered, and orca drops that failed removal once the directory is gone. A worktree a process is still working in stays until a later tick sees that process gone. A worktree that holds a commit the remote does not stays.
    ///
    /// ## What it answers
    ///
    /// {"did", "errors", "held", "frontier_remaining", "idle", "locks"} — and "held_back_by" when admission held back a worker it would have started
    ///
    /// - `did`: each step taken: {do, issue, outcome}. do is settle, forget, judge, watch, advance, escalate, end_patrol, patrol, end_cut, unlock, reclaim, dispatch, slow_gate, cut, board, stage_label or heartbeat; outcome is done, nudged, waiting, continued, restarted, nothing, lost, blocked, gone or failed. A step whose worker waits on its own background work says since when it was first seen to (epoch seconds): {do, issue, outcome, waiting_since}. A step on a patrol has a null issue and names it: {do, issue, patrol, outcome}. An `advance` that read a verdict and left the issue at a stage names it: {do, issue, outcome, stage} — `land` once the last stage passed to a landing turn. A step that escalated its issue says why, in a few words: {do, issue, outcome, escalated}. A `slow_gate` step also has `evidence`, naming the warning threshold, termination limit, host and recorded run durations; the tick log retains it and prints the same evidence when opening the issue. A `dispatch` that found an open blocker says which: {do, issue, outcome: "blocked", blocked_by}
    /// - `errors`: each step that failed: {do, issue, error}, with `patrol` as in `did` — and `see`, where a help topic explains what refused it: the command that prints the topic. A claimed issue the rebuild could not read is {do: "read", issue, error}: the rest of the tick is made, with it taken for open
    /// - `held`: the claims the instance holds after the pass, a patrol's run still going among them
    /// - `frontier_remaining`: the frontier issues it left undispatched
    /// - `idle`: whether the fleet instance is idle after the pass: it holds nothing, nothing of its is on the base branch, its frontier is empty, and no cut it started is going. `afkconvoy checkouts` answers it of each checkout's last tick
    /// - `locks`: the fleet locks that stood as it ran: the open issues carrying the lock label — none: []. `run` opens its line with them: "FLEET LOCK #7 stands: only it is worked"
    /// - `held_back_by`: each threshold of the host past which admission started no new worker, as the line says it: "the load 31.20 is past 28.00, 200% of its 14 cores". A slot was free and something waited for it; it stays on the frontier (`afkconvoy run --help`, ADMISSION)
    ///
    /// Errors: a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run); the repo fails the startup probe — its tracker is public, say; the config, GitHub or the remote cannot be read; the host kit cannot be read or lacks a required harness; the worker launch command of a harness the fleet could be asked for resolves to nothing, or does not take the flags the fleet adds for its role or worktree trust; `--instance` is not an instance id ([a-z0-9][a-z0-9-]{0,39}).
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required: `run` mints an id, a tick has to be given one. Give every tick of one fleet the same id — a new id holds no claims, and sees the old id's as another instance's.
    /// - In a bound repo the tick that reads a verdict ends the claim its stage was performed under (`advance`) and, where the verdict leads to a stage a role performs, starts that stage's worker into the slot it freed, under a claim of its own (`dispatch`, on the same issue): a `pass` to the next stage, a `reject` to an earlier one, a ruling that runs a stage again. That worker goes ahead of everything else the tick takes a slot for. Only a fleet over its concurrency has no slot for it: the issue is then on the frontier of the tick after, ahead of the issues no stage has declared on yet.
    /// - A dispatch reads the issue's blockers once more, just before its claim: the rebuild can be minutes older (settling runs in between), and a blocker added since holds the issue back. Nothing is claimed, no worker starts and no status board is shown; the step says `blocked`, the line `skipped #68 (blocked by #71)`, and the slot it would have taken goes to the next issue on the frontier, in the same tick. The rebuild's own requests are as many as ever.
    /// - An issue GitHub has no more is gone, like a closed one, and costs the tick nothing else. GitHub says one of three things of it: deleted, answered 410; transferred, its read redirected to the repo it moved to; no such issue, a 404 with no move — a number nobody issued. Only the first two are called deleted or transferred: a 404 with no move is no such issue. A claim of mine on it is let go (`forget`): released, its worktree removed when it holds nothing the remote does not — else kept, its worker's terminal closed — and nothing written on GitHub. A dispatch that finds it gone, before its claim or after, leaves no claim behind, says `gone` and takes no slot. The line says `released #231 (the issue was deleted)`, `(the issue was transferred to acme/other#12)` or `(there is no such issue)`, or `skipped #231 (…)` of a dispatch that found it gone with no claim to release; `afkconvoy doctor` names a claim on one whose fleet is not running, with its fix.
    /// - The last stage is different: the tick after a `pass` that leads to `land` starts a worker in the workflow's landing role, which gates the PR and lands in a turn; a `pass` to `land` with no open PR to land is handed to a human. A consult and a stage that runs again once a fleet lock closed are the tick after's too.
    /// - `run` does not sleep across that hop: after a tick that settled an issue or advanced one without starting what follows, and left a slot free, it starts the next tick at once (`next now` in its line), at most {const:BACK_TO_BACK} ticks back to back. Ticking by hand, the next `tick` is yours to run.
    /// - The step that ends a piece of work ends its worker too: no worker is left at its prompt once the tick has acted on what it declared. What becomes of the terminal and the worktree:
    ///
    ///   - `end_patrol`: The terminal is closed and the checkout removed: the patrol's next run starts in a new one, cut from the base branch as it then stands. A run that failed keeps its checkout, with the terminal closed — the issue that reports it gives the path — until the patrol is run again.
    ///   - advance, to a stage or a consult: The terminal is closed and the worktree kept: the next stage's worker is started in it, in a terminal of its own — a stage's by the same tick. The same where the issue then waits on a human.
    ///   - advance, to `done`: The terminal is closed and the worktree removed, with whatever was uncommitted in it. Its branch goes with it when it holds no commit past the base branch, and is kept when it does. The issue stays open until somebody closes it.
    ///   - an escalation: The terminal is closed and the worktree removed when the remote holds all of it — a clean tree, every commit pushed or on the base branch — or kept, when it holds what only this host has: the comment and the status board say what, the comment naming each commit the remote lacks. The PR and the branch are kept for the human. A park does the same, and a later dispatch recreates a removed worktree at the pushed tip.
    ///
    ///   An issue that lands loses its worktree, and its worker with it, as it is settled: the worker's own session, idle at its prompt after it landed, is ended at settle — its terminal closed and its processes confirmed gone — and then the worktree is removed. A worker still working, or any other process in the worktree, makes settle wait until a later tick finds it gone. After the worker health grace of {const:GRACE} seconds, doctor reports the wait with the worker's state and processes in the worktree.
    ///
    /// - A fleet fault — its worker start failed again, its harness's worktree trust gap — escalates at once, in a bound repo and an unbound one alike: no consult is asked and no consult claim is taken. The comment and `doctor`'s `waiting_on_human` say the fix is on the host. The operator fixes it and hands the issue back; another failed start escalates at once again. Every other hand-off goes to a consult first only where the stage names one; otherwise it escalates for the host's watch (`afkconvoy help humans`). This includes a worker gone silent: it had its brief, so the work may be the cause.
    /// - An `escalate` does not always hand the issue to a human. From a stage that names a consult (`escalate: { to, do }`), the tick that reads it ends the stage's claim, and the next one starts a worker in the consult's role — under a claim of its own, in one slot of the concurrency, in the same worktree. Its ruling (`afkconvoy rule`) runs the stage that asked again, in the tick that reads it, and counts as one more of the issue's rework in total; its own `escalate` goes to a human. A consult that stays silent is nudged once and then escalated, like any worker.
    /// - A `reject` does not always start the earlier stage again. Where it would pass one of the workflow's breakers — `max_rework`, `max_consecutive_rework` — the tick escalates the issue straight to a human instead, whatever consult the stage names; the comment names the breaker and lists every rework's stage and reason in order. `afkconvoy rehearse` trips them at the same verdict.
    /// - A verdict is not carried out when its stage left what it may not have: a stage marked `writes: false` whose branch holds a commit that was not there when its worker started has failed, and the issue escalates whatever it declared. The same holds for a consult, whatever its stage is marked: it writes no code. The guard (`afkconvoy guard --help`) refuses such a commit as it is made; this is the check of what a guard missed.
    /// - A patrol the repo names (`patrols`, `afkconvoy config --help`) is run by the tick that finds it due: its interval has passed since its last recorded run, and no run of it is still going. The tick claims the run on the remote — of several hosts one wins, and the rest say `lost` — starts a worker in the patrol's role, in a checkout of the base branch as it stands, linked to no issue, and records the run. A run takes one slot of the concurrency. A patrol is not run while its open issues have reached its `max_open`, nor while a fleet lock stands; one that missed runs is run once. `afkconvoy plan` says how each stands.
    /// - A patrol's worker declares no verdict: it declares its run over (`afkconvoy over --help`), with the line its brief ends with, and the next tick ends the run (`end_patrol`) and frees its slot. One that never does is ended once its worker is gone, or has been stopped for the grace period. Either way a run whose checkout ends on a commit not on the base branch, has working-tree changes, or opened a PR has failed (`afkconvoy help patrols`): the tick opens an issue that says which, with the team's escalate label, and touches no other issue. The patrol is not run again until a human closes it. An operator reading orca sees a patrol's checkout only while its run is going, or has failed.
    /// - A tick that finds the gate slow opens one issue that says so (`slow_gate`). The gate is slow when, on one host, the last {const:SLOW_GATE_LANDINGS} landings on the base branch that have a recorded run of the config's `gate_command` with a time all exceeded the fixed `gate_budget_seconds` when declared, or took more than {const:SLOW_GATE_PERCENT}% of the termination gate limit, `gate_seconds`, otherwise. The issue carries the config's `ready_label`, so the fleet works it, and the label `slow-gate`, which is how it is found again. Its body gives the warning threshold, the termination limit, the host and the {const:SLOW_GATE_LANDINGS} runs, each with its commit and its seconds, and says what is not a fix: deleting or skipping tests, and raising `gate_seconds` or `gate_budget_seconds`. `afkconvoy plan` shows the same evidence, as `slow_gate`.
    /// - It opens no other while an issue carrying `slow-gate` is open, on any host, and the opening is claimed on the remote as a patrol's run is: of several hosts that found the gate slow one opens the issue, and the rest say `lost`. After that issue is closed only the landings made since count: the gate is slow again when {const:SLOW_GATE_LANDINGS} of those all exceeded the same warning threshold. It takes no slot of the concurrency.
    /// - In a repo that turns cutting on, a pass starts a cut (`cut`) when idle with landed commits past the newest version tag, or while busy once the oldest landing reaches the repo's age bound; an untagged remote's declared `initial` version starts only when idle. It claims the cut on the remote — of several hosts one wins, and the rest say `lost` — picks the version from the tags as they are then, and starts `afkconvoy cut` in a checkout of its own (`afkconvoy cut --help`). Its line says `fleet idle` or `fleet busy`. A cut takes no slot of the concurrency; while it goes the fleet keeps its heartbeat, and dispatches what comes as ever. The tick that finds it over ends it (`end_cut`): its line says what it did with each step's seconds, or where it failed and the issue that says so. A patch superseded by a minor landing ends without failure and starts the minor in that pass. A fleet lock or an open cut failure holds cuts back; admission holds only a cut before its age bound.
    /// - A ruling makes its stage run again with the existing rework semantics. Lasting rules are amended through ordinary work in their owning ADR, team or role rules, preserving source decisions. No proposal worker is started.
    /// - Upgrade: a retired team key `precedents` is refused with migration instructions. Before upgrading a configured team, migrate its effective rules and source links into their owning documents and remove that key. Existing proposal claims take no concurrency slot and cannot keep a fleet from idle. A tick stops only its verified local owned worker, or releases a stale remote claim without stopping any worker. Worktrees, branches, PRs and ruling comments are retained. Once per checkout/team binding it reads legacy open PRs from the team's own branches and comments their links and disposition for ordinary content migration; it neither closes nor merges them. A failed retirement retries while normal issues continue.
    ///
    /// - With a fleet lock standing a tick works lock issues only (`afkconvoy lock --help`). It dispatches nothing else, runs no patrol, takes no stale claim of another issue, hands no other issue to a human, and leaves alone every claim of its own on another issue whose PR is not on the base branch: no judge, no watch, no advance, no nudge. What those workers declared is acted on by the first tick after the last lock issue closed. Their claims take no slot meanwhile, so `held` may pass the concurrency by as many; the line of `run` shows them apart, as `left alone`.
    /// - A claim whose worker declared a verdict under it takes no slot either: the slot is free the tick it is read, though the claim is held until the verdict is acted on — once its worker has been quiet the grace period, as ever. A verdict on the issue from before the claim was created is not one declared under it. So `held` may pass the concurrency by as many, and the line of `run` shows them apart, as `taking no slot`. Where acting on the verdict starts a worker under the claim again — every blocker closed, a gone worker continued — that start waits for a free slot as any dispatch does, and is made by a later tick while none is free.
    /// - A landing claim — its PR open and not on the base branch — takes a slot while its worker is changing the tree: resolving a `conflict` or `speculative_conflict`, fixing a `gate_red`, or still preparing the branch for the turn. While its worker is gone or idle, or its turn is being asked for, or the last turn answered a wait (`afkconvoy land --help`), it takes none, and is still held until the PR lands. Its worker stops on a wait; the tick asks for the turn again itself (`asked for a turn again for #7` in its line), in the worktree, with no harness started. If that turn answers `conflict`, `speculative_conflict` or `gate_red`, the next tick hands its worker the answer, including the files or failing output, the ending's instructions and the worktree's full `convoy-turn.log` path. Its line says `told its turn's answer to #7`. A worker gone is continued onto its land step with that answer in its brief. Giving the worker this answer waits for a free slot; once the worker takes it, the claim takes that slot until it asks for a turn again, even while its harness still reports idle. Changes the fleet's turn made in the worktree count as no worker sign of life.
    /// - What a closed lock explains is run again, not carried out. In a bound repo, a stage whose own worker declared `escalate` before the lock closed and was not acted on since — and one whose worker stayed silent past a nudge sent before it closed — is not handed to its consult or to a human: the fleet leaves a stage record (`verdict=lock-closed by=fleet`), the claim is released (`advance`, `done`), and the next tick starts a worker on that stage, whose brief says a lock stood. It is no rework. A `pass`, a `blocked` and an `already-satisfied` stand as they are. What landed is still settled.
    /// - A step that failed is not a failed command: the exit is 0 and the failure is in `errors`. Read `errors`, not the exit code.
    /// - A read of GitHub that failed in transit — an answer cut short, a 5xx, a connection reset, a timeout — is made again, twice more and a second apart, before anything is said. One that still fails is `GitHub could not be reached: <the call>: <what gh said>`. A 404 or a 403 is GitHub's answer, and is not asked for again.
    /// - A role whose repo GitHub could not be reached for is no fault of the team, and stops only the step that would start a worker in it: an issue at a stage another role performs is advanced, started and labelled as ever.
    /// - It does not take the claims stamped with this host: only `run` does that, at its start. A tick takes another instance's claims only once they are stale.
    /// - Do not tick as an instance whose `run` is alive: both would act on the same claims.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Tick {
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
        #[command(flatten)]
        at: At,
    },
    /// Run the gate in this worktree and record a green run
    ///
    /// ## When to run it
    ///
    /// Run `gate` on a committed change before workflow review or for diagnosis. Run `gate --land` once an issue PR is open to take the same landing turn as `land --issue <n>`. Where the repo splits its gate in two, run `gate --fast` before opening or updating a PR, and `gate --slow` to run the rest alone. A fast gate waits on no slow gate or landing turn: it takes the host's fast lane (`afkconvoy exclusive --help`).
    ///
    /// ## Who runs it
    ///
    /// A worker in its own worktree, or the operator diagnosing a red gate.
    ///
    /// Before running any gate, a worker's AFKCONVOY_ISSUE claim is read live. If another host holds it, the command refuses: `the claim on #N is held by instance X on host H since <ts>: this worker's work was taken; stop`. Nothing runs or is recorded. The host is the name its host settings give it, regardless of --host; a restart on the same host keeps its workers.
    ///
    /// ## What it reads and writes
    ///
    /// The config's gate_command runs on this worktree, with its log streaming to stderr and its gate limit, gate_seconds (default {const:GATE_SECONDS}). A green run on the committed tree is recorded with the tree, command, host and elapsed seconds. The record has the tested commit as its parent, so its push sends only objects the remote lacks; parentless records already on the remote are still found by tree and command. A dirty worktree or artifacts the gate fails to ignore cannot produce a record. Its first green run records the fresh build for worker disk observations.
    ///
    /// With --fast or --slow, the fast gate's or the slow gate's command runs alone, as the ```yaml gates block of the config declares it (`afkconvoy config --help`), under that gate's own limit, `seconds`. A run here is put on record as neither, and neither stands in for the whole gate: a landing turn runs the fast gate per candidate and the slow gate per round, and records the whole gate only from both on one tree (`afkconvoy land --help`). A run past that gate's `budget_seconds` is green and says it is slow. A repo that declares no gates block answers an error.
    ///
    /// With --land, open the PR first. One turn makes the PR's merge commit on the current base tip, gates that commit, records green and lands it as a fast-forward. An unrecorded base tip is gated alone first; worker turns wait while it is red and the fleet dispatches its repair. Only the fleet-lock issue's repair may take a turn on a red base. `land --help` describes every outcome.
    ///
    /// ## What it answers
    ///
    /// {"status", "recorded", "detail", "exit_code", "timed_out", "seconds", "command", "head"}. status is green or red; recorded is true only for a green run of the committed tree written to the remote. detail says what to do next. seconds times the command. A run that reaches gate_seconds is red with exit_code {const:TIMED_OUT_CODE} and timed_out true. uncommitted lists at most {const:STRAYS_LISTED} paths when green could not certify a committed tree; not_recorded says why the record could not be written, including a push ended after stalling for 30 seconds. locks names standing fleet locks on a red run. termination is null unless a signal ended the command outside its limit; then {pid, signal, core_dumped}, with exit_code -1 and timed_out false. Its pid also names its process group. The sender is unknown and repository checks may not have started; inspect harness/tool cancellation and host process logs using that evidence. A shell exit such as 137 alone does not prove a signal. With --land the answer also carries the landing outcome, PR and base tip merged. With --fast or --slow it is {"gate", "status", "recorded", "detail", "exit_code", "timed_out", "seconds", "command", "limit_seconds", "budget_seconds", "over_budget", "termination", "excerpt"}: gate is fast or slow, recorded is always false, over_budget says whether seconds passed budget_seconds, and excerpt is the end of a red run's log.
    ///
    /// Errors are {"error": …} with non-zero exit: an unreadable config, an uncommitted landing change, a missing base branch or no open issue PR for this branch. Red is an answer with exit 0; read status and recorded.
    ///
    /// ## What trips you up
    ///
    /// - Commit every change before gating. A further commit needs another run.
    /// - A standalone gate lands nothing; a workflow's landing worker lands after its final stage.
    /// - Resolve every conflict and commit the merge before asking for another turn.
    /// - The gate limit is gate_seconds in the config, not a flag.
    /// - A green fast gate is not a green gate: the slow gate's tests have not run. Only the whole gate, at landing, is on record, from a round's fast and slow runs on one tree or a run of gate_command.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// After the PR is open and all changes are committed: `afkconvoy land <n>` or `afkconvoy land --issue <n>`. A watch uses `land --watch` for its own PR. Cuts take the same turn automatically.
    ///
    /// A bound repo's landing worker can stop with `afkconvoy verdict blocked --issue <n> --stage land --blocked-by <issues> --text-file <file>` when another issue must be fixed first. The fleet parks its claim on open blockers and returns it to Landing when they all close. `verdict escalate --issue <n> --stage land --text-file <file>` hands it to a human, with the PR and branch kept; `afkconvoy rule` returns the issue to Landing after the cause is resolved.
    ///
    /// ## Who runs it
    ///
    /// An issue's worker in its own worktree; in a bound workflow its landing worker after the last stage passes. A watch uses its own linked checkout named {const:WATCH_CHECKOUT}, with an optional numeric suffix. Other watch checkout names are refused.
    ///
    /// The issue's claim is checked live first. If another host holds it, the command refuses: `the claim on #N is held by instance X on host H since <ts>: this worker's work was taken; stop`. No candidate, gate record or landing is written. Ownership uses the host's name from its host settings, regardless of --host; a new instance on the same host keeps its workers.
    ///
    /// ## What it reads and writes
    ///
    /// A task uses its pinned target branch (`afkconvoy target --help`), or resolves the configured base when it has no target. Its PR must actually name that base; a wrong base, missing branch or unreadable target refuses without falling back. Each repo and target has its own candidate chain and landed commits. Watches and cuts retain the configured base.
    ///
    /// {const:REMOTE_READS}
    ///
    /// A turn preserves the PR's original head and atomically assigns a candidate merged onto the certified base or its preceding candidate. Each candidate must gate that exact commit in its own worktree, and its gate run is recorded green against it. A gate-record push that does not end within 30 seconds is killed with its process group: `not_recorded` says so, the candidate is not green, and queued work can take its lane. With free worker lanes, candidates of the same repo validate concurrently; green successors wait for their exact predecessors, then push that commit to the target branch as a fast-forward without another gate. A candidate holds a worker lane only while it builds and runs its gate; the window has no bound of its own, but for barriers and unconfirmed cancellation. A light gate keeps its repo lane. Committed source changes are pushed to the PR first; candidate merges never move that source branch. The merge message names the PR and issue or watch. Landing closes the issue; the tick settles its claim and source worktree. Once the landing is published, a failure to close the issue or write its work account does not turn the answer into an error: it is `merged`, with the failure in `settle_error`, and the tick settles the issue; a close GitHub refuses while it closes the issue itself, as a merged PR that says `Closes` it, is a close.
    ///
    /// Where the candidate's commit declares a ```yaml gates block (`afkconvoy config --help`), the candidate runs the fast gate, not `gate_command`, and waits fast-green for a round. A round is as big as the PRs that are ready: a turn that asks to land is on the candidate record as pending while it waits for a worker lane, and a fast-green candidate frees its lane. When no turn is pending and every candidate of the chain is fast-green, the turn of the chain's last candidate, the round's tip, takes a slow lane and runs the slow gate once on its commit, in its worktree; a PR that asks meanwhile waits for the next round. Green, the whole gate is put on record for the tip's tree from its fast and slow runs, and one atomic push lands every candidate up to the tip; each of their turns answers `merged` and closes its own issue. Red is not split to find whose change failed: the tip's turn answers `gate_red` with `gate` slow and `round`, the round's PRs and issues, and merges the round's tip into its branch, so its worker fixes the round from those issues and the failing tests and asks again; every other candidate of the round answers `round_red` and stops, its change carried by the tip's PR, and its issue is settled from the merge commits on the target branch once that lands. A merge onto the candidate ahead that conflicts is left in the worker's worktree to resolve, as `speculative_conflict`: the worker's branch then carries that candidate. If a fast-green candidate's owner exits on this host after its gate processes are confirmed stopped, the next turn of the same PR and original head adopts it with its recorded evidence and successors intact; a running gate is never adopted.
    ///
    /// Candidate worktrees are kept in `candidates/<repo-key>/<candidate-id>` under the host's cache directory (`afkconvoy help cache`), outside the checkout: a gate's tools discover the candidate's own configuration without finding the checkout's above it. The repo key hashes the canonical common git directory, so all worktrees of one repository share a root and separate clones have separate roots.
    /// A cache configured inside any checkout is refused before reserving a turn or assigning a candidate. Choose a cache outside every checkout (`afkconvoy help cache`) and run `afkconvoy land` again.
    ///
    /// Once a candidate has stopped — landed, red, or cancelled with its processes confirmed gone — its worktree is removed. Removal accepts only that repo's candidate root and the earlier `convoy-candidates` root in its common git directory, so a candidate left by an older version is removed too. The candidate chain keeps the gate evidence: the phase, the cause and the gate excerpt, and a recorded gate run stays on the remote. A directory left behind after git dropped the registration is removed too, including when a directory inside it is not writable by its owner. A candidate that is building, validating, green and waiting on its predecessor or for a round, slow-gating, or cancelling, keeps its worktree. A worktree a process still has as its working directory stays until that process is gone, and a later tick removes it. The issue's, watch's or cut's own worktree is left, and the candidate chain is not rewritten.
    ///
    /// The remote candidate record holds original head, predecessor, commit, tree, gate declaration, owner, process group, validation and cancellation. Publication rechecks validity, the PR's actual base, head, predecessor landing, issue blockers, fleet locks and the green record, then atomically fast-forwards the exact tested candidate, or a round's tip with every candidate it lands, and its state. No force push or extra merge is used; a round's slow gate is the one gate of more than one candidate. Known candidate advances preserve successors; an outside base move or changed PR head invalidates the affected suffix. Cuts, watches, fleet-lock repairs and base certification drain the candidate window and run as serial barriers.
    ///
    /// Failure invalidates the dependent suffix before terminating its running gates and children. Their lanes remain occupied until exit is confirmed. Cancellation does not record a dependent worker's gate as failed or increase its rework count. A later turn rebuilds from preserved original heads and the last landed base, respecting issue blockers. Gate startup waits until its group is recorded; inherited kernel lane leases prevent duplicate capacity after runner or owner death. Interrupted cancellation is completed on the owning host. Another host's processes are never signalled from here, but that host is dead once nothing of it said it was alive within the lease, {const:LEASE_MINUTES} minutes: none of its turns stamped the candidate record, which each does at least every {const:BEAT_EVERY_MINUTES} minutes while it waits, and no fleet instance of it beat on this remote. The next turn on another host takes over what a dead host holds on the chain: its candidates are cancelled `owner_died` with their suffix and no longer hold the chain, unconfirmed, and its pending turns and barriers are dropped. A turn of that host that comes back learns it at its next look at the chain: it stops its own gate and answers `cancelled`, cause `owner_died`; one still waiting for a candidate takes a place again, at the back. A successful base push records its landed prefix atomically, and its merge commit lets recovery settle the issue after a crash.
    ///
    /// Publication uses an explicit atomic Git receive-pack transaction so an unchanged source head is verified alongside the base and candidate record. Normal git push omits that unchanged-ref condition. Local remotes use git; SSH origins use their existing SSH login. A GitHub HTTPS origin publishes through GitHub's all-or-none `updateRefs` after a plain `git push` carries the objects: `gh` must be logged in with write access and git's credential helper must give that login (`gh auth setup-git`). No token is read or handled. Other HTTP transports require an SSH origin. Failed authentication or missing atomic capability refuses before any ref update is sent.
    ///
    /// A watch finds its branch's PR through REST, and every turn checks fleet locks through REST. Exhausted GraphQL quota alone therefore does not prevent a watch from landing its repair; an unreadable lock listing still refuses the turn.
    ///
    /// An unrecorded base tip is gated alone in a separate checkout using its own gate declaration. While it is red, ordinary turns wait outside worker lanes. The fleet opens a dispatchable fleet-lock issue for its repair. That issue's worker alone may take a turn on the red tip, and must still gate its complete merge green before landing. A standing fleet lock allows only its issues to land.
    ///
    /// ## What it answers
    ///
    /// {"outcome", "pr", "commit", "merged", "candidate", "predecessor", "target_branch", "status", "recorded", "detail", …}. issue or watch identifies whose PR it is. merged and predecessor name the assigned predecessor; commit is the exact candidate landed. status and recorded describe the gate, with head, command, exit_code, timed_out, seconds and excerpt. doctor and the panel expose the same candidate lifecycle with validation, predecessor, waiting and cancellation cause.
    ///
    /// {const:LAND_ENDINGS}
    ///
    /// Errors are {"error": …} with non-zero exit: no open PR, unreadable config or remote, uncommitted tracked changes, an invalid watch checkout or a green base record that could not be written. detail names what to fix.
    ///
    /// ## What trips you up
    ///
    /// - The turn owns its lane through landing. Never run gh pr merge or push the base branch yourself.
    /// - A rejected push ends the turn; no untested merge is silently retried.
    /// - A standalone gate before review lands nothing. Only the landing worker of a bound workflow takes the turn after review.
    /// - A conflict remains in progress. Commit its resolution before asking again.
    #[command(
        next_help_heading = WORKER,
        verbatim_doc_comment,
        override_usage = "afkconvoy land <n> | afkconvoy land --issue <n>"
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
    ///
    /// ## When to run it
    ///
    /// After the operator has decided the current question, or asked to put it aside, using the exact line in the asking session's brief:
    ///
    /// ```
    /// afkconvoy asked --question <episode-id> --text-file <report.json>
    /// ```
    ///
    /// A settled report is {"outcome":"settled","url":"<comment-url>"}. Its URL is the issue comment holding the operator's quoted words and their time; the asking session's own words establish no human intent. A put-aside report is {"outcome":"aside"} and has no URL. Put-aside questions stay pending and are listed when the operator asks.
    ///
    /// ## Who runs it
    ///
    /// The resident asking session, after recording the operator's actual decision on the issue. The operator may repeat its exact command if the receipt was lost.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the current delivered question on this host and atomically keeps the report in the host cache. It needs no repo or GitHub login. Only the current, delivered, unwithdrawn question may report. The next question is delivered only after this report. The same report may be submitted again; a different one cannot replace it.
    ///
    /// ## What it answers
    ///
    /// {question, reported: true, report}: the question's episode identity and its record. Errors name the current question's brief and the JSON to fix.
    ///
    /// ## What trips you up
    ///
    /// Settling requires the link to the operator's quoted words and time, not a summary written by the Agent. Printed output is no report. A recovered question is withdrawn by the host runner and cannot be settled afterwards.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Asked {
        /// The question's episode id from its brief
        #[arg(long)]
        question: String,
        /// The JSON settlement or put-aside report
        #[arg(long)]
        text_file: PathBuf,
    },
    /// Record what this host's watch did and complete its round (a watch's command)
    ///
    /// ## When to run it
    ///
    /// As the watch's last step, using the exact line at the end of its brief:
    ///
    /// ```
    /// afkconvoy watched --watch <id> --text-file <report.json>
    /// ```
    ///
    /// Write a JSON object with all five keys: `held_back`, `did`, `verified` and `pending` are lists of nonempty strings; `human` is a list of {condition, question, answer}, all nonempty strings. `condition` names an exact condition id from the round's look, listed in its brief, and is checked against that look. It may be omitted only for a look with exactly one actionable condition; ambiguous or unknown conditions are refused. Question and answer words never identify a condition. Use [] where nothing belongs. `held_back` says what held work back; `did` each action and its evidence, including repo, PR, cut and version; `verified` what was checked and what it showed; `human` each decision left alone and its exact answer command; `pending` everything unfinished. Printed terminal output is no report.
    ///
    /// ## Who runs it
    ///
    /// The host's watch, in its watch directory, as the last step of each round in its resident interactive session. The operator may repeat the watch's exact command if its receipt was lost.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the running watch on this host and the report file, and atomically keeps the report in the host cache. It needs no repo or GitHub login. The same report may be submitted again; a different report cannot replace it. A missing part, unknown key, empty entry, wrong watch or expired hour is refused with {error} and nonzero exit; fix the JSON or use the running watch's brief.
    ///
    /// Once the host runner reads the report, it appends one entry for the round to the daybook (`afkconvoy daybook --help`), of session kind `watch`: what held work back under `Why`; the actions, what was verified and what was left under `Did`; and the issues and PRs the report links under `Links`. Each human question still goes to the asking session; the entry notes only that it was raised, with its link, and asks nothing. The entry is written later and retried until it is: it never holds the round, or the next one, back; one not written is a `doctor` finding.
    ///
    /// Recording completes only the named round and keeps the healthy resident Agent and Terminal available for later rounds. It does not finish queued work or another active round. `doctor.host.watch` and later briefs show the report; an ended watch with no report says `reported: false`.
    ///
    /// ## What it answers
    ///
    /// {watch, reported: true, report}: the watch id and its complete record. Reporting does not claim recovery beyond the evidence in the report.
    ///
    /// ## What trips you up
    ///
    /// Use the id and report path from this watch's brief. Include all five parts, even those with empty lists. The report cannot be replaced once recorded, and a successful harness exit without this command is shown as not reported.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Watched {
        /// The watch id from its brief
        #[arg(long)]
        watch: String,
        /// The JSON report with all five parts
        #[arg(long)]
        text_file: PathBuf,
    },
    /// Declare the verdict that ends a stage, with the text it produced (a worker's command, and a human's for a stage nobody holds)
    ///
    /// ## When to run it
    ///
    /// Once, as the last thing a stage does: it declares the verdict that ends the stage, with the text the stage produced. A worker's brief gives the line with `--issue` and `--stage` filled in; add the verdict and the text:
    ///
    /// ```
    /// afkconvoy verdict pass --issue 7 --stage build --text-file /tmp/build.md
    /// ```
    ///
    /// The five verdicts:
    ///
    /// - `pass`: the stage did its work: the issue goes where the workflow says
    /// - `reject`: the work of an earlier stage is turned down; the text says why. Only a stage whose workflow gives it a `reject` may declare it
    /// - `blocked`: the issue waits on other issues: name them with `--blocked-by`. A stage that splits its issue declares it on the issues it opened and took in (`afkconvoy intake --help`), and runs again when they have all closed. So does a stage whose gate is red for a defect outside its issue, on the defect's issue: one it opened and took in, or the open one that already recorded it
    /// - `already-satisfied`: the issue needs no change: it is already done
    /// - `escalate`: the stage cannot decide; the text says what has to be decided
    ///
    /// Opening a PR declares nothing: a stage that wrote one still declares `pass` — after `afkconvoy gate` says green on what it pushed, as its brief tells it.
    ///
    /// At Landing, a worker declares with `--stage land`: `blocked` names open issues the fleet can wait for and parks the claim, keeping work as `afkconvoy park` does. Once every blocker closes it returns to Landing, without repeating a stage. `escalate` hands it to a human with an answer line, keeping its PR and branch; a human's `afkconvoy rule` returns the issue to Landing. Other worker verdicts do not advance Landing.
    ///
    /// A consult also declares `close` or `park`, with its reason in the text. Close ends the issue; park leaves it open for new evidence, off the frontier until explicitly brought back (see `afkconvoy help humans`). Neither adds rework or resets its counts. `escalate` lets through to a human what the consult was asked to rule on. Its ruling uses `afkconvoy rule --help`.
    ///
    /// A human runs it too, for a stage a workflow gives them (`by: human`): the issue waits there, unclaimed, until they declare. `afkconvoy show --issue 7` names the stage:
    ///
    /// ```
    /// afkconvoy verdict pass --issue 7 --stage sign-off --text "Approved."
    /// ```
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree: it starts in its role's directory, so the line its brief gives changes to the worktree first. The fleet put the commits of its role and of its team, and the harness it is a session of, in its environment, and the command reads them from there.
    ///
    /// Or a human, in a checkout of the bound repo, with neither commit named and none in the environment: that is what makes the record a human's.
    ///
    /// The issue's claim is checked live first. If another host holds it, the command refuses: `the claim on #N is held by instance X on host H since <ts>: this worker's work was taken; stop`. No stage record is written. Ownership uses the host's name from its host settings; a new instance on the same host keeps its workers.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the claim on the issue off the remote and the issue's comments, and writes one comment on the issue: the stage record. The record opens with a marker that carries the stage, the verdict, the claim it was declared under, the commits of the role and of the team and the harness the worker ran on — and nothing of how it was started — and the text follows it. Nothing but this command writes that format.
    ///
    /// A human's record is written under no claim, and only on an issue nobody holds. It says a human left it and carries no commit. Unlike a worker's it is checked first, against the team the config names and the issue's records: the issue must be at the stage named, and the verdict one that may be declared there.
    ///
    /// The text is `--text`, or the contents of `--text-file`: one of the two, and not empty. Then it wakes the fleet, as `afkconvoy wake` does.
    ///
    /// It moves nothing. Where the issue goes is read by the fleet off its records and its workflow at the next tick: see `afkconvoy show --help`.
    ///
    /// ## What it answers
    ///
    /// {"issue", "stage", "verdict", "claim", "declared", "woke"} {"issue", "stage", "verdict", "by": "human", "declared": true, "woke"} — a human's
    ///
    /// - declared: true: the record was written
    /// - declared: false: this stage already has a record under this claim: nothing was written, and `standing` is the verdict that record declared
    /// - `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers
    ///
    /// Errors, as {"error": …} with a non-zero exit: a word that is none of the five verdicts; no text, or an empty one; `--blocked-by` with another verdict than `blocked`; a worker's on an issue that is not claimed; one of `--role-commit` and `--team-commit` (or AFKCONVOY_ROLE_COMMIT and AFKCONVOY_TEAM_COMMIT) without the other; a `--harness` (or AFKCONVOY_HARNESS) that names no harness, or one with no commit; "is not in a git checkout", or git's own words for a checkout it refuses.
    ///
    /// A human's is refused, with nothing written, when the issue "is claimed" — its stage is its worker's, and a worker's record names both commits; when it is at another stage, or has left its stages; when the verdict "cannot be declared" there; with `--blocked-by`; and when the issue carries no one flow label.
    ///
    /// ## What trips you up
    ///
    /// - One stage declares once under one claim. A second run changes nothing, whatever it says: the first record stands.
    /// - A record is a verdict only for the stage the claim was dispatched to. One that names another stage is written and counts for nothing: the worker is then treated as having declared no verdict.
    /// - It does not check that the verdict is one the stage may declare. A `reject` from a stage whose workflow gives it none is no valid verdict, and the issue escalates.
    /// - A ruling is not a verdict, and is not declared here: see `afkconvoy rule --help`.
    /// - An `escalate` starts a consult only where the stage names one. Otherwise it escalates for the host's watch to answer. A named consult rules — and the stage runs again under the ruling — or declares `escalate` itself. That one always goes to a human: the chain is one role long.
    /// - Whether a record is a consult's is read from AFKCONVOY_CONSULT, which the fleet put in the worker's environment (or `--consult`). Anything a consult declares but `escalate`, `close` or `park` is no valid verdict, and the issue escalates.
    /// - With no commit named a record is a human's. A worker whose environment lost its commits is refused, as its issue is claimed.
    /// - A human's `already-satisfied` ends the workflow and closes nothing, and a human's `blocked` parks nothing: no claim is there to do either.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
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
        /// The issues a `blocked` verdict waits on, as numbers separated by commas
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<u64>,
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
    ///
    /// ## When to run it
    ///
    /// When an issue of a bound repo was escalated and you have decided what it asked: the issue carries the team's escalate label in place of its flow label, and the comment above the label says why. Answer it with the ruling:
    ///
    /// ```
    /// afkconvoy rule 7 --text "Keep the v1 schema; migrate in a later issue."
    /// ```
    ///
    /// The issue's number is an argument, or `--issue 7`: one of the two. The comment of the escalation ends with this line, the number filled in, and `afkconvoy doctor` lists it for every issue that waits (`waiting_on_human`).
    ///
    /// The stage the issue stopped at runs again under the ruling, and both rework counts start over from it. `afkconvoy show --issue 7` says which stage that is before you rule. To put the issue at another stage instead, see `afkconvoy send --help`.
    ///
    /// An issue escalated at Landing returns to Landing under the ruling: its existing PR is kept and no stage runs again. Its answer has `stage: land`.
    ///
    /// Or as a consult: the issue was at a stage that names one (`escalate: { to, do }` in its workflow), and would have gone to a human — the stage declared `escalate`, a breaker tripped, it failed, or its workflow changed — so the fleet started you in that role to answer it first. The stage comes from your environment:
    ///
    /// ```
    /// afkconvoy rule --issue 7 --text-file /tmp/ruling.md
    /// ```
    ///
    /// The stage runs again under your ruling. What is not yours to rule on you let through to a human instead, with `afkconvoy verdict escalate` — your brief gives that line too.
    ///
    /// ## Who runs it
    ///
    /// The operator, or whichever human the issue was handed to: in a checkout of the bound repo, with no commit named and none in the environment.
    ///
    /// Or a consult, in the issue's worktree: the fleet put the commits of its role and of its team in its environment, and those are what make the ruling a consult's; the harness it is a session of is there too, and its record names it. No other worker runs it: a stage does not rule on itself.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the team the config names, the claims on the remote, and the issue's labels and comments. The workflow is the one the issue's flow label names; with the label swapped out, the one its escalation's comment says it left.
    ///
    /// It writes one comment on the issue — a stage record that carries the ruling, says a human left it and names the stage — then puts the flow label back on and takes the team's `escalate_label` off, and wakes the fleet. The record is written first: once the flow label is back the issue is the fleet's again, and its next tick starts a worker on the stage, with the ruling in its brief.
    ///
    /// A consult's ruling is one comment too, and nothing else: a stage record that says the stage's consult left it, under the claim the consult holds on the issue, with the two commits. `--stage` is AFKCONVOY_STAGE where not given. It moves no label — the issue never left the fleet — and the next tick ends the consult's claim; the one after starts the stage that asked, with the ruling in its brief. It is checked no more than a worker's verdict is.
    ///
    /// The ruling is `--text`, or the contents of `--text-file`: one of the two, not empty.
    ///
    /// ## What it answers
    ///
    /// {"issue", "workflow", "stage", "ruled": true, "relabelled", "woke"} {"issue", "stage", "verdict": "ruling", "claim", "declared", "woke"} — a consult's, as `afkconvoy verdict` answers a worker
    ///
    /// - `stage`: the stage that runs again under the ruling
    /// - `relabelled`: whether a label was moved; false when the issue already carried its flow label and no escalate label
    /// - `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers
    ///
    /// Errors, as {"error": …} with a non-zero exit, with nothing written: the issue "is claimed" — its escalation is still under way, or a worker is at it; it "is not escalated" — its records put it at a stage and it carries its flow label; it "cannot be ruled on" — it has left the stages of its workflow, or no flow label and no escalation says which workflow it follows; no text, or an empty one; the repo names no team; and every error of `afkconvoy check`. A consult's: `--stage` is missing, or given with no commits; the issue "is not claimed".
    ///
    /// ## What trips you up
    ///
    /// - A ruling runs the same stage again. It does not decide the stage's verdict: the worker started under it does.
    /// - An issue escalated because its worker went silent, or its gate stayed red, is at the stage that worker was performing: a ruling starts that stage anew.
    /// - An escalation with no position cannot be ruled on: no flow label and no escalation comment says which workflow it follows. The error gives the line to put a flow label on it, as `afkconvoy doctor` does. With one workflow the label is filled in; with several, choose from the flow labels listed. This starts it at that workflow's first stage (`afkconvoy help humans`).
    /// - Moving the labels by hand hands the issue back with no ruling on record: an issue its records show as escalating is escalated again at the next tick.
    /// - Run twice, it leaves two rulings. Both count: the stage runs again once. A consult's second changes nothing: one claim declares once.
    /// - A human's ruling starts both rework counts over. A consult's does not. On an `escalate` it is one more of the issue's rework in total, and one that would pass `max_rework` goes to a human instead; on anything else it counts nothing, and the same hand-off again goes to a human.
    /// - A consult writes no code. The guard refuses its commits and pushes, and a commit it left on the branch all the same is a failure: the issue escalates, whatever it ruled.
    #[command(next_help_heading = HUMAN, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// To put an issue of a bound repo at a stage of your choosing, whatever its records say: back to planning after a review that missed the point, past a stage you have done by hand, or out of an escalation to another stage than the one that asked.
    ///
    /// ```
    /// afkconvoy send --issue 7 --stage plan --text "The approach changed: see #12."
    /// ```
    ///
    /// The issue is then at that stage, to be performed, with both rework counts started over. To answer an escalated issue where it stopped, `afkconvoy rule` is the command.
    ///
    /// ## Who runs it
    ///
    /// The operator, or whichever human steers the issue: in a checkout of the bound repo. Or a consult answering in a human's place, under its claim, as with `afkconvoy rule`; its send starts no count over. No other worker runs it: where an issue goes is its workflow's to say, or a human's.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the team the config names, the claims on the remote, and the issue's labels and comments. The workflow is the one the issue's flow label names; for an escalated issue, the one its escalation's comment says it left.
    ///
    /// It writes one comment on the issue — a stage record that names the stage, says a human sent the issue there and carries the text — and wakes the fleet. An escalated issue also gets its flow label back and loses the team's `escalate_label`, after the record is written. Nothing else moves: the next tick reads the issue's position off its records, starts a worker on the stage if a role performs it, and waits for a human's verdict if the stage is `by: human`.
    ///
    /// The text is `--text`, or the contents of `--text-file`: one of the two, not empty. The worker started on the stage is shown it in its brief.
    ///
    /// ## What it answers
    ///
    /// {"issue", "workflow", "stage", "sent": true, "relabelled", "woke"}
    ///
    /// - `relabelled`: whether a label was moved: true for an issue that was escalated
    /// - `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers
    ///
    /// Errors, as {"error": …} with a non-zero exit, with nothing written: the workflow "has no stage" of that name — the error lists the ones it has; the issue "is claimed" — a worker is at a stage of it; it "cannot be sent to a stage" — no flow label and no escalation says which workflow it follows; no text, or an empty one; the repo names no team; and every error of `afkconvoy check`.
    ///
    /// ## What trips you up
    ///
    /// - It is refused while the issue is claimed: a claim is dispatched to one stage, and its worker would go on performing it. Wait for the stage to declare, or hand the claim to a human with `afkconvoy escalate` first.
    /// - It reopens nothing and closes nothing. An issue sent back from an ended workflow must be open to be worked.
    /// - The stage is named as its workflow's file names it. `afkconvoy show` draws them.
    /// - An escalation with no position cannot be sent to a stage before it follows a workflow. The error gives the same line as `afkconvoy doctor` to put a flow label on it (`afkconvoy help humans`).
    /// - Run twice, it leaves two records; the issue is at the stage either way.
    #[command(next_help_heading = HUMAN, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// When a worker has an issue the fleet is to work with no human labelling it: one a patrol opened, or one a stage split off its own. It is the intake a worker makes — a human takes an issue in by putting its flow label on, on GitHub.
    ///
    /// ```
    /// afkconvoy intake --issue 12 --workflow feature
    /// ```
    ///
    /// A stage that splits its issue runs it once per child, saying with `--blocked-by` which of the others a child has to wait for, and then declares `blocked` on all of them (`afkconvoy verdict --help`):
    ///
    /// ```
    /// afkconvoy intake --issue 13 --workflow fix --blocked-by 11,12
    /// ```
    ///
    /// Fleet repairs are dispatched automatically outside product workflows; no intake is needed.
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree: it starts in its role's directory, so it changes to the worktree first, as the line its brief gives for `afkconvoy verdict` does. The fleet put its role and the issue it is on in its environment (AFKCONVOY_ROLE, AFKCONVOY_ISSUE), and the command reads them from there; `--role` and `--from` say them by hand. A patrol's worker is on no issue: it has its patrol's name there instead (AFKCONVOY_PATROL), which `--patrol` says by hand. The repair worker has no role and is on no issue and no patrol: it says `--repair`, in its own checkout, and nothing else of who it is.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the config of the checkout, the team it names, the issue, and — to count — the comments of every open issue that carries a flow label. It writes on the issue, in this order: a blocked-by edge to each issue `--blocked-by` names that GitHub does not record yet; one comment, the intake record, whose marker carries the workflow, the role and the issue the worker was on — or the patrol it ran, or, of the repair worker's, `by=repair` and nothing else; then, where the checkout's tracker is another repo (`afkconvoy run --tracker`) and the issue lacks it, this repo's repo label, `repo:<owner>/<repo>`; then the workflow's flow label, `flow:<workflow>`. Nothing but this command writes that format. Then it wakes the fleet, as `afkconvoy wake` does: the issue is on the frontier at the next tick — or, with an open issue to wait for, once that has closed. The edges go first, so the issue is never on the frontier ahead of what blocks it.
    ///
    /// For a Jira delivery, run `afkconvoy flows --delivery <identity>`, open with its line (no runnable label), then run its intake_line. `--delivery <identity>` attaches the existing delivery before any runnable label is added. In a bound repo select --workflow; in an unbound repo omit it and intake adds the ready label. An operator or discussion agent may use explicit --delivery without a worker role: this follows human intake and consumes no worker intake limit. Workers still name their role and source task or patrol and count against the existing limit.
    ///
    /// A worker's --from (or its worker issue environment) inherits that source task's delivery automatically. Explicit --delivery selects the named delivery instead of inheritance for work genuinely belonging to another delivery. A dependency, parent link, checkout branch or discussion title supplies no delivery. Keep --blocked-by separate: it orders tasks and never assigns membership. The delivery reference lives once in the task's remote target record; stages, restart, rework and harness/model switches retain it. Missing, contradictory and cross-repo delivery context refuses without a runnable label being added. Existing pinned targets cannot be reassigned; claims, retained work and unaccounted PRs/candidates are preserved and reported.
    ///
    /// The issue, the issues it waits for and every issue it counts are the tracker's: the checkout's own repo, or the one its host says, where it counts only the issues that carry this repo's repo label.
    ///
    /// An issue not found there is refused naming the tracker, where `gh` finds it in the checkout's repo (following a transfer), and `gh issue transfer <n> <tracker> --repo <its repo>`. Use the number GitHub assigns after transfer. For another repo's blocker, run `afkconvoy flows` in that repo's checkout, open it on the tracker with that repo's repo label using its line (omit the flow label until intake), and take it in there.
    ///
    /// The limit. Before it writes anything it counts the open issues taken in by workers: those that carry a flow label and an intake record for it. With that count at the repo's `intake_limit` (see `afkconvoy config --help`) it refuses and writes nothing. The count is taken from the records each time and stored nowhere, so closing such an issue makes room. An issue a human labelled has no record: it is never counted, and nothing here refuses a human's label.
    ///
    /// ## What it answers
    ///
    /// {"issue", "workflow", "taken", "role", "from", "blocked_by", "open", "limit", "woke"}
    ///
    /// Delivery intake also answers `delivery` and resolved `target_branch`. Human delivery intake omits worker identity, open and limit. Unbound intake omits workflow and answers target_branch for ordinary tasks too.
    ///
    /// - taken: true: the record and the flow label were written
    /// - `role`, `from`: who took it in: the worker's role, and the issue it was on. A patrol's worker answers `patrol`, the patrol's name, in place of `from`. The repair worker answers "repair": true in place of both
    /// - `blocked_by`: the issues it was said to wait for
    /// - `open`: the open issues taken in by workers, this one among them
    /// - `limit`: the repo's `intake_limit`
    /// - `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers
    ///
    /// {"issue", "workflow", "taken": false, "standing"}
    ///
    /// - taken: false: the issue follows that workflow already: nothing was written
    /// - `standing`: how it came in: {"by": "worker", "role", "from", "url"} with the comment of its intake record — `patrol` in place of `from` where a patrol's worker took it in, "repair": true in place of both where the repair worker did — or {"by": "human"}
    ///
    /// Errors, as {"error": …} with a non-zero exit: the intake limit is reached — the error says the count and the limit; the team has no workflow of that name — the error lists the ones it has; the team has no such role, or no such patrol; the repo is unbound; the issue is closed, is a pull request, already carries the flow label of another workflow, or carries the repo label of another repo — it is that repo's to work — or, in a repo that is its own tracker, any repo label; the role, or both the issue the worker is on and its patrol, is neither in the environment nor given, and `--repair` is not given; `--repair` with a role, an issue or a patrol, given or in the environment; both `--from` and `--patrol` are given; `--blocked-by` names the issue itself, a pull request, or an issue that cannot be read; the team has a fault; "is not in a git checkout", or git's own words for a checkout it refuses.
    ///
    /// ## What trips you up
    ///
    /// - It is the only way a worker's intake is on the record. A flow label a worker puts on by itself would read as a human's and escape the limit: the guard refuses that, and an intake record written by hand (`afkconvoy guard --help`).
    /// - An issue a patrol's worker takes in counts against two limits: the repo's `intake_limit`, which refuses here, and the patrol's own `max_open`, which refuses nothing here — the fleet does not run the patrol again while that many are open.
    /// - The limit bounds the work the fleet gives itself, not a day's work: it is set high, to stop a runaway. `afkconvoy plan` reports the count against it.
    /// - The record counts only while the issue carries the flow label it names. A human who moves the issue to another workflow has taken it in themselves.
    /// - A split is nothing but this and a `blocked` verdict: the parent is parked on its children, and the stage that split it runs again when all of them have closed. A child counts against the limit like any issue a worker takes in, and may be split in turn.
    /// - An issue the repair worker takes in counts against the limit too. This compatibility intake is distinct from the fleet's automatic repair issue: that issue carries the ready and convoy-repair labels and runs outside product workflows.
    /// - A child has to close for its parent to return. One whose workflow ends in `done` stays open until somebody closes it.
    /// - `--blocked-by` is read only as the issue is taken in. On one that follows the workflow already, nothing is written.
    /// - Two workers taking in at the same moment can both pass the count: the limit can be overshot by as many as ran at once.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Run before dispatch or intake: `afkconvoy target --issue 7 --branch delivery/ABC-123`. Then give the issue its ready or flow label as usual. The worker cuts from that branch, opens its internal PR against it, gates and lands there, and closes only its task issue. The repo's configured base and its release destination stay as configured.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, before making the task dispatchable.
    ///
    /// ## What it reads and writes
    ///
    /// Without --branch, reads the target, or the configured base for a task not yet resolved. Selection requires an existing branch on origin and an open issue in this checkout's private tracker scope. It writes an immutable remote record, keyed by the tracker issue identity. A first claim also pins the configured base when no selection exists. Stages, continuation and landing retain it across restarts. Reading a malformed record or a missing branch refuses; it never falls back to the configured base.
    ///
    /// ## What it answers
    ///
    /// {"issue", "target_branch"}, with "delivery" for an attached task, and "woke" after selection. Selection is idempotent for the same branch. Errors answer {"error": …} with a non-zero exit.
    ///
    /// ## What trips you up
    ///
    /// A different resolved target, a claimed task, remaining candidates, an unreadable tracker, a foreign repo label or a missing branch is refused with a diagnostic. A resolved target cannot be changed: open a new task for another destination.
    ///
    /// Jira delivery branches are reserved: attach tasks through `afkconvoy delivery --jira <url> --final-target <branch> --issue <n>`. Their target records pin a reference to the immutable delivery definition; they never copy its branch settings. A missing or replaced definition blocks work instead of selecting the configured base.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Run `afkconvoy delivery -C <checkout> --jira https://example.atlassian.net/browse/ABC-123 --final-target <branch> --issue 7` before making the first task dispatchable. The checkout names the code repo. Repeat with another --issue to attach another open task. Omit --final-target and --issue for read-only inspection. Repeat the creation command to reconcile pending final review.
    ///
    /// For discussion and decomposition, create the delivery once without --issue, take its returned `delivery` identity, and use `afkconvoy flows --delivery <identity>`. Its line opens a non-runnable task; its intake_line records membership and then enables dispatch. For example, create ABC-123 and ABC-456 with full Jira URLs and --final-target <branch>. Use ABC-123's flows line twice, then `afkconvoy intake --issue 11 --delivery <ABC-123-identity>` and the same for issue 12. Use ABC-456's line once and intake issue 13 with <ABC-456-identity>. In a bound repo add the chosen --workflow to each intake. Tasks 11 and 12 target ABC-123's delivery branch; task 13 targets ABC-456's. Their internal PRs land only there. Repeated delivery inspection lists their tracker issue identities without writing anything or creating tasks. `afkconvoy plan` and `afkconvoy doctor` answer `targets` with delivery membership, resolved target_branch, configured_base, whether it is pinned, and per-task errors; `afkconvoy show --issue <n>` also shows delivery and target_branch for a bound task.
    ///
    /// Run `afkconvoy delivery -C <checkout> --jira <url> --handoff` once every attached task has landed. It checks the whole attached set and what actually landed, gates the combined delivery against the current final target without publishing that merge, and marks the same final PR ready for a human. Repeat it after a new task, a new delivery commit, or a moved final target: an older green check does not certify later work. After a human merges or closes that PR, run it again to record what happened.
    ///
    /// ## Who runs it
    ///
    /// The operator. Jira is an external reference; no Jira login or synchronization is performed.
    ///
    /// ## What it reads and writes
    ///
    /// The full HTTPS Jira browse URL, including site and optional context path, and the code repo identify the delivery. Creation requires an explicit existing final target on origin and a private tracker. It atomically creates an immutable JSON definition at delivery/<identity> under the remote fleet ref layout and a predictable delivery/jira-<key>-<identity> branch at that target's tip. This definition alone owns both branches. Task target records store only a delivery reference. The repo defaults and release destination stay unchanged.
    ///
    /// A task cuts from the delivery branch and lands its internal PR only there. The final review PR is opened as a Draft as soon as a meaningful diff exists, after task landing or on an acting tick. No diff leaves creation pending without empty commits or PR attempts. Its number lives once in the subordinate delivery-pr/<identity> receipt. Each invocation discovers actual remote branches, records and all PR states first. Later tasks reuse the same final PR even after it is closed or merged. A human owns colleague review and merging the final PR; ordinary dispatch, landing, watch and recovery exclude it.
    ///
    /// `--handoff` reads the delivery definition, every attached task's issue state, claims, open internal PRs, blockers, landing candidates and linked worktrees, plus the delivery branch and the current final target. A green check records that snapshot at delivery-evidence/<identity>, updates the one final PR's body with the Jira reference, what each attached task came to and the verification evidence, and marks that PR ready. It does not open a second final PR, does not publish the merge to the final target, and does not change reviews. A conflict or a red gate leaves both branches as they are and returns the PR to draft without recording a green snapshot. A merged or closed final PR is observed once at delivery-settled/<identity>; repeating the observation does not write another record or remove other work.
    ///
    /// ## What it answers
    ///
    /// {"jira", "delivery", "repository", "record", "branch", "final_target", "final_pr", "tasks"}, and "woke" after creation. final_pr is {number, url, state, draft, merged} or {pending: reason}; tasks lists the attached tracker issue identities. Inspection writes nothing and adds `handoff`, the read-only summary `plan` and `doctor` list under deliveries. Errors answer {"error": …} with a non-zero exit.
    ///
    /// `--handoff` answers {jira, delivery, branch, final_target, final_pr, tasks, ready, reasons, gate, settlement, discrepancy, delivered, reviews}, and "woke". tasks is each attached issue with `result` (implemented, not_planned, abandoned or incomplete) and blocking reasons. ready is true only when the evidence just recorded certifies those heads and what each task came to and the final PR is open and not a draft. reasons names every attached task that is not implemented, a conflict, a red gate, or a closed or incomplete merge. gate is {status, source, tree} after an aggregate check, or {status: red, detail} or {status: conflict}. settlement is the one observation, or null. delivered is true only when that observation is a complete merge onto the declared final target and still matches. reviews is "not changed". A task closed as not planned or abandoned is not implemented. An empty attached set is not ready.
    ///
    /// ## What trips you up
    ///
    /// Ticket keys alone are not references: different Jira sites may reuse a key. An unrelated existing branch, conflicting final target, claimed task with another target, ambiguous PRs, unreadable record or missing owned branch refuses with a diagnostic. Restore inconsistent remote metadata before continuing attached tasks; nothing falls back to the configured base. A ticket spanning code repos has a separate delivery and final PR in each. To hand a task to the fleet, give it its ready or flow label after attaching it.
    ///
    /// A green gate of one task does not make the final PR ready. `--handoff` is the check of the whole delivery. The fleet, ordinary landing, recovery and watches do not merge the final PR because it is green or names a delivery. Closing it without a merge is not delivery: the observation says it needs action. A merge while an attached task is not implemented is a discrepancy, not delivery of the whole Jira ticket. A new task, a new delivery commit, or a moved final target makes an older snapshot not certify; run `--handoff` again. It withdraws readiness with `gh pr ready --undo` or marks the same PR ready with `gh pr ready`. It does not dismiss reviews.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// When the fault is one every issue shares — the base branch is broken, so every gate is red whatever its PR holds — and one issue carries the fix. Raise the lock on that issue:
    ///
    /// ```
    /// afkconvoy lock --issue 7
    /// ```
    ///
    /// From the next tick the fleet works lock issues only, until the last of them closes. For anything that is wrong with one issue alone, declare a verdict instead.
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree. In a bound repo, a patrol or a stage that finds the base branch red. In an unbound repo, a worker or a watch that proves the base branch red the same way gated alone in a clean checkout: no role is needed. The repair worker does the same in either repo. Each finds the open issue for the fix or opens one, makes it dispatchable — the ready label in an unbound repo, intake to a workflow in a bound one — then runs this command on it from its worktree. The operator may run it too, in a checkout of the repo; a human may as well put the lock label on the issue on GitHub.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the issue and the config's `lock_label` (default fleet-lock). On an open issue that does not carry the label it writes one comment — the record of the role that raised the lock, read from `--role` or from the worker's environment (AFKCONVOY_ROLE); a worker of an unbound repo has none — and then puts the label on the issue. Then it wakes the fleet, as `afkconvoy wake` does.
    ///
    /// The issue is the tracker's: the checkout's own repo, or the one its host says (`afkconvoy run --tracker`). There a lock holds the fleets of the repos whose repo label it carries, `repo:<owner>/<repo>`, and no other repo's fleet that works the same tracker: before the lock label it puts this repo's repo label on an issue that lacks it.
    ///
    /// It claims nothing and starts nothing. The lock issue is worked as any issue is: in a bound repo once it carries a flow label, in an unbound one once it carries the ready label. Nothing here takes a lock off: closing the issue ends it — landing the PR that closes it does.
    ///
    /// ## What it answers
    ///
    /// {"issue", "label", "role", "raised", "locks", "woke"}
    ///
    /// - `label`: the lock label the issue now carries
    /// - `role`: the role on record as having raised it, or null
    /// - raised: true: the record was written and the label put on
    /// - raised: false: the issue was a lock already: nothing was written
    /// - `locks`: every fleet lock standing now, lowest number first
    /// - `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers
    ///
    /// Errors, as {"error": …} with a non-zero exit: the issue is closed; there is no such issue; the issue carries the repo label of another repo — or, in a repo that is its own tracker, any repo label; the config cannot be read; "is not in a git checkout", or git's own words for a checkout it refuses.
    ///
    /// ## What trips you up
    ///
    /// - A lock stops everything else, on every host: no other issue is dispatched, no stale claim of another issue is taken, no issue is handed to a human, and no PR of another issue lands in a turn (`afkconvoy land` answers `locked`).
    /// - Workers already running are left alone, and nobody asks after them: what they declare — a verdict, a PR — is acted on once no lock stands. Their claims take no slot meanwhile, so a lock issue is started even by a fleet at its bound.
    /// - Once the last lock has closed, a bound repo's stage that declared `escalate` or stayed silent past its nudge before then runs again, on the base branch as it is now: a broken base branch is what such a lock is raised for, and a human could only answer "run the gate again". Every other verdict stands.
    /// - What was on the base branch before the lock is not taken off it. The lock issue's PR lands after it.
    /// - A lock issue nothing makes dispatchable — no ready label, no flow label, an epic label, an open blocker — is worked by nobody, and the lock stands all the same. `afkconvoy plan` shows the lock under `locks` and the issue under `excluded`, with why.
    /// - It does not check that the base branch is broken. It is refused to nobody: the guard recognises the label being moved by hand, not this command.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// When you want a patrol's run now rather than when its interval has passed — a review right after several landings, say — without editing the team's patrol file, which changes the schedule for every repo that runs it:
    ///
    /// ```
    /// afkconvoy patrol-now sweep
    /// ```
    ///
    /// The next tick of whichever host claims the run starts it, and this command wakes the `run` of this host so that tick starts at once.
    ///
    /// ## Who runs it
    ///
    /// The operator, or any human, in a checkout of a bound repo whose config names the patrol under `patrols`. No worker is told to: a patrol's worker ends its run with `afkconvoy over`.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the team, the patrol's file, the remote's records of the patrol and what is open on GitHub — what `afkconvoy plan` reads of a patrol. Then it pushes one record to the remote, a ref of the fleet's, `asked/<name>`, saying when it asked and from which host, and wakes the `run` of this host (`afkconvoy wake --help`). It changes no config file, starts no worker and claims nothing itself.
    ///
    /// A run asked for this way:
    ///
    /// - passes `every` and `max_runs_a_day`: it runs though the interval has not passed since the last run, and though the day's runs have reached the bound;
    /// - is still held by what holds a due run: a fleet lock, the patrol's open issues having reached `max_open`, its completed issues having reached `max_landed_a_day`, a failed run whose report is still open, and a run of it still going. Each refuses the ask, and nothing is recorded;
    /// - is claimed on the remote as a due run is: of several hosts asked at once, one runs it and the others' ticks say `lost`;
    /// - is recorded as the patrol's run, with `asked=human` in its record: its start is the last run `every` counts from, and one of the day's starts `max_runs_a_day` counts. The ask is deleted as the run is recorded.
    ///
    /// Until a tick starts the run, `afkconvoy plan` shows `asked` (normally `due`; new completions may hold it again): when the ask was made. The panel says a human asked for it.
    ///
    /// ## What it answers
    ///
    /// {"patrol", "asked", "ref", "woke"} — the ask is on the remote at `ref`, made at `asked` (epoch seconds); `woke` is whether a `run` on this host was told.
    ///
    /// Errors, as {"error", "see"} with a non-zero exit, nothing recorded:
    ///
    /// - a fleet lock stands: no patrol runs until the last lock issue closes;
    /// - the open issues the patrol opened have reached its `max_open`: it runs once one closes, or `max_open` is raised;
    /// - its last run failed and the issue that reports it is open: close it once you have looked;
    /// - a run of it is still going: ask again once it has ended;
    /// - the repo runs no patrol of that name: the config's `patrols` lists those it runs.
    ///
    /// Also: "is not in a git checkout", or git's own words for a checkout it refuses; the config, the team, the remote or GitHub cannot be read; the remote does not take the push.
    ///
    /// ## What trips you up
    ///
    /// - It runs nothing itself: a `run` does, at its next tick. With no `run` on any host (`woke: false` here, and none elsewhere), the ask waits on the remote for the first tick of any of them.
    /// - An ask made while nothing held the patrol stands until a run answers it, whatever comes to hold the patrol meanwhile: a lock raised since holds it, and it runs once the lock has closed. `afkconvoy plan` shows it held, with `asked`.
    /// - Asking twice before the run asks once: one run answers both.
    /// - The schedule goes on from the run asked for: the next due run is `every` after its start, not after the last scheduled one.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    PatrolNow {
        /// The patrol, as the repo's config names it under `patrols`
        name: String,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Declare a patrol's run over: the next tick ends it (a worker's command)
    ///
    /// ## When to run it
    ///
    /// Once, as the last command of a worker that is on no issue: a patrol's, when it has nothing more to open. The worker's brief ends with the exact line, the run named in it.
    ///
    /// ## Who runs it
    ///
    /// A patrol's worker, in its own worktree: the checkout it was started in. Never a worker on an issue: its stage ends with a verdict (`afkconvoy verdict --help`).
    ///
    /// ## What it reads and writes
    ///
    /// It reads the claim on the run off the remote and pushes one record that names it — a ref of the fleet's, `over/<run>` — then wakes the `run` of this host (`afkconvoy wake --help`). The next tick finds the run declared over and ends it at once, as it would one whose worker is gone (`end_patrol`: `afkconvoy tick --help`): a patrol's checkout is still checked for a commit and a PR. The slot the run held is free from that tick on.
    ///
    /// ## What it answers
    ///
    /// {"over": true, "run", "ref", "woke"} — the record is on the remote; `woke` is whether a `run` on this host was told.
    ///
    /// Errors: no run is claimed under that name — it was ended already, or never started; the name is an issue's number; the worker is on an issue (AFKCONVOY_ISSUE is in the environment); the remote cannot be read or does not take the push.
    ///
    /// ## What trips you up
    ///
    /// - It ends nothing itself: the fleet ends the run, at its next tick. A worker that goes on working after it is cut short.
    /// - A worker that never runs it loses nothing but time: the fleet ends the run once the worker is gone, or has been stopped for the grace period — five minutes in which the run holds a slot of the concurrency no issue can be worked in.
    /// - The record names the claim it was read from: left behind, it says nothing of the next run of the same patrol.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Over {
        /// The run, as its brief names it
        #[arg(long)]
        run: String,
        #[command(flatten)]
        at: At,
    },
    /// Take the steps of a cut: set the version, land it, tag it, publish and install it (a cut's command)
    ///
    /// ## When to run it
    ///
    /// The fleet runs it: in a repo that turns cutting on, a tick starts a cut once landed work is past the newest version tag and either the fleet is idle or the oldest landing reaches `busy_after_seconds` (default {const:CUT_BUSY_AFTER_SECONDS}s). At the bound it starts while busy too, whatever claims or frontier remain, even while admission holds workers back. It cuts from the base branch as it stands; work still in flight goes into a later version. A remote with no version tag and a declared `initial` is cut on an idle tick. No flag, command or fixed schedule starts one.
    ///
    /// A human runs it with `--from <step>` once, after a cut failed, to go on from the step that failed: the issue the failure opened gives the exact line, in the checkout the cut kept. A watch may run that line too after fixing the cause; only the four human kinds in `afkconvoy help humans` stay with a human. To have a landed fix cut, the watch wakes the repo's checkout and fixes what holds its fleet back: the next idle tick, or busy tick past the age bound, starts the due cut through these same steps.
    ///
    /// ## Who runs it
    ///
    /// A cut's worker, in its own worktree: the checkout (`afkconvoy-cut`) the tick that started it cut from the base branch as it stood; a human, there, after a failure. Run anywhere else it is refused.
    ///
    /// A cut is a worker in no role, started the way a watch is: its own checkout, the bounds of any unattended worker, a time limit. It is this command, not a model session: its steps are known, each is timed, and a failure names its step. It takes no slot of the concurrency.
    ///
    /// The fleet titles its terminal `afkconvoy cut v<version>`. Before its first step, including when continued with `--from <step>`, it prints one line naming the version, the tag it is cut past (or the initial version), the PRs it holds, the steps it will take, and the paths to `convoy-cut.log` and `convoy-cut.json`. Each step prints a start line and an end line with its seconds and whether it passed or failed; its commands' output stays between those lines. These lines and the commands' output go to stderr; stdout holds the JSON answer.
    ///
    /// ## Turning it on
    ///
    /// A repo turns cutting on by naming its cut steps in its config file, {const:CONFIG_FILE}, in a block of their own opened by the line ```yaml cut — never in the ```yaml block of the config, which a version older than this one reads with every key known to it, refusing a key it does not know. Older versions read only that first block, and read past this one.
    ///
    /// ~~~markdown
    /// ```yaml cut
    /// initial: 0.1.0
    /// set: ./packaging/set-version.sh "$VERSION"
    /// publish: |
    ///   ./packaging/release.sh publish "$VERSION"
    ///   brew upgrade acme/tap/widget
    /// ```
    ///
    /// ```yaml cut
    /// busy_after_seconds: {const:CUT_BUSY_AFTER_SECONDS}
    /// ```
    /// ~~~
    ///
    /// `set` and `publish` are required. Optional `initial` declares the first version, exactly X.Y.Z: three whole numbers without leading zeroes, a `v` prefix or a prerelease suffix. Without a version tag and without `initial`, nothing is cut. With it, the next idle tick cuts that exact version, whatever labels earlier PRs carry; after a version tag exists, `initial` is ignored and the usual patch/minor selection applies. No seed tag or manual release is needed.
    ///
    /// A repo may declare host checks in a `yaml cut-needs` block (`afkconvoy config --help`). A host whose check fails does not claim a due cut; the tick, plan and doctor name it. Stdout is discarded and failed stderr is private to the host.
    ///
    /// Optional `busy_after_seconds` bounds how long the oldest landing past the newest tag waits for an idle fleet. Leaving it out means {const:CUT_BUSY_AFTER_SECONDS}s; zero permits a busy cut at the next tick. Its clock uses the landing commit's time on the base branch, rather than the age of commits on a side branch. Put this key in a second cut block as above: older versions that know cut steps read only the first cut block and skip the addition. All cut blocks form one mapping, and a key declared twice is refused. `plan`'s `cut` row gives `oldest_landing_age_seconds` and `busy_in_seconds`: zero means a busy cut may start now; null means no landing past a version tag.
    ///
    /// Each step is run by `sh -e` in the cut's checkout with the version, `0.5.17`, in `$VERSION`; what it prints goes to the terminal and the cut's log, in the checkout's git dir (`convoy-cut.log`). Its lines run in order, and the first that fails fails the step: the lines after it are not run. Above, a publish that fails is not followed by the upgrade, which would find nothing newer and succeed. A line `set -e` changes nothing of that, and `set -ex` adds each line to the log as it runs. A repo whose file has no such block turns cutting off: nothing is cut. `afkconvoy config` answers the steps as `cut`, and `afkconvoy plan` says where the cut stands, including `initial` while there is no version tag.
    ///
    /// ## What it does
    ///
    /// The version: the next patch past the newest version tag — a tag `v` and three numbers, `v0.5.16` — or the next minor, its patch 0, where a PR that landed since carries the label `{const:CUT_MINOR_LABEL}`. The tick that starts the cut picks it, from the tags as they are then, and keeps it in the checkout with the PRs the cut holds: those the base branch's landings past the tag name.
    ///
    /// Its steps, in order, each timed:
    ///
    /// 1. `set` — on the snapshot, the base tip the checkout was cut at, runs the `set` lines and commits what they changed, titled `<repo> <version>`: the version's commit, whose one parent is the snapshot. Lines that change nothing still make that commit, empty. Where the remote has the tag `v<version>` already, or a branch a cut of this version pushed, the cut takes up that commit instead, and makes no second.
    /// 2. `push` — pushes that commit to a branch of its own, `afkconvoy-cut/v<version>-<started>`. It opens no PR.
    /// 3. `gate` — gates that commit in the checkout: the whole gate, the fast gate and the slow gate, in a worker lane as any heavy command is (`afkconvoy exclusive --help`); a green run on record for its tree answers. It takes no landing turn, is no candidate and reserves no barrier: turns land on the base branch while it runs, whether it passes, fails or hangs.
    /// 4. `tag` — tags that commit `v<version>` (annotated), and pushes the tag. The PRs it holds are read again, up to that commit. A patch also checks landings up to the current base branch: where one landed while it ran and carries `{const:CUT_MINOR_LABEL}`, it stops at `tag` without tagging or publishing, and opens no failure issue. The next tick releases its claim and starts the minor cut, unless a fleet lock stands. An initial cut holds every PR the base branch's landings name, and keeps its declared version.
    /// 5. `publish` — with the tag checked out, runs the `publish` lines.
    ///
    /// The base branch never receives the version commit: it pushes its own branch and its version tag, and nothing to the base branch. The version's home is the tag; version files on the base branch stay as they are, and the next cut's `set` lines write its version on its own snapshot. The tag's parent is a base commit, so the landings past the tag are still those on the base branch past that parent.
    ///
    /// ## What it reads and writes
    ///
    /// It keeps the cut's record in the checkout's git dir (`convoy-cut.json`): the version, the tag it is cut past (`from: null` for an initial cut), the PRs it holds, its branch and its commit, the step it is at, and each step it took with its seconds — and, where one failed, which and why. The tick reads it, and ends the cut once it says it took every step or failed at one, once its terminal is gone, or once its time ran out: three hours beside the two gate limits (`gate_seconds`), for its gate and its wait for a worker lane — five hours at the default.
    ///
    /// A cut's gate and exclusive commands go ahead of waiting worker commands, keeping request order among cuts. They never interrupt a running command. Queue waits count toward the cut's time limit; the gate's own limit starts once its worker lane is granted. The tick line that starts a cut says whether the fleet was `idle` or `busy`.
    ///
    /// A cut that took every step is a line of the tick log — `cut v0.5.17 (past v0.5.16, holding #451, #452) · its commit 0123456789 on afkconvoy-cut/v0.5.17-1793666786 · set 3s, push 2s, gate 610s, tag 1s, publish 95s` — `afkconvoy doctor` lists it among its events, and the panel shows it; its checkout is removed.
    ///
    /// A cut that failed, ran out of time, was ended from outside, whose checkout is gone, or whose host went silent opens one issue under the escalate label, titled `afkconvoy: the cut of v<version> failed`: which step failed, why, what it did before, and the line that goes on from there. The tick log, `doctor` and the panel say it too. Its checkout is kept, its terminal closed. No cut starts while that issue is open (`afkconvoy help humans`).
    ///
    /// With `--from <step>` it takes that step and those after it, the record kept as it was; without, the step after the last it took. Once it took its last, it closes the open issue that said this version's cut failed.
    ///
    /// ## What it answers
    ///
    /// {"version", "tag", "prs", "branch", "commit", "took", "closed", "said"} — the cut took every step: `took` is each step with its seconds, `closed` the issues it closed. {"version", "stopped": "tag", "why", "said"} — a minor landing superseded the patch; nothing was tagged or published, and the fleet cuts the minor next. {"version", "failed", "why", "said", "log"} — it failed at the step `failed`.
    ///
    /// Errors: the checkout is no cut's, or keeps no cut's record; the repo names no cut steps; `--from` names no step.
    ///
    /// ## What trips you up
    ///
    /// - One cut runs at a time across a repo's hosts: it is claimed on the remote as a patrol's run is, and the tick that claims it reads the version tags again. A peer's that went silent is unclaimed by the next tick, which opens the failure issue for it: no cut of that version starts again on its own. A fleet lock and an open cut failure hold a new one back.
    /// - A cut interrupted after tagging goes on from the tag; one interrupted before, from its branch. A cut started again for the same version takes up that commit, and pushes no second branch.
    /// - Cut records and serial barriers that earlier versions left on the remote candidate chain are read as before, and drain: nothing new names a cut there.
    /// - `sh -e` fails a step at a line whose command fails, not at one whose command it tests: a command of an `if` or `while` test, on the left of `&&` or `||`, after `!`, or before the last of a pipe fails nothing. `./packaging/release.sh publish "$VERSION" | tee published` passes whatever `release.sh` did.
    /// - The cut's commit is made in the checkout's name, by its git config: a repo whose base branch is protected against pushes is as good as any other here, for the cut pushes nothing to it.
    /// - Turns keep landing while a cut runs. A cut that hangs costs only its own version: its time limit ends it, and its failure issue holds the next cut back.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// For work you know to be heavy: a build, a test suite or a benchmark that takes most of the host's cores, memory or disk for minutes. You know it from a watch that typed at you that the work really is that heavy, or from your role's rules that name the command. Put `afkconvoy exclusive --` in front of that command, every time you run it from then on:
    ///
    /// ```
    /// afkconvoy exclusive -- cargo test --workspace
    /// ```
    ///
    /// Not for everyday commands — `git`, a single test, a quick build: the queue makes each one wait for every heavy one before it, of every fleet on this host.
    ///
    /// Split a long measurement up front. Run each step that needs a quiet host as its own `afkconvoy exclusive`, so landing turns can run between steps.
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree. Anyone else on the host may too: the queue is the host's, not a fleet's.
    ///
    /// ## What it reads and writes
    ///
    /// Commands use worker lanes, {const:GATE_LANES} by default. `run --gate-lanes N` sets the count live across the host, and `default` restores it. N must be above zero. Independent candidate worktrees of one repo may validate together; ordinary heavy commands retain same-repo exclusion. Heavy gates, tests, benchmarks and builds use these lanes in FIFO order; a light gate takes its repo's own lane. A candidate holds its worker lane while it builds and gates; a round's slow gate takes one of the host's slow lanes, 1 by default, which `run --slow-gate-lanes N` sets live. A gate alone in the queue keeps every core; one that starts while another command runs in a worker, fast or slow lane is given its share of the host's cores — its cores over those lanes together, at least 1 — as `NEXTEST_TEST_THREADS`, unless its environment already sets it, and its build every core (sunfmin/control#245, sunfmin/control#318). Inherited kernel leases keep a dead owner's still-running descendants from being replaced.
    ///
    /// A worker's fast gate, run by `afkconvoy gate --fast`, takes the fast lane instead: `gate --fast` tells its command's `exclusive` that it runs the fast gate, through `AFKCONVOY_GATE`, which `exclusive` does not pass on to its command. Fast gates contend only with each other, up to the host setting `fast_lanes`, {const:FAST_LANES} by default (`run --fast-lanes N`); no slow gate, turn or other command holds a place they need, and fast gates of one repo run side by side, each in its own worktree. A slow gate run alone, `gate --slow`, keeps the worker lanes.
    ///
    /// A cut's gate and exclusive commands go ahead of waiting worker commands, known by the cut's own worktree. Cuts keep request order among themselves, and ordinary commands keep theirs. Priority never interrupts a running command or adds a lane. A worker may therefore wait behind a cut's gate or publish step. Its runs line says "runs as a cut, ahead of waiting worker commands".
    ///
    /// The queue is the host runner's (`afkconvoy run --help`), held in its process: `exclusive` asks it on its socket, naming its checkout, and stays on the line while it waits and while its command runs. A process that leaves it — its command ended, or it was killed while waiting or running — frees its place for the next. The command runs in the directory `exclusive` is run in, or with `-C <dir>` in that one, as named: a checkout of the repo or a directory in one, and not that checkout's top.
    ///
    /// It writes a line to the tick log of this host for this repo as it starts to wait, when another command is ahead of it; as its command runs — in its turn, or at once in no queue, with why; as it waits again, in the next host runner's queue; and as it ends, with its exit status. Its lines name the worker or repo lane. `doctor` and the panel show each lane's running and waiting commands, with the counts in force. A turn logs the commit it merged, whether its gate was green and its landing.
    ///
    /// While it waits or runs, the fleet counts the worker busy, not silent: it is not nudged for that. Waiting for a turn uses none of the gate limit (`gate_seconds`); the limit times the gate command once the turn starts. A worker with confirmed live background work remains waiting even past the observation interval; silence alone does not authorize escalation or continuation.
    ///
    /// ## What it answers
    ///
    /// A running command is ended once it has held other commands up for {const:EXCLUSIVE_SECONDS}s, or the seconds the host setting `afkconvoy run --exclusive-seconds N` says (`AFKCONVOY_EXCLUSIVE_SECONDS` names it too). `--exclusive-seconds default` changes it back; 0 ends a command as soon as another waits. Only time with another command waiting counts: time running alone never uses the bound, and gaps without waiters pause it. A changed host setting applies to the command already running. On reaching the bound it ends the command and its descendants before the next command starts, exits {const:EXCLUSIVE_BOUND_CODE}, and prints one line on stderr beginning `[afkconvoy] exclusive:` telling the worker to run each step as its own `afkconvoy exclusive`. The tick log records the command, its runtime and how many commands waited; `doctor` names each recent occurrence as a warning.
    ///
    /// Unlike every other command, no JSON of its own: what the command prints is printed, and it exits with the command's exit status — 128 and the signal for a command a signal ended. While it waits it says so on stderr, in one line beginning `[afkconvoy] exclusive:`.
    ///
    /// With no host runner on this host it runs the command at once, in no queue, and says so on stderr: `[afkconvoy] exclusive: runs `<command>` at once, in no queue: no host runner runs on this host`. So it does too where the host runner cannot hold it — one of an older afkconvoy, which cannot read the question — saying why. A host runner that is starting or stopping is waited for, as one replaced is.
    ///
    /// When the host runner is replaced — an upgrade moves it (`afkconvoy run --help`) — or ends, a command that waits asks the next one, within a minute, keeping its lane and when it first asked: within its lane it waits before every command that asked after it, saying so on stderr. Where no host runner places it within the minute, it runs at once, saying so. A command that runs runs on to its end: the next host runner does not know of it, and may start the next command beside it.
    ///
    /// Exit {const:EXCLUSIVE_BOUND_CODE} is reserved for the bound: a command that itself exits {const:EXCLUSIVE_BOUND_CODE} is reported as exit 1. Every other command exit status is passed through. It exits 127 when the command cannot be started, and 125, with one JSON object {"error"} on stdout, when the host runner cannot be asked — the cache directory is not known — or what it said cannot be read, or `-C` names no directory of a checkout: the command did not run, and nothing waited.
    ///
    /// ## What trips you up
    ///
    /// - It may wait long: as long as every heavy command ahead of it runs. Run it as you would the gate — in the background, or with a time limit of the gate limit — not under a short tool time limit that kills it while it waits.
    /// - Killing it kills its place in the queue, not only its wait: run it again to wait from the end.
    /// - Only commands run through it wait for one another. A heavy command run without it runs beside the one that runs alone.
    /// - The gate, run by `afkconvoy gate`, goes through the queue only when the config's `gate_command` does — `afkconvoy exclusive -- ./gate.sh`, for a gate that fills the host by itself. The turn owns the lane before it merges; the gate command's nested `exclusive` inherits that lease and never waits on itself. Existing `gate_command: afkconvoy exclusive -- ./gate.sh` configurations keep working. Never use `afkconvoy gate` or `land` as the gate_command: config and check name the build-and-test command to set.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Exclusive {
        /// The command and its arguments, after `--`
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Allow or refuse the tool call described on stdin (a worker's hook, added at launch)
    ///
    /// ## When to run it
    ///
    /// Never by hand. Every worker and the watch ask it before each tool call. Outside a bound stage, --terminal-input-only checks input to the asking Terminal without requiring stage facts. Bound workers also check what their stage may not do. Read this to learn what a worker is refused, or why one was.
    ///
    /// ## Who runs it
    ///
    /// Claude Code, as a hook: the fleet adds it to every worker of a bound repo as it starts it, whatever the role's repo holds, and Claude Code runs it before each tool call the worker makes. A call it refuses is not made.
    ///
    /// pi, through an extension of afkconvoy's own, named on the line that starts a pi worker: pi hands it each tool call before making it, it asks this command, and a call this command does not allow — one it refuses, and one it could not be asked of — is not made. A pi worker whose extension is not there does not start.
    ///
    /// Codex, as a hook too, said on the line that starts a Codex worker and kept in no file: Codex runs it before each tool call. Codex makes a call whose hook fails in any way but one, so the hook turns every answer but an allowed call into that one: a call this command refuses, and one it could not be asked of, is not made.
    ///
    /// DSH, through a plugin of afkconvoy's own, which the overlay on the line that starts a DSH worker loads last: it asks this command before every tool call. A DSH worker whose plugin is missing or broken does not start.
    ///
    /// What each harness's worker is started with, the guard among it: [`afkconvoy help worker-launch`](help:worker-launch).
    ///
    /// A role's own limits are hooks in the role's repo — for pi, extensions there. They load because the worker starts there, run beside this one, and refuse in the same way.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing. It reads two things, and a third for a watch that would end a process:
    ///
    /// Standard input: one tool call, as Claude Code describes it to a `PreToolUse` hook — a JSON object with `tool_name` and `tool_input`. Only a `Bash` call is judged, by the words of each simple command of its `tool_input.command`. pi's is described as pi describes it to an extension — `toolName` and `input` — and only a `bash` call is judged, the same way. Codex describes its own as Claude Code does, and a `Bash` call is judged; its `apply_patch` hands the text of a patch as `tool_input.command`, which is no shell line and is not judged as one. DSH describes a call with `name` and `arguments`: `bash` and `bash_persistent` are judged by `arguments.command`. What is refused is decided once, for all.
    ///
    /// The environment: the facts of the worker's stage, which the fleet put there when it started the worker — AFKCONVOY_STAGE, AFKCONVOY_WRITES, AFKCONVOY_CONSULT, AFKCONVOY_BASE_BRANCH, AFKCONVOY_ESCALATE_LABEL, AFKCONVOY_LOCK_LABEL — and, for a selected task destination, AFKCONVOY_TARGET_BRANCH, protected alongside the configured base — and AFKCONVOY_HARNESS, the harness that asks: how the call is described is its own. A watch names its host in AFKCONVOY_WATCH, in place of a stage, and HOME, the home folder it reads paths under.
    ///
    /// The host's processes, as `ps` says them, and the host runner's pid, as its lock says it: whom each process a `kill` names is. The fleets of this host started a process carrying a worker's `TMPDIR` — the link a fleet made for it (`afkconvoy help worker-launch`), or the own one (.afkconvoy-tmp) of a worktree they put a worker in — or one under either, as a gate sets, and every process under one of those, or left behind by one.
    ///
    /// Refused for every worker:
    ///
    /// - input to the asking session's Terminal (`orca terminal send`, including raw text, Enter, interrupts and retried prompts). The current handle comes from the asking launch record, never its title or directory. The host runner's own deliveries run outside worker hooks; the asking session itself is exempt by its runtime Terminal identity. Input to another Terminal is unchanged.
    /// - adding or removing a flow label, a `stage:` label, the escalate label or the lock label (`gh issue edit --add-label`, `--remove-label`, `gh issue create --label`, and any write of an issue's labels through `gh api`)
    /// - writing a stage record by hand: only `afkconvoy verdict` writes one
    /// - writing an intake record by hand: only `afkconvoy intake` writes one, and it is what puts a flow label on for a worker
    /// - writing a work account or a worker start by hand: the fleet writes each, and a worker writes neither
    /// - merging a PR (`gh pr merge`, or the merge endpoint through `gh api`)
    /// - pushing the base branch (`git push` with a refspec that lands on it, `--all`, `--mirror`)
    ///
    /// Refused besides in a stage its workflow marks `writes: false`, for every consult, and for a patrol's worker (AFKCONVOY_PATROL, in place of a stage's facts):
    ///
    /// - committing and pushing (`git commit`, `merge`, `cherry-pick`, `revert`, `am`, `push`)
    ///
    /// A watch is no consult: it is the operator on its host, started so that the fleet's work goes on (`afkconvoy run --help`). It commits, pushes a branch of its own and changes the host, whatever the facts of its role say. It is held instead to the bounds of any unattended worker — refused, beside what every worker is:
    ///
    /// - ending what the fleets of this host did not start: `kill` of a pid that is no worker's, watch's process, nor one they left behind; of the host runner; of what names no pid the guard can read — a variable, a process group; and `pkill` and `killall`, which end whatever a name matches, the user's own among them
    /// - deleting what cannot be rebuilt: `rm`, `rmdir`, `unlink` or `shred` of a path that is, or holds, model weights, downloads, media, Docker's images or the user's own files under the home folder, or of a path outside it but for the system's temporary files; a path it cannot place — relative, or named by another variable — is its own to place
    /// - deleting a change no commit holds: `git clean` without `-X`, `git worktree remove --force`, `git reset --hard`, `orca worktree rm --force`
    /// - a force push: `--force`, `-f`, `--force-with-lease`, or a refspec that begins with `+`
    /// - stopping the host runner, or taking a checkout off its list: `afkconvoy stop`
    ///
    /// ## What it answers
    ///
    /// Unlike every other command, no JSON: it answers Claude Code the way a hook does.
    ///
    /// - exit 0, nothing printed: the call is allowed
    /// - exit 2, a line on stderr: the call is refused; the line is the reason, and Claude Code shows it to the worker — as pi does, given it by the extension, and Codex
    ///
    /// A call it cannot read, an environment that lacks one of the stage's facts, or one that names no harness there is, is refused with the reason: nothing is allowed unread.
    ///
    /// ## What trips you up
    ///
    /// - Heredoc bodies and quoted arguments are data, not commands: a report may name `rm` or `git worktree remove`, and a read-only `gh` call may search for stage or intake records with `--jq` or `--template`. Sending a handwritten record through a writing `gh` call is still refused, including through a heredoc. The argument after `sh -c` or `bash -c` is executable shell text and is judged as commands.
    /// - It recognises commands. The same act wrapped in a script, hidden in a variable or in a substitution inside quotes gets past it: it stops a worker that errs, not one that means to get by.
    /// - What it misses is checked after the stage: a commit left on the branch by a stage marked `writes: false`, or by a consult, is a failure, and the issue escalates; a patrol's final commit not on the base branch, working-tree changes, or a PR is a failed run, reported to a human — see `afkconvoy tick --help`.
    /// - Unbound workers and the watch carry only the asking Terminal input guard. The opaque launch command stays intact; the harness adds its hook, extension or plugin. The asking session starts without this guard.
    #[command(next_help_heading = HOOK, verbatim_doc_comment)]
    Guard {
        /// Check only asking Terminal input, outside a bound stage
        #[arg(long)]
        terminal_input_only: bool,
    },
    /// Start a fresh worker on one issue
    ///
    /// ## When to run it
    ///
    /// To start a worker on one issue by hand. A tick does this on its own for every frontier issue it has a free slot for, and for every claim of its own left without a worker. Do it yourself when a plan shows an issue the fleet is not starting, or — with `--start fresh` — to throw away an attempt that went wrong and have the issue worked again. Run `recovery` first: it shows what this would continue from.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker is to run on.
    ///
    /// ## What it reads and writes
    ///
    /// Before it claims an issue the instance does not hold, it reads the issue's blockers on GitHub once more: with any of them open nothing is claimed and no worker starts. A tick's rebuild can be minutes older than its dispatches, and a blocker added in between holds the issue back all the same.
    ///
    /// It claims the issue for `--instance` unless that instance holds it already, looks at what survives of an earlier attempt, has orca put a worktree there, records the worktree's branch in a comment on the issue, writes the worker's brief into the worktree, starts the worker launch command in it — under `nice`, with the worktree's own `TMPDIR` — and hands the worker its prompt.
    ///
    /// In a bound repo the issue's flow label names its workflow, and the worker performs the stage the issue's stage records put it at — the workflow's first while there are none — in the stage's role, with the verdicts declared so far, and the reason of a rejection, in its brief. The team's repo and each role's are asked for the commit at their head; one the fleet has no copy of yet is cloned and kept, read-only, under the cache directory (copies/<owner>/<repo>/<commit>; `afkconvoy help cache` says where that directory is and what moves it), once however many roles it holds. A role that is a subdirectory of its repo is copied out of that on its own (<commit>-role-<path>; <commit>-role+codex-<path>, with a link to the role's skills where Codex finds them, for a Codex worker), and so is the team's directory, without the directories of its roles and without a `.claude/` at its top (<commit>-team+rules-<path>): a worker is given its role's directory and, of the team's, the team rules (its `CLAUDE.md`) — nothing else above its role, and no neighbour's.
    ///
    /// The terminal is opened on the worktree as ever, and the line it runs changes to the role's copy, puts the stage's facts in the environment, and starts the worker launch command with what puts the worker in its role: [`afkconvoy help worker-launch`](help:worker-launch) says what, for each harness. The worker's harness is the first eligible one in this host's kit order within its role's range. Its model is the issue's one model label of that harness, or that harness's user default (`afkconvoy help roster`).
    ///
    /// Where the stage declared `escalate` and names a consult, the worker is started as that consult instead: in the consult's role, on a brief that names the skill, the stage that asked and its reason, and the two lines it ends with — `afkconvoy rule`, or `afkconvoy verdict escalate` to let it through to a human. AFKCONVOY_CONSULT is `true` in its environment, the guard refuses its commits, and it runs its own role's range, not the stage's.
    ///
    /// Once the issue's last stage has passed to `land`, the worker is started in the workflow's landing role instead, on the landing brief with that role's landing skill named in it: it gates the issue's open PR and lands in a turn. Its stage is `land`; when it cannot proceed it may declare `blocked` or `escalate` (`afkconvoy verdict --help`).
    ///
    /// ## What it answers
    ///
    /// {"issue", "started": true, "claim", "tier", "action", "prompt", "reason", "worktree", "branch", "terminal"}
    ///
    /// - `claim`: won (claimed now) or held (the instance's already)
    /// - `tier`, `action`: 1 reuse_worktree — the worktree is still on this host; 2 recreate_at_tip — a new worktree at the issue's pushed branch; 3 dispatch_fresh — a new worktree at the base branch
    /// - `prompt`: the brief the worker got: fresh, continue, land (its PR is open: only the landing is left), or take_up (its PR is open and no claim held it: an escalation or a park kept it — take its landing turn) — in a bound repo, stage, consult, or land for a landing worker
    /// - `stage`: in a bound repo: {flow, name, role, skill, writes, consult, harness, model, role_commit, role_dir, team_commit, team_dir} — the stage the worker performs, and the copies and commits fixed for it; name is `land` for a landing worker. For a consult (`consult: true`), name is the stage that asked, and role and skill are the consult's
    /// - `reason`: why that tier, in words
    /// - `discarded`: with `--start fresh`: {closed_prs, deleted_branches, removed_worktree}
    ///
    /// {"issue", "started": false, "claim": "lost", "owner"} — another instance holds the claim. Nothing was started.
    ///
    /// {"issue", "started": false, "skipped": "blocked", "blocked_by"} — the issue has an open blocker: blocked_by lists them, lowest number first. Nothing was claimed and nothing was started.
    ///
    /// Errors: the issue is closed; in a bound repo, the issue carries no flow label, more than one, or one that names no workflow of the team, it is at no stage a worker performs (the error says where it is), it passed to `land` and no open PR closes it, the team does not load (the error carries its `faults`) — none of which claims the issue; the instance's claim on it is one a stage already declared under; the worker launch command resolves to nothing; orca fails, or the worker is not ready in time. After a failed start the claim is still held: until the worker takes its brief, the instance's next tick finds a claim with no worker and starts it again in the same worktree. A second failed start escalates and releases the claim, naming the start errors. A start held up only by the previous worker's terminal, still there after orca acknowledged closing it, is no failed start: it fails no tick, keeps the claim, and is tried again each tick until the terminal is gone; still there 300 seconds after the first such start, it escalates as a failed start, saying how long it was waited for. No nudge or silence judgment is made of an incomplete start.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and it is the instance that holds the claim afterwards. Give it the id of the fleet that is to look after the worker.
    /// - `--start fresh` discards the previous attempt: it closes the issue's open PRs, deletes its branches on the remote and removes its worktree, unpushed work included.
    /// - It checks that the issue is open and, unless the instance holds it already, that no blocker of it is open — nothing else of the frontier: an issue without the ready label, or past the concurrency, is started all the same. A claim the instance holds is continued whatever blocks it.
    /// - A role in a subdirectory is not the directory a human opens in a checkout: a `CLAUDE.md` or a `.claude/` above it there is not in the worker's copy, nor above it — but for the `CLAUDE.md` of the team's own directory, the team rules, which every worker is given. `afkconvoy check` notes each role that has another above it.
    /// - It does not ask after a worker already there. On a claim whose worker is at work it starts another in the same worktree.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// Before `dispatch` on an issue that was worked before: it shows what `dispatch` would continue from, and so what `--start fresh` would throw away.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker ran on — a worktree counts only where it is.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the issue's comments and the open PRs on GitHub, the issue's branches on the remote, and its worktree through orca. It changes nothing.
    ///
    /// ## What it answers
    ///
    /// {"issue", "base", "worktree", "branch", "tier", "action", "prompt", "reason"}
    ///
    /// - `worktree`: {present, path, commits_ahead, dirty} — what is on this host
    /// - `branch`: {name, candidates, commits_ahead} — the issue's own branches on the remote, and the one a worktree would be recreated at
    /// - `tier`, `action`, `prompt`, `reason`: what `dispatch` would do, as `dispatch` answers it
    ///
    /// Errors: the config, GitHub or the remote cannot be read; the remote has no base branch; there is no such issue (`no such issue: #7 (HTTP 404)`), or GitHub says it was deleted or transferred.
    ///
    /// ## What trips you up
    ///
    /// - It takes no `--instance` and looks at no claim: it answers for an issue nobody holds, and for one another instance holds.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
    Recovery {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Close one of my claims whose issue needed no change
    ///
    /// ## When to run it
    ///
    /// To close, by hand, a claim whose issue needed no change. A tick does this on its own for a worker that stopped with an `already-satisfied` verdict and no work on its branch. Do it yourself when a plan keeps showing such a claim under `mine` as `no_pr`.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker ran on.
    ///
    /// ## What it reads and writes
    ///
    /// It closes the issue on GitHub, writes the status board's last word, releases the claim on the remote, and removes the worktree.
    ///
    /// ## What it answers
    ///
    /// {"issue", "action": "closed", "released": true} {"issue", "released": true, "detail"} — no claim is held on the issue: nothing was done.
    ///
    /// Errors: the claim is another instance's ("is not this fleet's claim"); the worktree shows work, or there is no worktree on this host to show there is none ("is not closed as needing no change"). Nothing was changed.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claim.
    /// - It does not read the verdict. Any claim of the instance whose branch has no commits past its merge base with the remote base branch, has the same tree as that merge base and has nothing uncommitted is closed — one whose worker has only just started as well. A clean branch only behind the base branch qualifies. A refusal names files the branch changed since the merge base and anything uncommitted.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
    Close {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Hand one of my claims to a human
    ///
    /// ## When to run it
    ///
    /// To hand a claim to a human by hand. A tick does this on its own for every failure: an `escalate` verdict, a worker silent past its nudge, a blocker that cannot be parked on. Do it yourself when you can see a claim will not finish and the fleet has not given it up yet.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// On GitHub it comments `--reason` on the issue, swaps the config's `ready_label` for its `escalate_label`, and writes the status board's last word. In a bound repo it swaps the issue's flow label for the `escalate_label` of the team instead, and the comment records which workflow the issue left: `afkconvoy rule` puts that label back. On the remote it releases the claim. The PR and the branch are kept. The worktree is removed when its tree is clean and every commit of its branch is on the remote — pushed, or on the base branch: a later dispatch recreates it at the pushed tip. Otherwise it is kept, and the comment and the status board say what it holds that exists on this host only, the comment listing each commit the remote does not have by its subject. In orca it closes the terminal of the issue's worker: a worktree that is kept has no worker left in it.
    ///
    /// ## What it answers
    ///
    /// {"issue", "action": "escalated", "released": true, "worktree", "kept_because", "unpushed"} — worktree is "removed", "kept", or "none" when the issue has none on this host. A kept one says why: kept_because is "dirty", "unpushed" or "unread", and unpushed is how many commits only it holds (null when unread).
    ///
    /// {"issue", "released": true, "detail"} — no claim is held on the issue: nothing was done.
    ///
    /// Errors: the claim is another instance's ("is not this fleet's claim").
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claim.
    /// - It escalates only a claim. On an issue nobody holds it answers `released` with a `detail`, and has neither commented nor relabelled.
    /// - Cut short, run it again with the same reason: it goes on from the step it stopped at, and still posts one comment.
    /// - It stops no worker: one still at work goes on, on an issue the fleet gave up.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// To park a claim by hand. A tick does this on its own for a worker that stopped with a `blocked` verdict naming open issues the fleet can wait for. Do it yourself when a plan keeps showing such a claim under `mine` as `no_pr`.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the worker's latest verdict on the issue and where each issue it names stands. On GitHub it records each blocker as a blocked-by edge of the issue and writes the status board's last word. On the remote it releases the claim. Then the worktree: when its tree is clean and every commit of its branch is on the remote — pushed, or on the base branch — it is removed, and a later dispatch recreates it at the pushed tip. Otherwise it is kept, with the terminal of the issue's worker closed, and the status board says what it holds that exists on this host only.
    ///
    /// ## What it answers
    ///
    /// {"issue", "action": "parked", "blocked_by", "released": true, "worktree", "kept_because", "unpushed"} — blocked_by is the issues it waits on; worktree is "removed", "kept", or "none" when the issue has none on this host. A kept one says why: kept_because is "dirty" (something uncommitted), "unpushed" (commits the remote does not have) or "unread" (its commits could not be checked against the remote), and unpushed is how many commits only it holds (null when unread).
    ///
    /// {"issue", "released": true, "detail"} — no claim is held on the issue: nothing was done.
    ///
    /// Errors, each with nothing changed: the claim is another instance's; "is not parkable" — the latest verdict is not `blocked`, or every blocker it named is closed (dispatch the issue again instead), or a blocker cannot be waited for (escalate it instead: the error says why).
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claim.
    /// - It takes no blockers of yours: they are the ones the worker's verdict names.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
    Park {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Tell the worker of one of my claims, stopped with no PR and no verdict, to carry on
    ///
    /// ## When to run it
    ///
    /// To send a worker its nudge by hand. A tick does this on its own only when worker health or the capacity rule asks the worker to carry on; silence alone starts observation, and what no rule classes goes to Jev, where the host settings name a key command, or else to the operator. Do it yourself when you have checked the worker’s scene and want it to carry on.
    ///
    /// ## What counts as silent
    ///
    /// A stopped worker with no verdict or progress is observed over at least 300 seconds. Actual terminal output, git progress, runtime reports and process activity are evidence, never proof of quota exhaustion by themselves. Structured error codes and a harness's explicit usage-limit and task-refusal messages are classed by rule: Codex's `Usage limit reached … Increase your limits to continue using codex` is an account quota fault, with no observation delay. Codex's `We take extra care with some cybersecurity requests. If you're doing authorized security work, apply for Daybreak to get broader access.` is a refusal of this task by rule, also with no observation delay. This issue continues on the next harness in host kit order that can start for its role. The issue remembers each refusing harness on GitHub and skips it at every later stage and start; the harness stays healthy for other issues. With none left, the issue escalates at once with the refusal line as evidence and no consult. What no rule classes is judged by one Jev request on the worker's latest screen where the host settings name a key command; without one, or where the key command, TypeSafe or Jev's answers settle nothing, the worker waits for the operator and its scene is preserved.
    ///
    /// Account quota with no reset time continues on the next harness in host kit order that can start for the worker's role. A reset within the one-hour recovery window may be waited for; a later reset continues instead. Repeated waits on unchanged evidence never restart that window. After one hour the fleet acts on the fault class or escalates the issue to a human with its evidence. Doctor names the worker, its class and its next check, and a worker waiting for the operator with the reason and a line of its screen.
    ///
    /// Codex's `Selected model is at capacity. Please try a different model.` is a capacity stop by rule, also with no diagnosis or observation delay. The first stop waits {{CAPACITY_WAIT_SECONDS}} seconds before nudging the same session; the next waits {{CAPACITY_SECOND_WAIT_SECONDS}} seconds. After {{MAX_CAPACITY_STOPS}} stops in the episode, the worker continues on the next eligible harness in host kit order, keeping its worktree, branch, stage, claim and PR. The episode survives brief working turns and runner restarts, ending after {{CAPACITY_HEALTHY_SECONDS}} seconds observed healthy or a different session. Capacity faults no harness for other workers. Tick-log worker-health lines name the rule, waits and nudges; doctor's host kit reports capacity_stops per harness, retained after episodes end.
    ///
    /// Confirmed background work, waiting for a human or dependency and completed idle sessions are left alone. A worker's gate waiting in the exclusive queue is background work too. The tick does not turn a long wait into permission for another writer. Continuation closes the old agent and confirms its terminal, runtime and process tree are gone before starting the successor in the same worktree, branch, stage, claim and PR. Structured transient rate limits respect Retry-After and get two retries before session fallback. Account and model recovery probes run hourly, in the login shell a worker gets, and open no terminal; successful recovery affects new starts without moving healthy successors back.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker runs on.
    ///
    /// ## What it reads and writes
    ///
    /// It asks orca for the worker's state and reads the issue's comments for a verdict. Immediately before sending, it checks that the terminal still has a harness ready for a prompt: a bare shell receives nothing. It types one line at the worker's terminal and records the nudge in the worktree. Nothing on GitHub or the remote changes.
    ///
    /// ## What it answers
    ///
    /// {"issue", "action": "nudged", "terminal"}
    ///
    /// Errors, each with nothing sent: "has no stopped worker" — the instance holds no claim on the issue, or its worker is at work or gone; "is not nudged" — it was nudged already, showed a sign of life within the grace period, waits on background work of its own, or declared a verdict; the claim is another instance's; the worktree has no live terminal.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claim.
    /// - It is refused wherever a tick would not nudge: it cannot be used to hurry a worker.
    /// - One nudge per worker. A worker silent again after it is escalated by the next tick.
    /// - To tell a worker something it must know, busy or stopped, use `afkconvoy tell`: a nudge only says carry on.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
    Nudge {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Tell the worker at work on an issue something it must know, busy or stopped
    ///
    /// ## When to run it
    ///
    /// When the worker at work on an issue must know a decision or correction now. Each tick also tells it later issue comments and body edits automatically. Use it rather than `nudge`, which is the fleet's own line to a stopped worker telling it only to carry on: this is the operator's words, to a worker busy or stopped.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker runs on. Name the issue, never an orca terminal.
    ///
    /// ## What it reads and writes
    ///
    /// It posts the note as a comment on the issue first: that comment is the note's one home, which a continuation, a restart or a later stage's worker reads in its brief's first step. Rerun for the same note, it finds that comment and posts no second one.
    ///
    /// It then finds the worker at work under this host's claim of the issue, whichever fleet instance of the host holds it, and types one line at its terminal: the note, on one line, with the comment's link. At a worker that is stopped it first checks that the terminal has a harness ready for a prompt: a bare shell receives nothing. At a busy worker it types into the harness's input. Claude Code gives a line typed while it works to its model at the next step. A busy Grok keeps that line in its input queue; once the screen shows the line queued, tell presses send now, which gives it to the model at the next step. A harness that does neither waits, bounded, until its turn ends.
    ///
    /// It is told only once the harness's session record holds the line as a message its model was given as the user's. A line seen only in the harness's input queue is not delivered: a harness stopped or dead with it there drops it. It records the delivery in the worktree, beside the nudge, and in the tick log.
    ///
    /// ## What it answers
    ///
    /// {"issue", "action": "told", "terminal", "comment", "seconds"} — seconds from the send to the session record's user entry; "typed": false instead where a rerun found the line already taken.
    ///
    /// With no worker at work under this host's claim — the issue is not claimed, its worker has not taken its brief, it has no worktree here (not started, or between stages), no live terminal, or another host holds the claim — it posts the comment, types nothing, and answers {"issue", "action": "commented", "comment", "why"}, with "host" naming the host that holds the claim where another one does.
    ///
    /// Errors: past the bound (600 seconds) with the line not taken, it fails with the current screen and the comment's link, and types no second line; rerun it to try again. A terminal with no harness at its input is sent nothing.
    ///
    /// ## What trips you up
    ///
    /// - It never edits the issue's body. Each tick tells later body edits and issue comments to the worker under this host's claim, with session proof, and retries failed deliveries. A line the worker's terminal took but its harness has not given the model yet (a harness running a tool hands over queued input when the tool ends) is recorded queued, no failure, and told by the tick that finds the session holding it; still not taken past tell's bound (600 seconds), it fails each tick. A refused line, or a harness at no input, fails at once. Three consecutive failed ticks make a doctor finding naming the issue and comment. Fleet comments carrying a `<!--convoy:` marker and comments the worker itself posted (identified by its session's write receipts) are skipped. With no worker here, the next worker reads the issue at its start. PR comments are not told, and closed issues stay closed.
    /// - It takes no `--instance`: the claim says whose worker it is, and only a worker on this host can be typed at.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// To replace a silent worker by hand. A tick does this on its own, once, for a claim whose PR is open and not on the base branch, and whose worker stopped, was nudged, and stayed silent. Do it yourself when a plan keeps showing such a claim under `mine` as `landing`.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker runs on.
    ///
    /// ## What it reads and writes
    ///
    /// It records the restart in the fleet's comment on the PR, then starts a new worker in the same worktree, briefed only to land in a turn — in a bound repo, in the landing role of the issue's workflow. The PR, the branch and the worktree are untouched.
    ///
    /// ## What it answers
    ///
    /// {"issue", "pr", "action": "restarted", "restarted", "terminal", "prompt"} — restarted is the time, in epoch seconds; prompt is the brief the new worker got.
    ///
    /// Errors, each with nothing changed: "has no stopped worker" — the instance holds no claim on the issue, its worker is at work or gone, or the issue has no open PR; "is not restarted" — the PR's landing worker already had its restart, the worker was not nudged yet, or it showed a sign of life within the grace period; the claim is another instance's.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claim.
    /// - One restart per PR. A worker silent again after it is escalated by the next tick.
    /// - It is not `dispatch`: for a claim with no PR, continue it with `dispatch`.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// `afkconvoy switch --harness codex` moves Codex to the front of the host kit. Run it anywhere, with no repo or instance required. A checkout that names no harness follows this order. A checkout that names a harness keeps that name: switch does not change it, and does not continue that checkout's workers onto the kit's new first. `--all` skips those checkouts. To change priority again, switch again. The kit itself has no separate choice to clear. A checkout's harness is cleared with `afkconvoy run <checkout> --harness default`.
    ///
    /// ## Who runs it
    ///
    /// The operator, on the host whose kit and workers are being changed.
    ///
    /// ## What it reads and writes
    ///
    /// A known harness that is installed but absent is added first on its stock worker launch command, with no model, so workers start on its login's default at once. This includes one the operator removed with `afkconvoy kit --remove`: the switch clears its removal and puts it first, after which relisting, daily refresh and launch discovery keep it as an ordinary entry. The answer comes within seconds, before that harness is listed or researched. Only after the answer does a background refresh list it and hand its listing to research, preserving the order this switch set. No model is sent a prompt: newly listed models are `research pending`, and its workers stay on the default until research places them with dated sources. If the added harness cannot start a worker, the answer's start_fault says why and doctor names the same fault. Unknown or uninstalled harnesses, and existing entries that cannot start, are refused before the kit changes.
    ///
    /// `afkconvoy switch --model sonnet` accepts any listed id or researched name of an existing placed model, moves it first within its harness and moves that harness first. A name matching several harnesses or models is refused with candidates; an unknown name is refused with nearest names, leaving the kit unchanged. Repeating a switch is idempotent. Commands, model contents, research and the relative order of other models and harnesses stay intact. Rebuilds, daily refresh and new-model research preserve these orders; new discoveries follow established models. Ticket labels and team rosters are untouched. Role limits still constrain candidates. Paused candidates are skipped without reordering the kit, and become eligible again after a successful recovery probe. A switch with no target leaves running workers alone.
    ///
    /// --all continues workers across every checkout on this host's checkout list that names no harness. A checkout that names one is left as it is, workers included. ISSUE or --patrol on a checkout that names one records nothing moved. --instance may identify the owner of a claim in a checkout that names none. Each continuation keeps its claim, stage, worktree, branch, uncommitted work, PR and breaker counts. Its brief includes the previous output. A failure is reported and other workers continue. An old worker that cannot be confirmed stopped is never given a second writer. Every moved worker's answer and tick-log line name every harness ahead of its actual successor, why it was skipped, and what would make it eligible. To remove the harness those workers are on, move them first with `afkconvoy switch --all --harness <harness>`; `kit --remove` refuses while any live fleet worker remains on it and never ends one.
    ///
    ///
    /// ## What it answers
    ///
    /// {harness, model, path, order_before, order, changed, added, restored, start_fault, switched, failed}. model is the promoted id, or null for a harness-only switch; added says whether a missing harness was inserted; restored says whether this switch cleared an operator removal. Each moved worker has {from, to, skipped}, where skipped holds {harness, why, fix} for every earlier candidate. Each failure names its checkout and worker. A successful fallback reports the actual harness.
    ///
    /// ## What trips you up
    ///
    /// --all is machine-wide, even when --instance is supplied. --clear is removed: switch to the harness you want first instead. Old repository choice files are ignored. Only `switch --harness` restores an operator-removed harness; a pause, probe, relisting, daily refresh or launch discovery never does.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// When a fleet instance on another host is dead and its claims should not wait out the lease. `--list` first, to find its id and see whether its heartbeat is fresh; then `--from <id>`. For this host's own claims it is not needed: `run` takes them at its start. A claim is on an issue, a patrol's run or a cut: both ways take all of them.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host that is to carry the claims on.
    ///
    /// ## What it reads and writes
    ///
    /// `--list` reads the claims and heartbeats on the remote, and changes nothing. `--from` re-stamps each of that instance's claims on the remote in the name of `--instance`, and refreshes that instance's heartbeat when it is due. Each push is refused if the claim moved since it was read. A claim taken is still the claim it was created as: a verdict declared under it before it was taken is read as its. It starts no worker and changes nothing on GitHub.
    ///
    /// ## What it answers
    ///
    /// `--list`: {"instances", "me", "now", "lease"}
    ///
    /// - `instances`: one per instance the remote remembers: {instance, host, claims, runs, claim_count, heartbeat_ts, heartbeat_age, fresh, is_me}. claims: the issues it holds a claim on. runs: the runs it holds one on, by key — `patrol-<name>` a patrol's, `cut` a cut's. claim_count: both. fresh: its heartbeat is within the lease
    /// - `me`: `--instance`, when given
    /// - `lease`: the lease, in seconds
    ///
    /// `--from`: {"action", "instance", "taken", "lost", "taken_runs", "lost_runs", "detail", …}
    ///
    /// - `action`: taken — with "as", the instance they were taken as confirm — its heartbeat is fresh: nothing was taken. With "heartbeat_age" none — that instance holds no claims, or no instance has that id
    ///
    /// - `taken`: the issues whose claims were taken
    /// - `lost`: the ones that moved first: that fleet is not dead
    /// - `taken_runs`: the runs whose claims were taken, by key as `--list` gives them
    /// - `lost_runs`: the ones that moved first
    ///
    /// Errors: `--from` without `--instance`; neither `--list` nor `--from`; `--from` is `--instance` itself.
    ///
    /// ## What trips you up
    ///
    /// - `--from` an instance whose heartbeat is still fresh takes nothing until `--yes`: the answer is `confirm`, with exit 0. Add `--yes` only when you know that fleet is dead. The old host's next tick stops its workers on claims taken to another host, and their verdict, land and gate commands refuse.
    /// - Taking starts nothing. The claims are continued by the next tick of `--instance`: follow it with `run --instance <id>` or `tick --instance <id>`, under the same id.
    /// - A run taken is not continued: that tick ends it — its worker is on the host that died — and the patrol is due again by its interval, the ruling not proposed again.
    /// - `--list` is the way to learn a running fleet's instance id.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
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
    ///
    /// ## When to run it
    ///
    /// To see exactly what the remote holds: which instance claims which issue, and when each instance last beat. `plan` and `takeover --list` say what that means; this is the refs themselves, for repairing them.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the fleet's refs on the remote. It changes nothing, and does not read GitHub or the config.
    ///
    /// ## What it answers
    ///
    /// {"claims", "heartbeats"}
    ///
    /// - `claims`: one per claimed issue, lowest number first: {number, instance, host, ts, sha}. ts is when it was stamped, in epoch seconds; sha is what a take or a release of it is leased to
    /// - `heartbeats`: {<instance id>: <epoch seconds>}
    ///
    /// Errors: the remote cannot be read; "is not in a git checkout", or git's own words for a checkout it refuses; "the tracker is not said" — the checkout is on no checkout list of this host, and the remote holds claims named for another tracker, which would read as no claim.
    ///
    /// ## What trips you up
    ///
    /// - It looks where this repo's last startup probe found the remote keeps the fleet's refs. In a clone no `plan`, `tick` or `run` was ever run in, it looks under refs/convoy/ — and finds nothing on a remote that keeps them as convoy-* branches.
    /// - It does not say whether a claim is live: compare its instance's heartbeat with the lease, or read `takeover --list`.
    #[command(next_help_heading = PLUMBING, verbatim_doc_comment)]
    Scan {
        #[command(flatten)]
        at: At,
    },
    /// Claim one issue for a fleet instance
    ///
    /// ## When to run it
    ///
    /// Hardly ever: `dispatch` claims the issue it starts a worker on. Run it to hold an issue back from every fleet — a claim keeps the issue off the frontier — or to repair a claim that was released by mistake.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It creates the issue's claim on the remote in one atomic push, which succeeds for exactly one instance. Before it, it refreshes the instance's heartbeat when that is due. It does not read GitHub: the issue is not checked, and no worker is started.
    ///
    /// ## What it answers
    ///
    /// {"won": true, "issue", "ref", "sha", "instance"} {"won": false, "issue", "ref", "owner", "detail"} — the issue is claimed already. owner is that claim: {number, instance, host, ts, sha}.
    ///
    /// Errors: the push failed and it was not a lost race; the remote cannot be read.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required. A lost race is an answer with exit 0: read `won`.
    /// - A claim the instance holds already answers `won: false`, with itself as the owner.
    /// - A claim is live only while its instance's heartbeat is within the lease. Under an id nothing beats for, another fleet takes it as stale once the lease runs out.
    /// - Claimed as a running fleet's instance, it is a claim with no worker: that fleet's next tick starts a worker on it.
    #[command(next_help_heading = PLUMBING, verbatim_doc_comment)]
    Claim {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Release a fleet instance's own claim on one issue
    ///
    /// ## When to run it
    ///
    /// Hardly ever: `close`, `escalate` and `park` release the claim they end, and a landed PR's claim is released by the tick. Run it to drop a claim made with `claim`, or one left behind by something that went wrong.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It deletes the issue's claim on the remote, only while it is still the claim that was read. It does not read or change GitHub, and touches no worker and no worktree.
    ///
    /// ## What it answers
    ///
    /// {"released": true, "issue", "ref"} — also when the issue had no claim: already gone counts as released.
    ///
    /// Errors: the claim is another instance's ("is not this fleet's claim"); it "moved since it was read" — somebody took it meanwhile, and it was left alone; the delete failed; "the tracker is not said" — the checkout is on no checkout list of this host, and the remote holds claims named for another tracker, so the claim's ref cannot be named.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claim. To end another instance's claim, take it first: `takeover`.
    /// - It only unlocks. The issue keeps its labels, its worker keeps working, and — still open and ready — the issue is back on the frontier: a fleet dispatches it again.
    #[command(next_help_heading = PLUMBING, verbatim_doc_comment)]
    Release {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Refresh a fleet instance's heartbeat, if it is due
    ///
    /// ## When to run it
    ///
    /// Hardly ever: every tick refreshes its instance's heartbeat. Run it to keep the claims of an instance live while nothing ticks for it — claims made by hand, or a fleet stopped for a while whose claims no other host should take.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the instance's heartbeat on the remote and, when it is due — never written, or older than a third of the lease — writes it anew. Nothing on GitHub changes.
    ///
    /// ## What it answers
    ///
    /// {"refreshed": true, "ts", "ref"} {"refreshed": false, "reason": "not due", "ts", "ref"} — ts is the heartbeat that stands, in epoch seconds.
    ///
    /// Errors: the remote cannot be read, or refuses the push.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required.
    /// - It keeps a dead fleet's claims alive: while they are beaten for, no other host takes them as stale, and nobody works them.
    /// - One beat lasts one lease. Kept up by hand, it has to be run again before that.
    #[command(next_help_heading = PLUMBING, verbatim_doc_comment)]
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

/// The command line as parsed: `Cli`, each command about one repo with how it
/// takes its checkout added to its text, and each command that acts with what
/// it logs.
fn cli() -> clap::Command {
    let mut cli = Cli::command();
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
        .constant("CONCURRENCY", afkconvoy_core::config::CONCURRENCY)
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
        .constant("DOCTOR_EVENTS_SHOWN", doctor::EVENTS_SHOWN)
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
    let parsed = cli()
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
        Command::Skill => Ok(Value::String(SKILL.to_string())),
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
            let repo = checkout?;
            let launches = Launches::foreseen(&repo).with_no_proxy(&proxy.worker_no_proxy)?;
            let warnings = match probe::standing(&repo, now, &launches) {
                Some(warnings) => warnings,
                None => probe::startup(&repo, now, &launches)?,
            };
            probe::announce(&warnings);
            // `plan` is one look: its host reading is the report's.
            let readings = admission::Readings::new();
            let (mut fleet, open) = Fleet::read(&repo, now)?;
            let (mut set, flows) = fleet.rebuild(&open, instance.as_ref())?;
            let report = fleet_report::FleetReport::read(&fleet, &flows, &set, &readings);
            if let Ok(admission) = &report.admission {
                set.held_back = admission.holds();
            }
            let findings = blockers::findings(&set.unworked_blockers);
            let mut plan = serde_json::to_value(set)?;
            plan["findings"] = serde_json::to_value(findings)?;
            if let Some(plan) = plan.as_object_mut() {
                plan.extend(report.plan_json()?);
            }
            plan["need_installs"] = json!(
                probe::installs(&fleet.repo, &fleet.config)
                    .into_iter()
                    .map(|(kind, installed)| json!({
                        "kind": kind,
                        "check": installed.check.name,
                        "at": installed.at,
                        "seconds": installed.seconds,
                        "status": installed.status,
                        "passed": installed.passed,
                        "stderr": installed.stderr,
                    }))
                    .collect::<Vec<_>>()
            );
            plan["targets"] = fleet.targets(&open.issues)?;
            plan["host_kit"] = kit::show(now);
            plan["selection"] = fleet.selection();
            plan["codex_workers"] = json!(codex_launch::workers(&fleet.repo));
            report.put_launches(&mut plan["frontier"]);
            if let Ok(team) = flows.team() {
                plan["selection"]["roles"] = json!(
                    team.roles.iter().map(|role| fleet.role_selection(role)).collect::<Vec<_>>()
                );
            }
            plan["worker_no_proxy"] = launches.proxy_json();
            Ok(plan)
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
                blocked_by,
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
                    blocked_by: vec![],
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
            let dispatched =
                dispatch::dispatch(&mut fleet, &who.me(now), &agent, issue.number(), start);
            probe::announce(&agent.launches.warned());
            dispatched
        }
        Command::Recovery { issue, .. } => {
            recovery::recovery(&Fleet::open(&checkout?, now)?, issue.number())
        }
        Command::Close { issue, who, .. } => {
            judge::close(&mut Fleet::open(&checkout?, now)?, &who.me(now), issue.number())
        }
        Command::Escalate { issue, who, reason, .. } => {
            let mut fleet = Fleet::open(&checkout?, now)?;
            judge::escalate(&mut fleet, &who.me(now), issue.number(), &reason)
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
