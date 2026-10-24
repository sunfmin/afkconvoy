//! The effect layer: reads the world through `git`, `gh` and `orca`, asks the
//! core crate what to do, and does it (docs/adr/0007).

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use afkconvoy_core::claims::InstanceId;
use afkconvoy_core::config::{RepoDir, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_sleep::IDLE_WAIT;
use afkconvoy_core::inspection::{Found, INSPECTION_ENV};
use afkconvoy_core::line::Line;
use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::position::Declared;
use afkconvoy_core::role::{
    CONSULT_ENV, HARNESS_ENV, ISSUE_ENV, MODEL_ENV, ROLE_COMMIT_ENV, ROLE_ENV, STAGE_ENV,
    TEAM_COMMIT_ENV,
};

use anyhow::Context;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::{Value, json};

pub mod abandon;
pub mod acting;
pub mod admission;
pub mod advance;
pub mod board;
pub mod caster;
pub mod checkout;
pub mod checkout_list;
pub mod checkouts;
pub mod claim_status;
pub mod claims;
pub mod concurrency;
pub mod config;
pub mod copies;
pub mod dispatch;
pub mod doctor;
pub mod draft;
pub mod evidence;
pub mod exclusive;
pub mod fleet;
pub mod flows;
pub mod forget;
pub mod gate;
pub mod gh;
pub mod git;
pub mod guard;
pub mod harness_down;
pub mod host_evidence;
pub mod host_runner;
pub mod host_settings;
pub mod host_sleep;
pub mod hot;
pub mod inspection;
pub mod instance;
pub mod intake;
pub mod judge;
pub mod kept;
mod kit;
mod kit_research;
pub mod landing;
pub mod launch;
pub mod lock;
pub mod log;
pub mod measure;
pub mod opening;
pub mod orca;
pub mod panel;
pub mod park;
pub mod patrol;
pub mod paused;
pub mod precedent;
pub mod probe;
pub mod reclaim;
pub mod recovery;
pub mod role;
pub mod roster_issue;
pub mod runner_socket;
pub mod scaffold;
pub mod serve;
pub mod settle;
pub mod slow_gate;
pub mod stage;
pub mod steer;
pub mod stop;
pub mod switch;
pub mod takeover;
pub mod team;
pub mod tend;
pub mod tick;
pub mod tools;
pub mod topic;
pub mod train;
pub mod wake;
pub mod watch;
pub mod workflow;

use acting::Acting;
use claims::{Me, Scan, Take};
use dispatch::Agent;
use fleet::Fleet;
use git::Repo;
use host_settings::{
    ADMIT_DISK_ENV, ADMIT_INSPECT_AFTER_ENV, ADMIT_LOAD_ENV, ADMIT_MEMORY_ENV,
    HOST_SLEEP_HOURS_ENV, HOT_CORES_ENV, HOT_MEMORY_ENV, HOT_WRITTEN_ENV, NO_HOST_SLEEP_ENV, To,
    hours_or_default, port_or_default, thousandths_or_default, whole_or_default,
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
// manual. The listing goes straight from the usage line to the commands: one
// line less, so that `-h` stays one screen.
/// afkconvoy works a repo's ready GitHub-issue backlog unattended. A fleet instance — one per repo, held by the host's one host runner, which `afkconvoy run` starts — dispatches one worker per issue, a coding agent isolated in its own git worktree, and lands each finished PR on the base branch through the landing train, behind the gate. Fleet state lives entirely in GitHub and the repo's remote: no process holds it.
///
/// `afkconvoy --help` answers the index: this guide, the way in; then every command by its one line, under who runs it; then every topic by its first sentence, what belongs to no one command. `afkconvoy help all` answers the whole manual: this guide, then every command and every topic in full. `afkconvoy -h` lists the commands on one screen.
///
/// ## What it needs
///
/// - A config on the base branch: the first yaml block of {const:CONFIG_FILE}.
/// - `gh` logged in as a user who may push to the repo.
/// - `orca` on the PATH: every worker is started through it. It is running (`orca open`) and was given this checkout, once (`orca repo add --path "$PWD"`).
/// - `claude` on the PATH and logged in, when it is what starts a worker.
/// - `pi` there too, holding credentials for a provider (`/login` at its prompt), or `codex`, logged in (`codex login`): each when a stage or a patrol is on it, only then.
///
/// `plan`, `tick` and `run` check these first. With one missing, logged out, not running or not given the checkout they do nothing and the error gives the line to run for each: nothing is installed, started or added for you.
///
/// ## What to run first, in a checkout of the repo
///
/// 1. `afkconvoy config`: read the config, what the fleet goes by. Its `--help` shows one whole. Then `afkconvoy check`, only where the config names a team: does the team load? A team is designed first: see designing a team, below.
/// 2. `afkconvoy plan`: what a tick would act on. It changes nothing.
/// 3. `afkconvoy run`: the checkout's fleet instance, held by the host runner until it is stopped. It answers once the first tick ended.
///
/// And whenever something looks wrong: `afkconvoy doctor` — it changes nothing, and names each fix.
///
/// ## What a command answers
///
/// Every command prints exactly one JSON object on stdout. A failure is {"error": …} with a non-zero exit — and "see" beside it, where a topic explains it: the command that prints that topic. Warnings go to stderr. A few print otherwise: `show` a drawing unless asked about an issue, `help` Markdown as `--help` does, `skill` the text of a file, `guard` answers as a hook does, with its exit code, and `exclusive` as its command does. A fleet instance prints nothing: it logs a line as each tick starts and one as it ends, in its tick log.
///
/// ## The commands, by who runs them
///
/// Read `afkconvoy <command> --help` before the first time you run one. The commands below are grouped by who runs them:
///
/// - **{const:OPERATOR}** — starting, watching or stopping a fleet. Read these first. `doctor` is what to run when something looks wrong, or you are asked whether the fleet is working: it says what the fleet on this host is doing and what is wrong, each finding with the line that fixes it.
/// - **{const:DESIGNER}** — `scaffold` to start one or a role of it from a skeleton, `draft` to draft its roster from the host kit, then, once `check` accepts it, the others: to review a workflow, to try one out, or to ask where an issue is in it.
/// - **{const:HANDS_WORK}** — a human, or the agent a human is talking to, with work decided on and issues to open for it. Read it before opening the first: it says which workflow takes which issue and the line that opens one, and creates the labels.
/// - **{const:SETS_UP}** — it prints the skill file that sends their agent to `afkconvoy --help`.
/// - **{const:WORKER}** — its prompt names each one as it is needed. Read them only when you are a worker, or are finding out why one stopped (`rule` is a consult's too).
/// - **{const:HUMAN}** — in a bound repo. Read them to answer an escalated issue, to put an issue at a stage of your choosing, or to see what a `by: human` stage asks.
/// - **{const:HOOK}** — its harness asks it before each thing a bound repo's worker does. Nobody runs it by hand. Read it to learn what a worker is refused, and why one was.
/// - **{const:BY_HAND}** — each is something a tick does on its own. Read one when a plan shows a transition the fleet is not making, or you are told to make it yourself.
/// - **{const:PLUMBING}** — what the transitions are built on. Read these only to inspect or repair claims and heartbeats on the remote.
///
/// ## The topics
///
/// Read `afkconvoy help <topic>` when this guide or a command's help names the topic: it prints that topic, Markdown, as `--help` itself is, and no JSON object. The topics below are what belongs to no one command, each opening with the sentence that says what it is for.
///
/// ## Rules to know before you start
///
/// - One fleet instance per host per repo. `run` in a second checkout of a repo on the host's checkout list is refused, naming the first: run it there.
/// - `run` returns once its checkout's first tick ended: the host runner ticks on, in the background, until `afkconvoy stop --all` stops it, keeping the checkout list. `afkconvoy stop` takes one checkout off the list. `tick` is one pass that exits.
/// - Each harness's worker launch command comes from this host's host kit, kept outside the repo. `run` writes it on a host that has none, and `afkconvoy kit` writes it again; neither asks anything. `--worker-command` is gone. A missing harness refuses the start.
/// - A host whose fleets are all idle is put to sleep, but only within its sleep hours: {const:SLEEP_HOURS} of its local time, unless it says others. `afkconvoy run --host-sleep-hours 22:00-06:00` says others for every fleet on the host, `--host-sleep-hours default` changes them back, and `--no-host-sleep` keeps the host awake at any hour.
/// - In a bound repo the worker launch command must end in its harness — Claude Code, pi or Codex — taking flags: the fleet adds to it what starts each worker in its role and under the guard ([`afkconvoy help worker-launch`](help:worker-launch) says what, for each harness), and a start refuses a command whose `--help` does not name those flags. Every harness the team could ask for is checked first, and none is ever started in another's place.
///
/// ## Designing a team, for a bound repo
///
/// A repo whose config names a team — `team: owner/repo`, or `team: owner/repo/path` for a team in a subdirectory — is a bound repo. An issue of it that carries a flow label is walked through that workflow's stages, each performed by a new worker started in the stage's role. A team and a role are each a directory of text files — a whole repo, or a subdirectory of one, so that one repo can hold a team and every role it names — and designing one is editing them. Nothing else holds any of it.
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
    help_template = "{before-help}{about-with-newline}\n{usage-heading} {usage}\n{all-args}{after-help}")]
struct Cli {
    /// The time to act at, in epoch seconds [default: the clock's]
    #[arg(long, global = true, hide = true)]
    now: Option<u64>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write this host's host kit: every harness and model it can start now
    ///
    /// ## When to run it
    ///
    /// When a harness is installed or removed, a login changes, or its models are to be probed again. `run` writes the host kit itself on a host that has none, and a tick writes it again once a day (below). It needs no repo and changes no remote.
    ///
    /// ## Who runs it
    ///
    /// The operator, or a machine in their place, on the host that starts the workers: it asks nothing, at a terminal or not, and does the same whoever runs it.
    ///
    /// ## What it reads and writes
    ///
    /// The host kit is $XDG_CONFIG_HOME/afkconvoy/kit.json, or $HOME/.config/afkconvoy/kit.json when XDG_CONFIG_HOME is unset. It is kept on this host and never pushed. It is the only source of worker launch commands. It holds the harnesses this host can start, in order of preference, each with its worker launch command and the models it was seen to answer with, in order — each by its id, the model its probe answered as, with its other names — and the harness and model of the stock consult. It is written automatically, with no confirmation:
    ///
    /// 1. Harnesses. Each known harness whose program `type` finds in an orca terminal — the login shell orca starts, which is the shell a worker gets, with its rc files, PATH and logins — on the command the host kit holds for it, or else its stock worker launch command.
    /// 2. Models. Each harness found is asked for the models its own login runs, in an orca terminal. Claude Code is asked through its command with an `initialize` request and no prompt; Codex through its command with `debug models`, its server's list; pi through its command with `--list-models`; DSH through `dsh headless`, whose listing plugin asks `https://api.deepseek.com/models` with DSH's account token from `$DSH_HOME/.credentials.yaml` (default `~/.dsh`), or its `DEEPSEEK_API_KEY` when no account token is held. The server's ids and display names are used, never a bundled catalog. This is the sole credential the fleet reads, through DSH's credential service, only for listing: never printed or stored, and sent only to DeepSeek. Not signed in or a server refusal leaves DSH out with the reason.
    /// 3. Probes. Every model listed is sent a one-line prompt in an orca terminal of its own, at most 60 seconds each, eight at a time; the terminal is closed once it answers or its time is up. DSH's prompt goes through `dsh headless`, on the model named. Only those that answered are kept, with when and how long each took. A harness none answered for is left out.
    /// 4. Names. Each probe says which model answered: Claude Code in the `modelUsage` of its JSON answer, pi in its JSON events, Codex in the session file of the thread its JSON events name (which is then deleted), DSH by its probe plugin. The names of one harness that answered as one model are one model, held once by that id, the model a worker is started on, with the other names and the names its harness shows them by (Claude Code's `Opus 5.5`) as its `names`. Claude Code's `opus` and `default` are `claude-opus-5-5`. A name whose probe says nothing of what answered is its own id.
    /// 5. Order. The harnesses the host kit held keep their order, and new ones follow in this fixed order: claude-code, pi, codex, dsh. Each harness's models are in the order it lists them (Claude Code's `default` first). Research may reorder these; with no new model the researched order is kept. The first harness and its first model are the host kit's first.
    /// 6. Stock consult. The harness and model the host kit names for it, while it still holds them — the model by any of its names; otherwise none is named, and it runs the host kit's first. An unmatched stock consult follows the host kit's first.
    /// 7. Research. Only a new answered identity is researched, once across its names and harnesses. One headless session on the host kit's first, in an orca terminal using its login, has 15 minutes to search vendor model cards and release notes, SWE-bench, Terminal-Bench and Aider. It answers one JSON object: harness and model preference order, good_at (strengths and weaknesses for coding work), and source URLs with publication dates. Every named model must be requested, exactly once; invalid JSON, an unknown or duplicate model, a failed session or a timeout leaves the fixed order and held good_at. An omitted model or one without a dated source URL is `not researched` and keeps its fixed place. An older kit's unsourced good_at becomes `not researched` without researching an identity already held. Sourced research is kept as good_at, sources ({url, date}) and researched_at (epoch seconds); it is never refreshed on a schedule. The kit is written either way, with the reason on stderr. A caster is told good_at (`afkconvoy cast --help`).
    ///
    /// A model named anywhere — a roster entry, a `model:` label, the stock consult — matches a model the host kit holds for its harness when it is its id or one of its names, whatever the case; with the provider prefix left off where that leaves one match (`gpt-6-sol` for pi's `openai-codex/gpt-6-sol`). A name with two matches matches neither, and says which two; one with none says the nearest names.
    ///
    /// It says on stderr where the file is, what it holds and what was left out.
    ///
    /// ## What it answers
    ///
    /// {path, written, kit, left_out}: the file; true; the kit written; and each harness or model not kept, as {harness, model, why} — model absent for a whole harness — why `not installed`, `listing failed`, `no longer listed` (a model the host kit held that its harness lists no more), `silent to its probe`, or `no model answered its probe`, with what was asked. Run twice with nothing changed, it writes the same kit but for the probe times. Errors: orca not running, with the line that starts it; a kit that cannot be read or written.
    ///
    /// ## What trips you up
    ///
    /// A harness or model left out is gone from the file: commands, the stock consult and research are kept only for what is kept. A missing harness is refused before workers start; it has no implicit stock command. `--worker-command` is gone: a harness's command is the host kit's. A command is an opaque shell string; an unbound repo starts it as written, under nice with its worktree's TMPDIR, adding only its harness's model flag for the model an issue's casting, the host kit's first or the stock consult names. A bound repo adds its role flags ([`afkconvoy help worker-launch`](help:worker-launch)).
    ///
    /// Probing sends each model a real prompt: it spends a little of each login. A harness the fleet finds refusing every prompt is paused on this host: left out of the host kit, and so of every range, until a probe finds it working. Each is probed every hour; one that answers here is back too.
    ///
    /// ## It stays current
    ///
    /// A tick — of `run`, or `afkconvoy tick` — writes the host kit again once its file was last written a day ago or more, before anything else: a model a harness now lists is added once it answers its probe, and one it no longer lists is dropped. The rest is kept as it is, unprobed: a harness that cannot be found or listed, or that stops answering, is kept, and a pause leaves it out, as above. The file's modification time is when it was last written — by `kit`, by `run` on a host without one, or by a tick — and `plan` and `doctor` answer it as `host_kit.refreshed`. Every fleet of the host shares the file and the lock beside it (kit.refresh): one of them writes it again, once a day, and the others read what it wrote. A refresh that changed the host kit is a line of the run log, `refresh_kit (fleet): the host kit was written again: added claude-code sonnet; dropped claude-code opus`; one that failed is one too, and is tried again an hour later. The pause is the host's running state, in its cache: this file is not changed. `plan`, `doctor` and the panel say which is paused, since when and why.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Kit,
    /// Cast every uncast issue that has a choice to make, in one session of the host kit's first
    ///
    /// ## When to run it
    ///
    /// Run `afkconvoy cast` in a repo to cast its issues now, at the terminal, rather than wait for a tick to start a caster. It casts the same issues a caster does, the same way.
    ///
    /// ## Who runs it
    ///
    /// The operator, in the repo's checkout. A tick runs it too: the caster it starts is this command, in an orca terminal of the repo's `convoy-caster` worktree.
    ///
    /// ## What it reads and writes
    ///
    /// An issue is cast by its `harness:` and `model:` labels. It is a caster's to cast while it is open and carries the ready label, no epic label, and no `harness:` or `model:` label — and while the host kit leaves a choice to make: more than one harness and model, a harness held with no model being one.
    ///
    /// In a bound repo an issue is the caster's while it carries a flow label, no epic label and no casting, and one of its workflow's stages' ranges leaves a choice (`afkconvoy help roster`). Each is sent with its stages, each with its role's range, and the prompt says how the roster says to cast in each role; what is offered is what those ranges hold, with what the roster says each model is good at. A casting none of its stages' ranges holds is not put on.
    ///
    /// 1. It reads every such issue, its title and its body.
    /// 2. It sends them, in one prompt, to the host kit's first — its first harness on its first model, in the login shell a worker gets — with the host kit's harnesses and models in order, each with the host kit's sourced research (`good_at`), and waits up to nine minutes for its answer: one line a casting.
    /// 3. On each issue it was answered for, where the host kit holds the harness and the model named — by any of its names — it puts the `harness:` label and, with a model, the `model:` label naming the model's id, and comments saying why. An issue cast meanwhile is left as it is.
    ///
    /// A casting a human put on is never changed: an issue with a `harness:` or a `model:` label is never given to the caster. A caster's own casting is cast again, before a worker started on it, when its harness is paused, or when its model matches none the host kit holds any more.
    ///
    /// ## What it answers
    ///
    /// {cast, uncast}: cast lists {issue, harness, model, why} of each issue it labelled, uncast {issue, why} of each it did not. With no choice to make or no issue to cast, both are empty and `said` says why. Errors: a host kit, or a bound repo's team, that cannot be read; a model that did not answer.
    ///
    /// ## What trips you up
    ///
    /// A tick holds an issue that waits for a caster back while one runs, for ten minutes at most from its start, and starts no second one meanwhile; `plan` shows it as `caster` and the issue as "held for the caster". An issue the caster had and did not cast is dispatched uncast, on its range's first, and `plan` and the panel say so. A switch chooses whatever the casting: while one stands, nothing is held for a caster.
    ///
    /// Casting sends the host kit's first a real prompt: it spends some of its login.
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
    /// It reads one file — {const:CONFIG_FILE} of the checkout it is about, or `--file` — and of that file only the first ```yaml block. It writes nothing, and reads neither GitHub nor the remote.
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
    /// concurrency: 3
    /// escalate_label: ready-for-human
    /// lock_label: fleet-lock
    /// team: acme/team                    # naming a team binds the repo
    /// patrols: []
    /// intake_limit: {const:INTAKE_LIMIT}
    /// ```
    /// ~~~
    ///
    /// A key left out is at the value shown, so the shortest config is the first two lines, and a bound repo's those and `team`. The last two are read only beside `team`: an unbound repo leaves all three out, and may say one key more, never beside `team`:
    ///
    /// ```
    /// escalate_to: consult               # or human
    /// ```
    ///
    /// What is outside the block is yours to explain it in.
    ///
    /// No command writes this file: write it, commit it, and run `afkconvoy config`.
    ///
    /// ## What it answers
    ///
    /// {"base_branch", "gate_command", "gate_seconds", "ready_label", "epic_labels", "concurrency", "escalate_label", "lock_label"} — every key, the defaults filled in. `gate_seconds` is the gate limit: how many seconds a run of `gate_command` is given, by `afkconvoy gate` and by `afkconvoy land --train` alike, before it is ended and is red (default {const:GATE_SECONDS}). It is written nowhere else: neither command has a flag for it. A config that leaves it out is a finding of `afkconvoy doctor`. `lock_label` is the label of a fleet lock (default fleet-lock): while an open issue carries it, the fleet works lock issues only (`afkconvoy lock --help`). `concurrency` is how many claims a fleet holds at work at once (default 3) where its checkout says none of its own: a checkout's own on its host, said by `afkconvoy run --concurrency`, wins there. This answers the repo's alone; `afkconvoy plan` says the one in force, and who says it.
    ///
    /// A repo whose config names a team is a bound repo, and answers three keys more: {"team", "patrols", "intake_limit"} — the team's directory as owner/repo or owner/repo/path (a whole repo, or a subdirectory of one), which of its patrols run here, by name (default none; `afkconvoy check` holds them against the team's, and `afkconvoy tick --help` says when one runs), how many issues taken in by workers may be open (default {const:INTAKE_LIMIT}). A config names no harness: what each worker runs is its team's `roster`'s to say (`afkconvoy help roster`), within what the host kit holds (`afkconvoy kit --help`). A repo that names no team is unbound and answers the eight above and one more: {"escalate_to"} — where a hand-off to a human goes first. `consult` (the default): the stock consult, afkconvoy's own, is asked first and stands where the human would; a human is reached only where it lets the issue through, or fails. `human`: every hand-off goes straight to a human, as `afkconvoy help humans` says. A bound repo's stages say this each (`afkconvoy help workflows`).
    ///
    /// Errors: the file cannot be read; it has no ```yaml block; `base_branch` or `gate_command` is missing; a key is unknown or of the wrong type; `gate_seconds` is not a whole number from 1; `team` is neither owner/repo nor owner/repo/path; a `harness`, which a config names no more — the error says the line to delete; `patrols` or `intake_limit` is set where no `team` is; `escalate_to` is neither consult nor human, or is set beside `team` — the error names the key. Without `--file`, outside a git checkout: "is not in a git checkout".
    ///
    /// ## What trips you up
    ///
    /// - It reads the config only. Whether the team it names loads is `afkconvoy check`.
    /// - It reads the file as it is in this checkout, committed or not. A worker reads the one in its own worktree, which is cut from the base branch: a change is in force only once it is on the base branch.
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
    /// {"team", "path", "commit", "files", "escalate_label", "rules", "host_user", "dsh_profile", "split", "roles", "workflows", "patrols", "precedents"} — what it read.
    ///
    /// `team` is the team's repo and `path` its directory there, "" for a whole repo — with `--dir`, the directory read and null. `commit` is the team's commit read, null with `--dir`; `files` the definition files read, each from the team's directory. `rules` is whether the team has rules of its own: a `CLAUDE.md` in its directory, which every one of its workers is given.
    ///
    /// `host_user` is whether its workers are given the host user's own skills and rules beside their role's: true unless the team file says `host_user: false`, and a value that is not true or false is a fault (`afkconvoy help roles`). It is each role's default, and the stock consult's.
    ///
    /// `split` is whether stage briefs offer splitting: true unless the team file says `split: false`. Anything but true or false is a fault (`afkconvoy help team`). `dsh_profile` is `fleet` (default) or `host-user`: which profile DSH workers load, independently of host_user. A role may say its own choice; check answers its effective dsh_profile and refuses every other value.
    ///
    /// Each of `roles` is {"name", "repo", "path", "commit", "host_user", "dsh_profile", "skills", "above", "limits", "roster", "range", "left_out", "unmatched", "nothing_safe"}: `path` is the role's directory in `repo`, "" for a whole repo; `commit` the one the role's repo was read at; `host_user` whether its workers are given the host user's skills and rules — its own say, `{ dir, host_user, dsh_profile }`, or the team's; a key of that map but those three, or a `host_user` that is not true or false, is a fault; `above` each `CLAUDE.md` and `.claude/` a directory above the role holds in its repo, as a path from the repo's root — but for the team's own `CLAUDE.md`, where the team's directory is above the role: those are the team rules, which a worker is given; `limits` the harnesses the role holds limits for — a hook or a denied permission in its `.claude/settings.json` for claude-code, an entry of its `.pi/extensions/` for pi, a hook in its `.codex/hooks.json` for codex, a `.dsh/cordis.patch.yml` overlay for dsh; `roster` its entry in the team's roster, {"cast", "run": [{"harness", "model", "good_at"}]} with null for what it leaves out, and null for a role it names none for; `range` its range on this host, [{"harness", "models"}] in the order a worker takes them — the host kit whole for a role with no entry, or whose entry matches nothing in the host kit, less each harness the role holds no limits for while it holds limits for another, and null where this host has no host kit (`afkconvoy help roster`); `left_out` each harness its entry and the host kit allow it and its range leaves out, [{"harness", "why"}] — why `no limits for pi` — and null where this host has no host kit; `unmatched` what its roster entry allows and the host kit holds, where the entry matches nothing in the host kit — the role then ranges over the host kit whole — and null otherwise; `nothing_safe` why no worker of it can be started on this host, where it holds limits for no harness the host kit holds — its stages wait — and null otherwise. Each of `workflows` is {"name", "flow_label", "max_rework", "max_consecutive_rework", "stages", "land"}, the defaults filled in; each stage is {"name", "by", "do", "writes", "escalate", "pass", "reject"}. `unmatched_models` lists each roster model matching nothing held, as {role, harness, model, holds, said}, with the nearest names or the two matches; a tick opens one issue for each in the team’s repo.
    ///
    /// Each of `patrols` is {"name", "by", "do", "every", "max_open", "max_runs_a_day", "max_landed_a_day", "runs_here"}: `every` in seconds, `max_runs_a_day` null where the file says none. `runs_here` is whether the config of this checkout names the patrol — false with `--team` and `--dir`, which read no config. `precedents` is {"by", "do", "max_open"}, or null where the team names nobody to propose them.
    ///
    /// A team with a fault: {"error", "faults"} and exit 1. `faults` holds every fault of the team, not the first: each {"file", "key", "problem", "see"}. `key` is a path from the top of the file's yaml, counting from 0 — `stages[1].on.reject` — and empty when the fault is the file itself. `see` is the command that prints the help topic describing that file, to run as it is: `afkconvoy help workflows` for a workflow's, `afkconvoy help patrols` for a patrol's, and `afkconvoy help team` for the team file — a role is named there, and that topic says what its directory holds — and for the config's keys that bind the repo. A fault in a file no topic describes has no `see`.
    ///
    /// A fault is one of: an unknown key, or one written twice; a wrong type; a duplicate stage name; a stage named `land`, `done` or `human`; a `pass` or `reject` that names no stage; a `reject` to its own or a later stage; a stage no path from the first reaches; a stage whose passes never come to `land` or `done`; a `pass: land` with no `land:`, or a `land:` no pass leads to; a role the team file does not name; a role named as none of owner/repo, owner/repo/path and ./path; a role's path with a `..`, or an absolute one; a role whose repo cannot be read, or whose path is no directory at its head; a role's directory that is the team's or contains it, or is inside the team's `workflows/` or `patrols/`; a role's directory that contains another role's or is inside one — two names for one directory are none; a symlink in a role in a subdirectory that leaves the role's directory; a file in a role's directory that a harness would load in place of `CLAUDE.md`, its rules under every harness — pi `AGENTS.override.md`, `AGENTS.md` or `AGENTS.MD`, Codex `AGENTS.override.md` or `AGENTS.md` — the fault naming the file and each harness that would load it, whatever harnesses the role may run; a role's limits that cannot be read; a skill its role's directory does not hold; a missing `by`, `do` or `on`; a `harness` or a `model` in a stage or a patrol, which name none any more — the fault says the line to delete and the roster entry to write in its place; in the `roster`, an entry for a name that is no role of the team, a `run` that is missing or empty, a `harness` that names no harness, or an allowance written twice; a flow label two workflows take, or one that is the team's escalate label, the repo's lock label or an epic label — under `label`, or the file where it is the `flow:<name>` it names none in place of; a patrol file whose name is not a patrol's; an `every` that is no duration; a missing `every` or `max_open`; and — in the config, {const:CONFIG_FILE}, under `patrols` — a name the team has no patrol for.
    ///
    /// A team that loads is then held to its harnesses, and these are faults too, each with its file and key, all at once:
    ///
    /// - A skill a worker is asked to perform whose file is not in the form every harness reads a skill in: its directory holds no `SKILL.md`, or one that opens with no frontmatter, or whose frontmatter says another `name` than the directory's, or no `description`. The key is the `do` that names the skill — `stages[1].do`, `stages[1].escalate.do`, `land.do`, a patrol's `do`, and `precedents.do` in team.md — and each `do` that names it has the fault. The form is in `afkconvoy help skill-file`. A skill no `do` names is not held to it.
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
    /// - `workflows/feature.md`: one workflow, `feature`: one stage the first role performs as its skill `implement`, then the landing train
    /// - `roles/<name>/`: that role's skeleton, as `scaffold role` writes it — the first role's with the skill `implement`, every other's with no skill
    ///
    /// `--role` is given once for each role, and at least once. `--role <name>` is a role written here. `--role <name>=<owner/repo>` or `<name>=<owner/repo/path>` is one that is elsewhere — a role several teams share, in a repo of its own: `team.md` names it as given and nothing is written for it. The two forms mix. The example workflow is performed by the first role given; one that is elsewhere must hold a skill `implement`.
    ///
    /// A role after the first is written with its rules and its limits, and no skill: a reviewer does not implement. What it is to perform is yours to write, as `roles/<name>/.claude/skills/<skill>/SKILL.md`, with the stage that names it in `workflows/feature.md` — that file says so too.
    ///
    /// `scaffold team <dir> --example <name>` writes an example team whole in place of a skeleton: its team file, team rules, workflows, patrols and precedents, and every role it names under `roles/<name>/`, with each role's rules, skills and hooks. The one name it takes is `hospital`: seven roles, three workflows and four patrols, which `afkconvoy help example` goes through file by file. It takes no `--role` — the team file names its own, each as `./roles/<name>` — and is a team's alone. A hook, like any file written that starts with `#!`, is written executable. Skills by other authors that the example's roles load are not written, and one of its stages runs on pi: `afkconvoy help example-run` says what a host needs before a fleet works with it.
    ///
    /// `scaffold role <dir>` writes a role's three kinds of file, and takes no `--role`:
    ///
    /// - `CLAUDE.md`: rules to replace with the role's own
    /// - `.claude/skills/implement/SKILL.md`: one example skill, `implement`: rename it, and rewrite it, for a role that does not; its form is in `afkconvoy help skill-file`
    /// - `.claude/settings.json`: its limits for claude-code — hooks: none yet
    /// - `.codex/hooks.json`: its limits for codex — hooks: none yet
    ///
    /// The rules and the skills are written once, whatever harness a stage runs the role on. The role's limits are written once for each harness, in that harness's own form: for claude-code, hooks and denied permissions in `.claude/settings.json`; for codex, hooks in `.codex/hooks.json`; and for pi, extensions in `.pi/extensions/`, a directory this command does not write. For dsh, a role writes its own overlay in `.dsh/cordis.patch.yml`; this command writes none. A file that names no hook holds no limit: the skeleton is right for every harness. `check` refuses a stage on a harness its role holds no limits for while it holds some for another.
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
    /// Whoever hands the fleet work: a human, or the agent a human is talking to, with work decided on and issues to open for it. In a checkout of the repo the issues go to, bound or unbound. A worker does not: it takes an issue in with `afkconvoy intake`.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the config of the checkout and the labels the repo has. In a bound repo it also reads the team as `afkconvoy check` does — the team's files and every role's repo, from GitHub at the head of each default branch — and, to count, the comments of every open issue that carries a flow label.
    ///
    /// It writes one thing: each label its lines name that the repo lacks. In a bound repo that is the flow label of every workflow of the team — `flow:<workflow>`, or the one its file's `label` names; in an unbound one, the ready label. It puts no label on any issue and opens none. Everything is read first: a team with a fault creates nothing.
    ///
    /// ## What it answers
    ///
    /// In a bound repo:
    ///
    /// {"team", "workflows", "keep_off", "intake", "created"}
    ///
    /// - `team`: the team's repo
    /// - `workflows`: every workflow of the team, each {"name", "flow_label", "description", "first", "ends", "line"}:
    ///
    ///   - `description`: what the workflow is for and what an issue of it holds: the prose of its file, workflows/<name>.md, above the yaml block. Empty when the file says nothing there — `afkconvoy check` warns of that. Choose the workflow by it
    ///   - `first`: the stage an issue enters at, {"stage", "by", "do"}: the role that performs it and the skill — `by` is `human`, and `do` null, where a human's decision is waited for
    ///   - `ends`: where its passes end: `land`, the landing train — its issues end in a PR landed on the base branch — `done`, nothing more to do, or both, by different stages
    ///   - `line`: the line that opens an issue into it. Give it a title and a file holding the body
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
    /// Errors, as {"error": …} with a non-zero exit: the team has a fault — {"error", "faults"}, as `check` answers it; the team's repo cannot be read; the config cannot be read; "is not in a git checkout"; GitHub refused to list or create a label.
    ///
    /// ## What trips you up
    ///
    /// - `gh issue create --label` refuses a label the repo lacks, and nothing but this command creates a flow label ahead of the first issue. Run it before the line.
    /// - An issue follows one workflow: put one flow label on it. With several it follows none, and no stage of any is performed on it.
    /// - The flow label is what hands the issue over. Leave it off an issue that is not ready to be worked, and put it on later: `gh issue edit <n> --add-label <label>`.
    /// - A workflow whose `first` is `by: human` starts by waiting for a human: the fleet does nothing on its issue until one declares a verdict (`afkconvoy verdict --help`).
    /// - It answers the team as pushed to its default branch. A workflow added in a checkout of the team is not answered, and its label not created, until it is pushed.
    /// - A label of a workflow the team no longer has is left on the repo: this command creates labels and removes none.
    #[command(next_help_heading = HANDS_WORK, verbatim_doc_comment)]
    Flows {
        #[command(flatten)]
        at: At,
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
    /// Draw a team's workflows as graphs, or say where one issue is in its workflow
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
    /// ## Who runs it
    ///
    /// The operator, or whoever designs the team: in a checkout of a bound repo, or anywhere with `--team` or `--dir`.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing. It reads a team as `check` does, with the same `--team` and `--dir`: the team's files, and every role's repo from GitHub. See `afkconvoy check --help`. With a workflow's name it draws that one; with none, every workflow of the team.
    ///
    /// With `--issue` it also reads, in a checkout of the issue's repo, the issue's labels and its comments. The workflow is the one the issue's flow label names — for an escalated issue, whose flow label was swapped out, the one its escalation's comment says it left. The position is read off the issue's stage records — the comments `afkconvoy verdict`, `rule` and `send` write — and off nothing else: not its `stage:` label, which the fleet writes for humans to filter by and never reads. How it came in is read off its intake record, the comment `afkconvoy intake` writes.
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
    /// The first line is the workflow, its flow label and its two breakers. Then one row per stage, in the order its file lists them — an issue enters at the first — with who performs it, as `<role>: <skill>` or `human`, and what else its file sets. Under each stage is where its `pass` leads. A `reject` is said on its stage's row and drawn as the line that leaves that row to the left and comes back in, at the arrow, to the stage it leads to. The last rows are the ends: `land` with who lands, and `done`. Under the drawings, once, is the team's roster: a row per role, each harness and model it may run in order of preference — `any model` where the entry names none — with what it is good at, and how the team says to cast among them under it; `the host kit whole` for a role the roster names no entry for (`afkconvoy help roster`). What a role's range is on this host is `afkconvoy check`'s to answer.
    ///
    /// With `--issue`, JSON:
    ///
    /// {"issue", "workflow", "intake", "position", "follows", "rework", "consecutive_rework", "records"}
    ///
    /// - `intake`: how the issue came in. {"by": "worker", "role", "from", "url"}: a worker in that role, on the issue `from`, took it in with `afkconvoy intake`, and `url` is the comment that records it; `patrol` in place of `from` where a patrol's worker did. {"by": "human"}: its flow label has no intake record for it
    /// - `position`: where the issue is: {"is", "stage", "by", "do", "harness", "reason"}, as `afkconvoy rehearse --help` spells out
    /// - `follows`: the position a `pass` declared now would lead to; null where none can be declared: the issue is landing, ended, escalating or with a consult
    /// - `rework`: {"had", "left"}: its rework so far, and what `max_rework` still allows
    /// - `consecutive_rework`: for each stage that may reject, {"had", "left"}: its rejections in a row, and what `max_consecutive_rework` still allows
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
    /// Errors, as {"error": …} with a non-zero exit: the team has a fault — {"error", "faults"}, as `check` answers it; the team has no workflow of that name, or none at all; the issue has no position — it carries several flow labels, one that names no workflow of the team, or none with no escalation that says which it left; and every error of `check`.
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
        /// follows, and the rework it has left
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
    /// 1. It sends the host kit's first — its first harness on its first model, in the login shell a worker gets — one prompt: every harness and model of the host kit in order, and each role with the work the team asks of it and what it may run. It gives the model the host kit's good_at as given and waits up to nine minutes for one line for each role, saying how to cast an issue there. It asks no model to assess model abilities.
    /// 2. It drafts the team file's `roster` (`afkconvoy help roster`): an entry for each role, its `run` every harness and model of the host kit in the host kit's order, each with the `good_at` from the host kit, and the role's `cast`. A harness the role holds no limits for, while it holds limits for another, is left out of its entry and named in a comment there, so that the draft passes `check` as written; a role left nothing is named in a comment alone.
    /// 3. It writes the draft into the team file, team.md, in place of the `roster` there and everything under it, or at the end of its yaml block where it has none. Nothing else of the file changes, and nothing is committed.
    ///
    /// Where the team file holds a roster, it says on stderr each line the draft would take out (-) and put in (+), and writes only once the human at the terminal confirms, or with `--yes`. With no terminal and no `--yes` it writes nothing.
    ///
    /// ## What it answers
    ///
    /// {"path", "drafted_by", "roster", "replaces", "removed", "added", "unsaid", "written"}: `path` the team file, `drafted_by` the {harness, model} asked, `roster` the draft as written, `replaces` whether the team file held a roster, `removed` and `added` the lines the draft takes out and puts in, `unsaid` what the model said nothing of — each also a warning on stderr, to write in by hand — and `written` whether the team file was written. A roster the same as the draft is not written again.
    ///
    /// Errors: a team that does not load, with every fault, as `check` answers it; a host kit that cannot be read or holds no harness; a model that did not answer.
    ///
    /// ## What trips you up
    ///
    /// - Read the draft before committing it: model strengths come from the host kit's sourced research, and how to cast comes from the drafting model. Edit them, take out what a role is not to run, and order what is left as the team prefers — `afkconvoy check --dir` then says whether the team loads.
    /// - A team file whose roster has a fault does not load: delete the roster, and draft again.
    /// - The draft is the host kit of the host it runs on. A harness another host holds and this one does not is not in it.
    /// - Drafting sends the host kit's first a real prompt: it spends some of its login.
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
    /// - is: landing: the workflow's `land` joins the landing train, as `do` of `by`
    /// - is: ended: nothing more to do
    /// - is: escalating: handed to a human, with the `reason`; under their `ruling` the `stage` runs again
    ///
    /// `harness` is the harness the worker of the position is a session of, null where no worker acts: the stage's — the one it names, or the default of this checkout's config — for a worker that performs it, and the default for a consult and a landing.
    ///
    /// `valid` is false for what may not be declared where the issue is: a `reject` from a stage with no `on.reject`, a `ruling` nobody asked for, anything but `ruling` or `escalate`, `close` or `park` from a consult. An invalid verdict escalates the issue. Anything declared on an issue that is landing or ended, and anything but a `ruling` on one that is escalating, is not valid either and changes nothing.
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
    /// It reads this host (the commands the fleet needs, in a login shell; orca), the log this host's `run` keeps of its ticks, the open issues, PRs and stage records on GitHub, the claims, heartbeats and recorded gate runs on the remote, and orca after the worker of each claim. It writes nothing on GitHub and nothing on the remote, and makes no startup probe: it is safe beside a live fleet, as often as you like. It fixes nothing either: each finding names the command that does.
    ///
    /// With `--all` it reads the checkout list (checkouts.json in the cache directory) and the same of each checkout on it, all at once.
    ///
    /// ## What it answers
    ///
    /// {"host", "fleet", "claims", "issues", "waiting_on_human", "findings"}
    ///
    /// - `host`: what this host has of what the fleet needs:
    ///
    ///   - `tools`: each command needed — a harness only where a worker launch command the fleet could be asked for runs it by name: {name, state} — state is ready, missing or logged_out; of orca also not_running, knows_no_checkout (the checkout worktrees are cut from was never added to it) or unasked
    ///   - `worker_commands`: one for each harness the fleet could be asked for: {harness, called_for_by, command, from, starts_a_worker, takes_role_flags} — called_for_by is the host kit's first, each role whose range holds the harness, and the stock consult; from is `host_kit`. Commands come from this host's kit, never a run log.
    ///   - `host_kit`: {path, refreshed, harnesses, stock_consult}: where the kit is kept; when it was last written, in epoch seconds — by `kit`, by `run`, or by a tick's daily refresh (`afkconvoy kit --help`); each harness in order, first first, with its full worker launch command and its models in order, each with when it last answered a probe (`probed.at`, epoch seconds; null: a model a kit written before DSH was probed holds as named); and the harness and model the stock consult runs (`named` false: the host kit's first; `unmatched`: what is named for it matches nothing held, and it runs the host kit's first). {path, error} when unreadable. `afkconvoy kit` writes it again.
    ///   - `settings`: the host settings (`afkconvoy run --help`): {file, said, in_force} — each by its key, the name of the flag of `run` that names it (`hot_cores` for `--hot-cores`), in that flag's units: said is what the host says, null where it says none and the default holds; in_force what holds for this repo's fleet on this host, each default worked out for it — the panel's port null where any free port will do. {file, error} where the file cannot be read
    ///   - `admission`: whether a tick would start a new worker on this host now, the thresholds in force and what the host reads: as `plan` answers it (`afkconvoy plan --help`), or {error} where a share the host says is wrong. A tick that admission held back is no "started no worker" finding. Its inspected is what the last host inspection on this host found, whichever fleet started it: {repo, reason, reading, by, started, ended, found, said, issue, reclaimed, named} — what admission held back for and read, the role, reclaimable, fleet, not-its or nothing, its words, the issue a fleet finding opened, and each [{path, bytes}] it freed and named for a human; null with none
    ///   - `exclusive`: the host's queue of `afkconvoy exclusive`, whatever its repo, as its host runner holds it: {running, waiting} — running is the command that runs alone now, null with none; waiting, those that wait their turn, next first; both empty with no host runner. Each is {command, dir, issue, pid, asked, ran}: the worktree it was run in, the issue of its worker, and when it asked and began to run, in epoch seconds. {error} when the host runner cannot say it
    ///   - `inspection`: the inspection running on this host, whatever its repo: {kind, repo, worker, issue, by, since, held_back_by} — worker or host, the repo whose fleet started it, the hot worker and its issue ("the host" and null of a host's), the role it runs in, when it started, in epoch seconds, and of a host's what admission held back for; null with none (`afkconvoy inspected --help`)
    /// - `fleet`: the `run` of this host for this repo:
    ///
    ///   - `alive`: whether one is running now: this host's runner, asked on its socket naming this checkout, holds a fleet instance of this repo
    ///   - `instance`, `host`: its fleet instance id: the one its log names, else the one the claims stamped with this host name; null when this host never ran one
    ///   - `heartbeat_age`: seconds since its heartbeat; null: it has none
    ///   - `heartbeats`: {found, live}: how many heartbeats the remote has, whoever stamped them, and the instance ids of those within the lease. A `run` told to stop removes its own, and a start removes the dead ones that hold nothing: more than one found and not live is fleets nobody started again
    ///   - `log`: the file its ticks are logged in, or "no log": no `run` of this version ran here
    ///   - `last_tick`: {count, ended, next_due, line, did, error}: when its last tick ended and the next is due, in epoch seconds, the line `run` printed, and what `tick` answers as `did`; error, when the tick failed whole. null with no log
    ///   - `failed_steps`: the last {const:DOCTOR_FAILED_SHOWN} failures, newest first: {tick, ended, do, issue, patrol, error, count}. One cause that stopped several steps of a tick is one failure: count is how many, and do, issue and patrol the first's
    ///   - `host_sleep`: {checked, slept, kept_awake_by}: its last check for host sleep — when it ended, whether it put the host to sleep, and else what kept the host awake, each in words: "the host's local time is outside its sleep hours, 00:00-06:00" among them. null: it made none, the host settings keeping the host awake, or never idle {const:HOST_IDLE_MINUTES} minutes
    ///   - `events`: the last {const:DOCTOR_EVENTS_SHOWN} events that reached a `run` of this host, newest first, whichever instance: {at, event, cause, instance} — event being wake (between ticks, or during one), start, stop (told to, by SIGTERM or SIGINT), broken, back (`afkconvoy run --help`), host_sleep or host_wake; cause says what caused it, in words. [] with no log
    ///   - `broken`: {why, since, next, instance}: its fleet instance is broken (`afkconvoy run --help`) — why, as it last broke; since when, and when the host runner starts it again, in epoch seconds, null where no host runner runs on this host; and the fleet instance that broke, "" where none had started. null: it is not broken
    ///   - `concurrency`: {in_force, said_by}: how many claims its fleet instance holds at work at once on this host, as `plan` says it — said_by `checkout`, `config` or `default`. {error} where the config or the checkout list cannot be read
    /// - `claims`: each claim of that instance: {issue, title, status, pr, stage, role, held_for, worker, worktree, gate} — status as `plan` gives it; stage and role in a bound repo; held_for in seconds; worker is {state, quiet}, state being busy, stopped or gone, and null when orca could not say; gate is {green_at} when a green gate run is on record for the tree the worktree is at, else null
    /// - `issues`: as `plan` answers them: {frontier, excluded, escalating, locks, free_slots, patrols} — what a worker can be started on, what waits and why, and what is being handed to a human and why. null when they could not be read
    /// - `waiting_on_human`: as `plan` answers it, and as the panel lists it under "With a human": every open issue nobody holds that waits for a human — escalated, at a stage that is a human's, past the end of its workflow and still open, or reporting a patrol's failed run — each as {issue, title, why, since, answer}: `answer` is the line that answers it. null when the issues could not be read
    /// - `findings`: what is wrong, most severe first — none: []. Each is {severity, what, evidence, fix, see}:
    ///
    ///   - `severity`: how much it matters, one of those below
    ///   - `what`: the fault, in a sentence
    ///   - `evidence`: what was read that says so
    ///   - `fix`: one command line to run as it is — or, for a fault in a team's or a role's file, or a key the repo's config leaves out, `<file>: <key>` to edit
    ///   - `see`: the command that prints the help topic explaining it: `afkconvoy help humans` where what it reports waits for a human, else `afkconvoy help troubleshooting`, which has a row for each finding
    ///
    /// A finding's `severity` is one of, most first:
    ///
    /// {table:severity}
    ///
    /// It exits 0 with no critical finding, and non-zero with one — printing the same object, not {"error"} — so that a script can gate on it. Errors: "is not in a git checkout". Whatever else cannot be read is itself a finding.
    ///
    /// With `--all`:
    ///
    /// {"host": {"host_kit", "exclusive", "inspection"}, "findings", "checkouts": [{"checkout", "repo", "doctor"}]}
    ///
    /// - `host`: what of `host` is the host's alone, as above, said once
    /// - `findings`: the host's findings, said once however many checkouts found them, most severe first: a command missing or logged out, orca not running, a harness paused
    /// - `checkouts`: each checkout on the list, in its order — [] with none — with the repo its remote `origin` names, null where it is no git checkout any more, and its `doctor`: what `doctor` answers in it — `waiting_on_human` among it — but for the host's, which are above. Each is asked as the fleet of the host its `run` named, unless `--host` names one. {"error", "see"} where it could not be asked
    ///
    /// It exits non-zero where any finding is critical, the host's or a checkout's, or a checkout could not be asked — printing the same object. Errors: the checkout list cannot be read.
    ///
    /// The findings: a command the fleet needs is missing or logged out; orca cannot be asked, or does not know the checkout; the worker launch command of a harness the fleet could be asked for cannot start a worker — said with the harness and what calls for it; no `run` is alive and claims are held; the fleet instance of this checkout is broken — with why, since when and when the host runner starts it again; the same step failed in each of the last three ticks; the team has a fault at its head; a fleet lock stands on an issue that cannot be worked (all critical) — a claim has no worker; a worker has been stopped for half an hour with its claim still at work; the frontier is not empty, slots are free and the last tick started nobody; a flow label names no workflow of the team; an issue has been at a stage longer than any before it took; an issue is one rework from a breaker; a patrol is failed or full; an issue has waited for a human for more than an hour, its `fix` being the line that answers it; the gate is slow — with the host, the gate limit and the {const:SLOW_GATE_LANDINGS} runs, each a commit and its seconds, as `afkconvoy plan` has them, and the issue open for it when there is one (all warning) — the role or the team a stage in flight started under has moved since; and the repo's config declares no gate limit, so the gate has the default's {const:GATE_SECONDS} seconds before anything ends it: `fix` is the file and the key, `gate_seconds`; and a worktree on this host is kept with no claim on its issue — said with what it holds that the remote does not (uncommitted changes, commits only it has, or nothing), its `fix` the line that removes it, pushing first whatever only it holds (all info).
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
    /// ## Worker proxy bypass
    ///
    /// `--worker-no-proxy [HARNESS=]HOSTS` shows what a start with those flags would give each harness, under `worker_no_proxy`: [`afkconvoy help worker-proxy`](help:worker-proxy).
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Doctor {
        /// Every checkout on this host's checkout list, saying the host's findings once
        #[arg(long, conflicts_with = CHECKOUT_ARG)]
        all: bool,
        #[command(flatten)]
        proxy: ProxyArgs,
        /// The host whose fleet is asked after [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: At,
    },
    /// Show what a tick would act on, and change nothing
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
    /// It makes the startup probe — which checks, as a start does, the worker launch command a `run` started now would give every harness the fleet could be asked for (`afkconvoy run --help`) — then the rebuild: the open issues and PRs on GitHub, the claims and heartbeats on the remote, and the landing train. It changes nothing on GitHub. On the remote it pushes its startup probe and deletes it again, together with what the probe sweeps: a probe an earlier start left behind, and recorded gate runs too old for a landing to believe.
    ///
    /// WHEN IT PROBES. Not every time: a startup probe that passed on this host in the last hour stands, and `plan` then makes none — it says so on stderr, repeats that probe's warnings, and pushes nothing. It probes when no probe stands: none passed on this host within the hour — `run` makes one at every start, `tick` and `plan` when they find none — or the config's `base_branch` or `team` changed since, or the remote's URL did, or the note of where the remote keeps the fleet's refs. The probe is noted in the repo's git dir (`convoy-probe`); delete that file to have the next `plan` probe.
    ///
    /// The rebuild asks GitHub twice, side by side with one listing of the remote, however many issues are open: one query for the open issues with their comments (an issue with more than a hundred comments is asked about once more), one for the open PRs. It asks once more — for the issues that carry the label `slow-gate` — only while the recorded gate runs of the base branch's landings say the gate is slow.
    ///
    /// ## What it answers
    ///
    /// {"frontier", "excluded", "waiting_on_human", "mine", "peer_live", "stale", "stale_closed", "free_slots", "now", "locks", "concurrency", "admission", "host_kit", "worker_no_proxy"} — "held_back" while admission holds; and in a bound repo "intake", "left_out", "unmatched" and "nothing_safe" where they name some, "patrols" where it names some, and "proposals" while a ruling is proposed as a precedent or waits to be; and "slow_gate" while the gate is slow; and "caster" while one runs or is due
    ///
    /// - `host_kit`: {path, refreshed, harnesses, stock_consult}: this host's kit file; when it was last written, in epoch seconds — by `kit`, by `run`, or by a tick's daily refresh, which writes it again once a day (`afkconvoy kit --help`); each harness in order of preference, first first, as {harness, command, models}, each model {model, names, probed: {at, seconds}, good_at, sources, researched_at} — good_at, from sourced research or `not researched`, what a caster is told the model is good at; sources the dated URLs and researched_at the epoch seconds of the research, absent where none succeeded — in order — `at` when it last answered a probe, in epoch seconds, and `probed` null for one a kit written before DSH was probed holds as named; and {harness, model, named, unmatched} that the stock consult runs, named false when it is the host kit's first, and unmatched, where what the host kit names for it matches nothing held, saying so — it then runs the host kit's first. With an unreadable kit, {path, error}. `afkconvoy kit` writes it again.
    /// - `frontier`: the issues a tick would dispatch, lowest number first: {number, title} — in a bound repo with the stage each is at: {number, title, stage}, those a stage has already declared on ahead of those never started; an issue whose stage's `escalate` waits for its consult is here too, at that stage; the stage is `land` once the last one passed to the landing train and the issue's PR waits for a worker to join it. Each but the stock consult's has cast: {harness, model, why}: what its worker would run, model null for its command as written, and why — `casting` (its `harness:` and `model:` labels, which the range holds), `switch` (this host's switch, over any casting), `unmatched` (a name it was given matches nothing held — a label, or a switch to a harness the host kit does not hold — and cast has `unmatched` too, naming it: the range's first, and the issue is told so once, in a comment, as its worker starts), `uncast` (a caster had it and did not cast it in ten minutes: the range's first); in an unbound repo `first` (no casting: the host kit's first); in a bound repo, of its stage's role's range (`afkconvoy help roster`), `range_first` (no casting: its first), `outside` (it does not hold the casting, and another stage's range does: its first) or `only` (it holds one harness and model, whatever the casting)
    /// - `excluded`: every other open issue, with why: {number, title, reason}. An escalated issue says so — "escalated: waits for a ruling" — and not that it lacks the label its escalation took off. A name that matches nothing never excludes an issue: it runs the default, `unmatched` on the frontier. A bound repo's issue at a stage whose role has no harness on this host it holds limits for waits, "<role> has no harness on this host it holds limits for: ", naming what the role holds limits for and what the host kit holds, and is dispatched once the host kit holds one; one that waits for the caster is "held for the caster: "
    /// - `left_out`: in a bound repo whose team and host kit can be read: each role, in the team's order, with what its range on this host leaves out: {role, left_out: [{harness, why}]} — a harness its roster entry and the host kit allow it and it holds no limits for, while it holds limits for another, why `no limits for pi` (`afkconvoy help roster`)
    /// - `unmatched`: in a bound repo, where some role's roster entry matches nothing in the host kit: {role, why} — the role ranges over the host kit whole, less what it holds no limits for
    /// - `nothing_safe`: in a bound repo, where some role has no harness on this host it holds limits for: {role, why} — its stages wait, and every other stage goes on
    /// - `unmatched_models`: each roster model matching nothing held, {role, harness, model, holds, said}, with the nearest names or the two matches
    /// - `caster`: while a caster runs or one is due:
    ///
    ///   {running_since, due, held, to_cast} — when the one that runs was started (null: none runs), whether a tick starts one now, the frontier's issues held back for it, and every issue a caster started now is given. An uncast issue with a choice to make waits for one, for ten minutes at most from its start: `afkconvoy cast --help`
    /// - `waiting_on_human`: every open issue nobody holds that waits for a human, lowest number first: {issue, title, why, since, answer} — why in a few words, since when in epoch seconds where GitHub said, and the line that answers it, to run as it is but for what stands between < and >. They are: an escalated issue, one whose records call for a human, one at a stage that is a human's, one whose workflow ended and that is still open, and the report of a patrol's failed run
    /// - `escalating`: in a bound repo, when there are any: the unclaimed issues a tick hands to a human, with why: {number, title, reason}
    /// - `mine`: the claims of `--instance`: {number, title, status, pr, declared} — `declared` is true while a verdict declared under the claim waits to be acted on: the claim takes no slot meanwhile. In a bound repo with `due`, the stage a worker is to be started on once the claim's stage has declared, where a role performs one. status is one of those below
    /// - `peer_live`: other instances' claims within the lease — never touched
    /// - `stale`: other instances' claims past the lease, on open issues — a tick takes them
    /// - `stale_closed`: the same on closed issues — a tick deletes them (each of the three: {number, instance, sha})
    /// - `free_slots`: how many more claims the instance may take
    /// - `concurrency`: {in_force, said_by}: how many claims the fleet of this checkout holds at work at once on this host — what `free_slots` counts from, beside the claims a fleet lock leaves alone and those whose worker declared — and who says it: `checkout`, where `afkconvoy run --concurrency` said it of this checkout, which keeps it on the checkout list; `config`, where the repo config's `concurrency` does; `default`, where neither does (3)
    /// - `now`: the time the plan was made at, in epoch seconds
    /// - `locks`: the fleet locks standing: the open issues that carry the lock label, lowest number first — none: []. While there is one, the frontier holds lock issues only, and every issue a tick would otherwise dispatch or hand to a human is under `excluded` with the reason "a fleet lock stands (#n)"
    /// - `admission`: whether a tick would start a new worker on this host now (`afkconvoy run --help`, ADMISSION): {admits, held_back_by, shares, thresholds, reading}. held_back_by is each threshold the host is past, as a tick's line says it: none, and it admits. shares are the percentages in force — load_percent_of_cores, memory_free_percent, disk_free_percent — and said_by_host the host settings that said one, by key (admit_load, admit_memory, admit_disk, admit_inspect_after); thresholds what they come to on this host — load, memory_free_percent, disk_free_bytes; reading what the host reads now — cores, load, memory_free_percent, disk_free_bytes, disk_size_bytes, each null where it could not be read
    /// - `worker_no_proxy`: every harness, with {harness, addition, no_proxy, NO_PROXY}: the host list added and the resulting login shell values, with this command's --worker-no-proxy flags. A null addition leaves the inherited spellings alone
    /// - `held_back`: while admission holds: each threshold the host is past, as data: {by: "load", load, limit} in hundredths, {by: "memory", free, floor} in percent, {by: "disk", free, floor} in bytes. The frontier is still what a tick would dispatch once it admits
    /// - `intake`: in a bound repo: {"open", "limit"} — the open issues taken in by workers, counted from their intake records, against the repo's `intake_limit`. With `open` at `limit`, `afkconvoy intake` refuses. An issue a human labelled is not among them
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
    /// - `proposals`: in a bound repo whose team names who proposes precedents: each ruling of a human that is being proposed, or is still to be: {issue, comment, standing} — the issue ruled on and the id of the comment that is the ruling. standing says what a tick does about it:
    ///
    ///   - `due`: a tick starts its worker, once the frontier is served
    ///   - `full`: the open proposals on the team's repo have reached the team's `max_open`
    ///   - `locked`: a fleet lock stands: nothing is proposed
    ///   - `running`: its worker, started by `--instance`, may still be at it
    ///   - `peer_running`: a live peer started its worker
    ///   - `stale_run`: it is claimed by a peer presumed dead: a tick ends it
    /// - `slow_gate`: while the gate is slow: {gate_seconds, host, runs, issue}. On `host`, the last {const:SLOW_GATE_LANDINGS} landings on the base branch that have a recorded run of the config's `gate_command` with a time all took more than {const:SLOW_GATE_PERCENT}% of the gate limit, `gate_seconds`. runs are those {const:SLOW_GATE_LANDINGS}, newest first: {commit, seconds}. issue is the open issue that says so — the lowest open one carrying the label `slow-gate` — or null: a tick opens one (`afkconvoy tick --help`)
    ///
    /// A claim's `status`, under `mine`, is one of:
    ///
    /// {table:claim_status}
    ///
    /// The probe's warnings go to stderr. Errors: a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run; `claude` is among them when no custom provider is set, as `run` would then start its workers with it); the repo fails the startup probe (the remote has no base branch, its protection refuses the fleet's push, or the remote takes none of the fleet's refs); the config cannot be read; GitHub or the remote cannot be read.
    ///
    /// ## What trips you up
    ///
    /// - Without `--instance` nobody's claims are its own: `mine` is empty, a running fleet's claims show under `peer_live`, and `free_slots` is the whole concurrency. To see a running fleet as it sees itself, pass its instance id — `takeover --list` shows every id the remote remembers.
    /// - A lock that stands is not thereby worked. A lock issue is on the frontier only when it would be without the lock: with the ready label in an unbound repo, a flow label in a bound one, no epic label, no open blocker, no claim. `excluded` says which of these keeps it off — and the lock stands until the issue is closed.
    /// - `frontier` is what the rebuild read, and `plan` reads nothing more. An issue that gains an open blocker after it is still listed; a tick reads each issue's blockers once more just before it claims it, and skips the issue then.
    /// - A patrol is due or it is not: one that missed ten intervals runs once. And a run still going takes one slot of `concurrency`, like a claim on an issue.
    /// - With a lock standing, `free_slots` counts as free the slot of every claim of `--instance` the lock leaves alone: a lock issue does not wait for them.
    /// - `free_slots` counts as free the slot of every claim of `--instance` that is `declared`: `mine` may hold more claims than the concurrency, and the frontier does not wait out the grace period of a worker that already declared.
    /// - "Changes nothing" is about fleet state. When it probes it still pushes to the remote, and deletes there: it needs push access, and it is not a read-only command.
    /// - A slow gate is read off the recorded gate runs the remote still has: a record is swept a day after its run, a landing with no record or with a record that states no time is passed over, and {const:SLOW_GATE_LANDINGS} landings spread over hosts none of which has {const:SLOW_GATE_LANDINGS} say nothing. After an issue for a slow gate closed, only the landings made since count.
    /// - A probe that stands is trusted, not checked: a command uninstalled or logged out since, or a rule the remote gained, shows as the error of whatever runs into it — or, for the ref layout, not until the next probe. `run` probes at every start.
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
    /// To have a repo worked: after `config` answers and `plan` shows the frontier you expect. Run it again after the host runner was stopped or killed, or the host restarted: it starts every checkout on the list, each where its fleet instance left off. And once after an upgrade of afkconvoy (`brew upgrade afkconvoy`), in any checkout: it moves the host runner, and every checkout on the list, onto the new version.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo or naming one. It returns once that checkout's fleet instance has ended its first tick; the host runner ticks on, in the background, until it is stopped.
    ///
    /// ## What it reads and writes
    ///
    /// The checkout list: checkouts.json in the cache directory, kept on this host and never pushed. A checkout goes on it once its fleet instance's first tick ended, with the `--instance`, `--host` and `--worker-no-proxy` its `run` named; one refused before its first tick is not left there. A `run` of a checkout on the list whose fleet instance does not run starts it with those, but for each that `run` names itself.
    ///
    /// The checkout's concurrency, with its entry: how many claims its fleet instance holds at work at once on this host. `--concurrency 6` says it — a whole number, at least 1 — and `--concurrency default` changes it back to the repo config's `concurrency`; a `run` that names none leaves it as it is. Until the checkout says one, its fleet instance goes by the repo config's, or 3 where that says none; once it says one, the checkout's wins. Unlike the `--instance`, `--host` and `--worker-no-proxy` it named, which hold from the start of its fleet instance, it is read at each tick, as the host settings are: a change holds at the next tick, and the fleet instance is never started again for it. Lowered below the claims held, it ends no worker: those claims are worked to the end — the tick's line counts those past it, as `holding 2/2 · 1 past the bound` — and no new one is taken until fewer are held. A checkout the list does not hold yet goes by it from its first tick, which puts it on the list with it. A host runner started again keeps it; `plan`, `doctor` and `checkouts` say the one in force and who says it, and `afkconvoy config` the repo's alone.
    ///
    /// The host settings, in the same file: what this host says for every fleet instance on it — whether it may be put to sleep (`--no-host-sleep`, `--host-sleep`) and in which hours of its local time (`--host-sleep-hours`), the limits past which a worker is hot (`--hot-cores`, `--hot-memory`, `--hot-written`), the shares admission holds back at (`--admit-load`, `--admit-memory`, `--admit-disk`, `--admit-inspect-after`) and the panel's port (`--panel`). A `run` that names one — by its flag, or by the variable of its environment that stands for it, the flag winning: AFKCONVOY_NO_HOST_SLEEP=true, AFKCONVOY_HOST_SLEEP_HOURS, AFKCONVOY_HOT_CORES, AFKCONVOY_HOT_MEMORY, AFKCONVOY_HOT_WRITTEN, AFKCONVOY_ADMIT_LOAD, AFKCONVOY_ADMIT_MEMORY, AFKCONVOY_ADMIT_DISK, AFKCONVOY_ADMIT_INSPECT_AFTER — changes it for the host before it hands its checkout over. Every fleet instance on the host reads them as it goes — admission at each tick, the hot limits at each sample, host sleep at each check — so a change holds for all of them at once, and none is started again for it. A `run` that names none changes none, and nothing else changes them: `tick`, `plan` and `doctor` read them as they are, and a host runner started again keeps them. `--host-sleep` changes `--no-host-sleep` back, and `default` changes any other back to its default (`--hot-cores default`, `--host-sleep-hours default`). `afkconvoy doctor` says those in force, as `host.settings`.
    ///
    /// The host runner: one process of the host, holding a fleet instance for each checkout on the list, each ticking as a fleet instance always has. `run` asks it on its socket in the cache directory (host-runner.sock) — the one socket of the host: `stop`, `wake`, `panel --open`, `exclusive` and `doctor` ask it there too, each naming its checkout, and no fleet instance or repo has one of its own; one nobody listens on, which a host runner killed with no goodbye left, is removed by whoever finds it — and starts it where none answers: in a process group of its own, so the terminal that ran `run` can close, in /, with a fresh login shell's environment — the names your login shell (`$SHELL -lic`) exports, and none of those of the shell that ran `run`. A variable exported only in that shell, or one a session set for itself (Claude Code's, orca's), reaches neither the host runner nor any worker it starts: put it in your login shell's rc files. It keeps the cache directory and the config directory of the `run` that started it. Started again, it starts every checkout on the list, each with what its `run` named, under the host settings the file keeps; a checkout whose fleet instance cannot start then is broken (below), and stays on the list.
    ///
    /// Before anything else, `run` asks the host runner for its version. One of an older release than this `run`'s — after `brew upgrade afkconvoy` — is stopped as SIGTERM stops it: each fleet instance finishes the step it is in, and the checkout list is kept. Once it has let the host go, `run` starts a host runner of its own version, which starts every checkout on the same list, and then adds its own. What the old one held for every fleet instance goes with it: the next one's fleet instances count as idle for host sleep from their first tick, the inspection's turn starts free, and an `exclusive` command waiting in the old one's queue waits in the next one's, from when it first asked (`afkconvoy exclusive --help`). One of a newer release refuses this `run`, which changes nothing. A build past a release (`afkconvoy --version` says `0.6.0+11.g9f58035`) is of that release, and one of the same release as the host runner's is handed the checkout as ever.
    ///
    /// The host runner's log: host-runner.log in the cache directory, moved aside to host-runner.log.1 once past a MiB. One line, stamped in UTC, as it starts and as it stops, as it replaces a host runner of an older release (`host runner upgraded: from afkconvoy 0.5.13 (pid 4100) to afkconvoy 0.6.0 (pid 4242)`), as each fleet instance starts and ends, as one breaks and is back, as a checkout is added, refused or removed, as anything in it panics (`panicked at <file>:<line>: <what it said>`), and each line a fleet instance says outside its tick log, after its checkout: its warnings, the `start:` lines after its first, and the `panel:` line:
    ///
    /// ```
    /// fleet instance broken: /Users/me/Developments/hkj: it panicked: <what it said> · since 2026-10-24 09:12:00 UTC · started again at 2026-10-24 09:13:00 UTC, in 60s
    /// fleet instance back: /Users/me/Developments/hkj: me-h1-5e6f7a8b, broken since 2026-10-24 09:12:00 UTC, for 75s
    /// ```
    ///
    /// Two kinds of checkout are refused, with an error and nothing on the list: one whose fleet instance is refused before its first tick — it fails the startup probe, say — which is taken off the list when it was on it; and a second checkout of a repo already on the list, a clone or a worktree of it: a host runs one fleet instance per repo, and the error names the checkout to run it from.
    ///
    /// A fleet instance is broken when its loop panics or ends in an error, or when a start of it that the host runner made with nobody asking — from the list, or after it broke — ends before its first tick: its startup probe no longer passes, say. It harms no other: the host runner and every other fleet instance go on, and its checkout stays on the list. The host runner starts it again after a backoff: {const:BACKOFF_FIRST_MINUTES} minute after it first broke, twice the wait before at each break in a row — one within {const:BACKOFF_MOST_MINUTES} minutes of the start before it — and {const:BACKOFF_MOST_MINUTES} minutes at the most. A `run` of its checkout starts it at once, with what that `run` names laid over what the list kept; a start of it that ends before its first tick is then refused as any is, and taken off the list. It is back once a start of it again has ended its first tick. Broken — with why, since when, and when it is started again — and back are lines of the host runner's log and of its tick log, and `afkconvoy doctor` in its checkout says it is broken, why, and when it is started again. A tick that fails is no break: it is a line of its own, and the loop goes on.
    ///
    /// Then everything a fleet instance does. It makes the startup probe — at every start, whatever probe passed before — takes every claim stamped with this host — on an issue, on a patrol's run, on the proposal of a precedent — removes the heartbeats that are dead and hold nothing (those of the instances it took from, this host's past the lease, anyone's older than a day) — then ticks: each tick rebuilds the working set from GitHub and the remote and makes every transition it calls for — claims and heartbeats on the remote, workers in orca worktrees, comments, labels and blocked-by edges on issues, status boards, the landing train's worker. Between ticks it sleeps; a worker's `wake` ends the sleep, and a tick that left the next one a worker to start is followed with no sleep.
    ///
    /// Once every fleet on this host has been idle {const:HOST_IDLE_MINUTES} minutes it puts the host to sleep (`pmset sleepnow`) — a host sleep — within the host's sleep hours, unless the host settings keep it awake: `--no-host-sleep`, for every fleet on it, until a `run` says `--host-sleep`. The sleep hours are {const:SLEEP_HOURS} of the host's local time, unless its host settings say others: `--host-sleep-hours 22:00-06:00`, until a `run` says `--host-sleep-hours default`, which changes them back. The fleet never wakes the host: one put to sleep within its sleep hours sleeps on past their end, until something else wakes it. Idle is holding no claim — on an issue, a patrol's run, a proposal — nothing on the landing train, and an empty frontier; an escalated issue and a patrol not yet due leave a fleet idle. Every fleet on this host is a fleet instance of its one host runner, which holds how each stands, as each says it as every tick of its ends: whether it keeps the host awake, since when it has been idle or not, and when it last worked. No fleet instance asks another.
    ///
    /// ADMISSION. A tick starts no new worker while its host is busy: its 1-minute load average past 200% of its cores, less than 10% of its memory free (what the kernel counts as available: `sysctl kern.memorystatus_level`, or MemAvailable in /proc/meminfo), or less than 1% of the disk under the checkout free (`df`). The issue stays on the frontier for the next tick, and so do a stale claim, a due patrol and a due proposal. Workers already running are left alone, and so are the landing train and the worker a declared verdict starts. The host says its own shares, each a whole percent, in its host settings: `--admit-load` (of its cores), `--admit-memory` and `--admit-disk` (free) — `--admit-load 400` lets the load reach four times the cores, and `--admit-disk 0` never holds for disk. A value that is no whole percent is refused, and `default` changes a share back. What cannot be read holds nothing back. `plan` and `doctor` say the thresholds in force and what the host reads.
    ///
    /// When admission has held back for one reason — load, memory or disk — at every tick for ten minutes in a row (`--admit-inspect-after`, in whole seconds, a host setting, says the host's own span), the fleet instance starts an inspection of the host: the stock consult, in a checkout of its own, given what was taken down as it started — admission's reading and shares; for disk, what each of this host's worktrees of every fleet holds, with its issue, its stage and whether a worker holds it, the landing train's among them, the fleet's caches and the largest directories of the whole volume under the home folder outside them (`du -x`, at most five minutes); for load or memory, the process trees of each worker and of the train, and the hottest processes outside them. Holding back for reasons that change starts none. It runs once an episode, which ends once admission has admitted for five minutes, and not where another fleet of the host inspected it in the same episode. It says what it found with `afkconvoy inspected`: reclaimable, fleet or not-its. For reclaimable it frees what can be rebuilt under the home folder — what git ignores in a checkout no process uses, a closed issue's worktree, a tool's cache — and names what cannot, never deleting it (`afkconvoy inspected --help`); for fleet it opens one issue in the repo, with the escalate label and the label fleet-outgrows-host, and no other while that one is open. What it found is a line of the log, and shown by the panel and `doctor` beside admission's reading.
    ///
    /// A fleet instance idle {const:HOST_IDLE_MINUTES} minutes checks then, tick due or not, whatever the hour, and again {const:HOST_IDLE_MINUTES} minutes after a check that kept the host awake — or, where that is outside the sleep hours, as they start. It puts the host to sleep only when all hold:
    ///
    /// - the host's local time (`localtime_r`: TZ, else /etc/localtime) is within its sleep hours, the start included and the end not: with {const:SLEEP_HOURS}, 05:59 is and 06:00 is not;
    /// - it worked — held a claim, or had something on the landing train — since the host last woke (`sysctl kern.waketime`): a host woken, by hand or otherwise, stays awake until a fleet on it works again;
    /// - every fleet instance on the host, as the host runner holds how it stands, keeps the host awake none, and each has been idle {const:HOST_IDLE_MINUTES} minutes;
    /// - nobody touched the keyboard or mouse for ten minutes (HIDIdleTime, from `ioreg`);
    /// - every fleet instance, itself included, ticks once more — each other one woken to — and each still finds nothing: idle, as before.
    ///
    /// One check is made on the host at a time: while one waits for the others' ticks, no other fleet instance checks. It waits for a tick each began after it was woken, however long that takes, and no longer for one that ended meanwhile. A check whose own fleet instance is woken while it waits — something changed for its fleet — or told to stop ends there, with the host awake; it is checked again later.
    ///
    /// It samples what each worker's processes take every ten seconds. As one turns hot (`afkconvoy help concepts`, Hot worker) it takes down what shows it — the process tree, a five-second `sample` of the hottest process, the resident memory of late, what its processes wrote, the largest directories of its worktree — and starts an inspection of it once per episode: a worker in the role of the stage's consult, in a checkout of its own, that says what it found with `afkconvoy inspected`. One inspection runs on this host at a time, across its fleets — the host runner holds whose turn it is; it takes no slot of the `concurrency`, and is ended after {const:INSPECTION_MINUTES} minutes. A host inspection is held to the same: of either kind, one runs on the host at a time, and a hot worker waiting for one goes first. While one runs, or a hot worker or the host waits for one, a fleet instance sleeps 30 seconds at most.
    ///
    /// Each fleet instance also keeps a log on this host, its tick log, in a file under the cache directory (runs/; `afkconvoy help cache` says where it is and what moves it): one line when it starts, one as each tick starts — with why, and the command whose wake started it — and one as it ends — what `tick` answers for it — and one of each event that reached it: a wake heard during a tick, a check for host sleep, the host waking, being told to stop. Every command that changes fleet state writes a line of its own to the same file as it acts (its `--help` says so under "What it logs", and [`afkconvoy help tick-log`](help:tick-log) says what the line holds). It keeps every line of the last {const:LOG_KEPT_HOURS} hours, and never fewer than {const:LOG_KEPT_LEAST} lines nor more than {const:LOG_KEPT_MOST}. A line is written whole as it happens, so killing the host runner loses none, and none a command appends while the fleet instance cuts the file back is lost. Nothing reads the log to decide anything; `afkconvoy doctor` reads it to say what the last ticks did, which steps failed, and the wakes, starts, stops and host sleeps of late.
    ///
    /// ## What it answers
    ///
    /// Once its checkout's fleet instance has ended its first tick, with exit 0:
    ///
    /// ```json
    /// {"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj", "added": true,
    ///  "instance": "me-h1-1a2b3c4d",
    ///  "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log", "started": true,
    ///                  "replaced": null},
    ///  "tick_log": "<cache>/runs/hkj-0123456789abcdef.jsonl",
    ///  "first_tick": "tick 1: dispatched #12, #15 · holding 2/6 · frontier 4 · next in 30s",
    ///  "concurrency": {"from": {"in_force": 3, "said_by": "config"},
    ///                  "to": {"in_force": 6, "said_by": "checkout"}},
    ///  "settings": {"hot_cores": {"from": null, "to": 4.0}},
    ///  "warnings": []}
    /// ```
    ///
    /// - `repo`: the repo the checkout is of, as its remote `origin` names it.
    /// - `added`: false when the checkout was on the list already. Where its fleet instance runs, the flags of this `run` that are its fleet instance's own change nothing, and the answer is that instance, with its own first tick.
    /// - `host_runner`: the one that answered — its pid, its version and its log — and whether this `run` started it. `replaced`: the host runner of an older release this `run` stopped to start it, as {"pid": 4100, "version": "0.5.13"}; null where it stopped none.
    /// - `tick_log` and `first_tick`: the fleet instance's tick log, and the line that ended its first tick. `<cache>` stands for the cache directory, whole in the answer.
    /// - `concurrency`: what this `run` changed of its checkout's concurrency, as {from, to}, each the one in force before and after, as `plan` says it: {in_force, said_by} — said_by `checkout`, `config` (the repo config's `concurrency`) or `default`. null where it named none, or named the one in force.
    /// - `settings`: what this `run` changed of the host settings, each by its key — its flag's name, as `hot_cores` for `--hot-cores` — as {from, to}, in its flag's units; null is the default. {} where it named none, or named each as it was.
    /// - `warnings`: what its fleet instance warned of until its first tick ended — the startup probe's warnings among them — also on stderr; empty where this `run` waited on no start.
    ///
    /// Each fleet instance's lines — `run` prints none of them. Its tick log holds them, and `afkconvoy doctor` reads them back, but for the `start:` lines after the first — the terms of host sleep, and the claims its start took — which are lines of the host runner's log, after the checkout (`/Users/me/Developments/hkj: start: took #4, #7 stamped with this host`):
    ///
    /// ```
    /// start: run as tester-h1-1a2b3c4d on h1, its workers started with `claude`
    /// start: puts this host to sleep within its sleep hours, {const:SLEEP_HOURS}, once every fleet on it has been idle {const:HOST_IDLE_MINUTES} minutes (`--host-sleep-hours` says others, `--no-host-sleep` keeps it awake)
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
    /// The second `start:` line is there unless the host settings keep the host awake as it starts. Every tick has a line as it starts, before it does anything, naming why: `timer` (its sleep ran out), `woken` (a wake ended the sleep, or was heard during the tick before: `woken by` the command that sent it, and what that acted on, where it named them), `start` (the fleet instance's first) or `follow-up` (the tick before left a worker to start, or could not reach GitHub, so it runs with no sleep). A wake heard during a tick is a line as it is heard — the first of each tick — and the next tick starts as that one ends: ticks never overlap. A check for host sleep is a line, and so is the host waking from a sleep, as the fleet instance notices it — a rest that took a minute past its length — and being told to stop:
    ///
    /// ```
    /// tick 4: started (woken by verdict #121)
    /// tick 4: started (woken)                          a wake that named nothing
    /// tick 5: started (follow-up)
    /// host sleep: every fleet on this host has been idle long enough, and ticked once more: put the host to sleep
    /// host sleep: kept awake: the keyboard or mouse was used 40s ago
    /// host sleep: kept awake: the host could not be put to sleep: <why>
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
    /// A tick's line names what it did — lost, escalated, released, left, settled, ended, advanced, unlocked, reclaimed, dispatched, skipped (a frontier issue found blocked just before its claim, with what blocks it), nudged, waiting on its own background work (with since when: `#7 (since 2026-10-20 23:03 UTC, 12m ago)`), continued, restarted, tended the train, abandoned the train — then, when admission held back a worker it would have started, which threshold did (`admission holds back new workers: the load 31.20 is past 28.00, 200% of its 14 cores`), then the claims held out of the concurrency, and the frontier left. The claims a fleet lock has it leave alone take no slot and are counted apart (`holding 1/3 · 2 left alone`), and so are those whose worker declared a verdict not acted on yet (`holding 3/3 · 1 declared`), as are those past the bound — a start took them, or the checkout's concurrency was lowered under them (`holding 3/3 · 1 past the bound`). A step that failed is in the line as `FAILED <step> #<issue>: <why>`, and a tick that could not start as `tick N: FAILED: <why>`; the loop goes on either way. One cause that stopped several steps is said once: `FAILED 7 steps: <why>`.
    ///
    /// `advanced #12 → review`: the tick read a verdict that left #12 at `review`. Where a role performs that stage the same tick starts its worker, and says so: `dispatched #12`. An issue advanced to no stage — its workflow ended, or it was parked — is named alone.
    ///
    /// `escalated #15 (workup: consult let it through)`: the tick handed #15 to a human, whichever step did, with why in a few words — the stage first, where it was at one. It is named as nothing else. The comment on the issue says why in full and ends with the line that answers it; `afkconvoy doctor` lists it under `waiting_on_human`.
    ///
    /// A change made on GitHub, or by another host's fleet, shows on this host at the next tick, and in no other way: that tick reads GitHub and the remote, as every tick does. Nothing reaches a fleet instance sooner — no webhook, no port — and `afkconvoy wake` makes that next tick start now. Where the tick acts on such a change, its line says what changed since the tick before, and who changed it where that is known:
    ///
    /// ```
    /// tick 8: lost #46 to instance beta-1a2b3c4d on host h2 (takeover) · holding 1/3 · frontier 0 · next in 90s
    /// tick 9: dispatched #45 (labelled ready-for-agent since tick 8) · holding 2/3 · frontier 0 · next in 90s
    /// tick 9: released #231 (the issue was deleted) · holding 1/3 · frontier 0 · next in 90s
    /// tick 9: left #97 (closed by @alice) · holding 1/3 · frontier 0 · next now
    /// tick 9: skipped #320 (blocked by #379, added since tick 8) · holding 1/3 · frontier 0 · next in 90s
    /// ```
    ///
    /// `lost #46`: a claim this instance held is another's now — taken by a takeover, or `(stale: past the lease)`, as a stale claim, this instance's heartbeat being past the lease. `released #231`: its issue was deleted, or `(the issue is gone from this repo)`, transferred. `left #97`: a claim with no PR whose issue was closed, by whoever GitHub says closed it (`closed on GitHub` where it says no one), is settled — nothing of the fleet's landed. `labelled …` and `added since …` say what the tick before saw otherwise. A change the tick does nothing about is in no line; nor is one made before the fleet instance's first tick, which has no tick before it, nor anything of the kind in what `afkconvoy tick` answers.
    ///
    /// `next now` in place of `next in <n>s`: the tick settled an issue, or advanced one without starting what follows, and left a slot free, so what follows — a consult, the join of a PR, an issue the landing unblocked — is the next tick's to start, and that tick starts with no sleep.
    ///
    /// So does the tick after one that failed on a read GitHub could not be reached for (`GitHub could not be reached: …`): nothing says the next read fails too. At most {const:BACK_TO_BACK} ticks run back to back; then it sleeps, whatever the last one left.
    ///
    /// With `--panel <port>` — the panel's port, a host setting — its checkout's fleet instance serves the panel on that port from this start until it ends: the fleet as a web page — every workflow as a line of its stages, the issues at work, queued and held back at each, the landing train — brought up to date in the browser after every tick, by itself. `afkconvoy panel --help` says what the page shows. Its address is a line of the host runner's log, after the checkout:
    ///
    /// ```
    /// /Users/me/Developments/hkj: panel: http://127.0.0.1:7070/
    /// ```
    ///
    /// Started without it — and started again by the host runner, which serves no panel by itself — a fleet instance serves the same page once `afkconvoy panel --open` asks it to, on the panel's port of the host settings where they say one, from any worktree of the repo on this host — while it sleeps or in the middle of a tick, which goes on — and then logs that line, wherever its ticks have got to. `panel --open` answers the address again later, of a panel opened either way.
    ///
    /// Errors, before the first tick, as {"error": …} with a non-zero exit: the host is not macOS and its host settings do not keep it awake (`--no-host-sleep`), or this `run` says `--host-sleep` there; the host settings cannot be read or written; the panel's port is taken; a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run); the repo fails the startup probe; the config cannot be read; the host kit cannot be read; the worker launch command of a harness the fleet could be asked for resolves to nothing, or — in a bound repo — does not take the flags that start a worker in its role; another checkout of the repo is on the checkout list; the checkout has no remote `origin`; the login shell did not answer within 20 seconds; the host runner is of a newer release than this `run` (the error names both versions, and nothing was changed); the host runner did not say its version within 10 seconds; the host runner of an older release, told to stop, still held the host after 30 seconds; no host runner answered within 35 seconds; the host runner ended before it answered, or is stopping; the checkout's fleet instance is stopping, told by `afkconvoy stop`; no first tick within 30 minutes; the checkout list cannot be written. Each error a host runner gave says which, as `host_runner`.
    ///
    /// ## What trips you up
    ///
    /// - Stop the host runner with `afkconvoy stop --all`, or SIGTERM to its pid, `host_runner.pid` in any `run`'s answer: each fleet instance finishes the step it is in, makes none of the steps left in its tick, and ends — within a second when it sleeps — and the host runner exits once every one has, keeping the checkout list: the next `run` starts every checkout on it. No claim is released on the way out and no worker is stopped: the workers work on, and the next start on this host takes the claims back. Holding no claim, a fleet instance removes its heartbeat from the remote; holding one, it leaves it, and that next start removes it. A second signal ends the host runner at once, as SIGKILL does, with no goodbye: what that leaves, a later start on this host removes once it is past the lease. A patrol's run and a proposal it takes back are ended by its first tick once their worker is gone or has stopped, as any run of its own.
    /// - Ctrl-C while `run` waits ends that `run` alone: the host runner is in a process group of its own, and goes on with the checkout.
    /// - Of a `run` whose checkout's fleet instance runs already, the flags that are that instance's own — `--instance`, `--host`, `--worker-no-proxy` — change nothing: `afkconvoy stop` it, and run again with those you want. `afkconvoy stop --help` says how to stop one checkout and how to stop them all. The host settings it names change all the same, for that instance and every other on the host, and so does its checkout's `--concurrency`, for that instance alone, from its next tick.
    /// - A checkout's concurrency is that checkout's, on this host: another host works the repo at its own, and the repo config's `concurrency` is what a checkout that says none goes by. Raising it for an afternoon, or lowering it while the host is needed for something else, is one `run`, and no commit.
    /// - Without `--instance` each start mints a new instance id, named for this host: the answer says it as `instance`, and `afkconvoy doctor` as `fleet.instance`, for `plan --instance` and the commands that make one transition by hand.
    /// - A host setting is the host's, not a `run`'s or a repo's: `--hot-cores 4` named in one checkout holds for every fleet instance on the host until a `run` names it again, and `--hot-cores default` changes it back. The variables that stand for the flags are read from the environment of a `run` and of nothing else — not of the host runner, nor of `tick`, `plan` or `doctor` — so one exported in your shell's rc files is named again by every `run` started from that shell.
    /// - The panel is served to this host only, and shows a tick, not this second: between two ticks it changes nothing but its countdown. It costs GitHub no read of its own.
    /// - Every worker launch command comes from the host kit: run `afkconvoy kit` to write it again. It is $XDG_CONFIG_HOME/afkconvoy/kit.json, or $HOME/.config/afkconvoy/kit.json, kept on this host and never pushed. `--worker-command` is gone. On a host with no host kit, `run` writes one as `afkconvoy kit` does, at a terminal or not, asking nothing: it says on stderr where the file is, what it holds and what was left out, and goes on to its first tick. A harness absent from the kit has no implicit stock command. Harness selection still follows a switch, a stage or patrol, then the repo's config.
    /// - The fleet starts every worker launch command under `nice`, with a `TMPDIR` of the worktree's own, adding its harness's model flag where the worker runs a model — and, in a bound repo, what puts the worker in its role, under the guard: [`afkconvoy help worker-launch`](help:worker-launch) says, harness by harness, the line each worker is started with, what each part is for and what loads from where.
    /// - In a bound repo, before any worker is started, the command of every harness the fleet could be asked for is checked: the host kit's first, each harness of each role's range (`afkconvoy help roster`), and the stock consult's. One that cannot be started refuses the start, named with its harness and the range that calls for it, and no worker at all is started: no harness is ever started in another's place. So does a role whose range on this host is empty: the refusal names the role, what the roster allows it, what the host kit holds and what the range leaves out. A range leaves out each harness the role holds no limits for while it holds limits for another, so no worker runs a harness it holds no limits for; `plan` and `check` name each harness left out.
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
        /// and never pushed; `default` changes it back [default: the repo config's
        /// `concurrency`]
        #[arg(long, value_name = "CLAIMS", value_parser = concurrency::claims_or_default)]
        concurrency: Option<To<NonZeroUsize>>,
        #[command(flatten)]
        agent: AgentArgs,
        /// The panel's port, a host setting, and serve the panel — the fleet as a web page,
        /// live — on it from this start; `default` changes it back [default: any free port]
        #[arg(long, value_name = "PORT", value_parser = port_or_default)]
        panel: Option<To<u16>>,
        /// Keep this host awake: no fleet on it puts it to sleep. A host setting, for every
        /// fleet on it until a `run` says `--host-sleep`, which changes it back
        #[arg(long, env = NO_HOST_SLEEP_ENV)]
        no_host_sleep: bool,
        /// Let the fleets on this host put it to sleep once every one has been idle ten
        /// minutes, within its sleep hours, as they do by default: a host setting, for every
        /// fleet on it. Said, it holds over `--no-host-sleep` and its variable
        #[arg(long)]
        host_sleep: bool,
        /// The hours of this host's local time a fleet may put it to sleep in, from the first,
        /// included, to the second, not — past midnight where the second comes first
        /// (22:00-06:00); 00:00-24:00 is any hour: a host setting, for every fleet on it;
        /// `default` changes them back [default: 00:00-06:00]
        #[arg(
            long,
            env = HOST_SLEEP_HOURS_ENV,
            value_name = "HH:MM-HH:MM",
            value_parser = hours_or_default
        )]
        host_sleep_hours: Option<To<afkconvoy_core::host_sleep::Hours>>,
        /// Seconds every fleet on this host must have been idle before it is put to sleep
        #[arg(long, hide = true, default_value_t = IDLE_WAIT)]
        host_sleep_after: u64,
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
        /// The seconds admission holds back for one reason before the host is inspected: a
        /// host setting; `default` changes it back [default: 600]
        #[arg(
            long,
            env = ADMIT_INSPECT_AFTER_ENV,
            value_name = "SECONDS",
            value_parser = whole_or_default
        )]
        admit_inspect_after: Option<To<u64>>,
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
    /// To stop working one repo on this host and leave the others worked: `afkconvoy stop` in its checkout, or naming it. To stop every repo — to start every fleet instance again with flags of its own, say — `afkconvoy stop --all`: the next `run`, in any checkout, starts every checkout on the list again. An upgrade needs none: one `run` moves the host runner onto the new afkconvoy (`afkconvoy run --help`).
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
    ///  "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log"}}
    /// ```
    ///
    /// - `removed`: false where the checkout was not on the list. Where no fleet instance of it ran either, nothing changed, and that is all it answers: a checkout not on the list exits 0.
    /// - `instance` and `stop`: the fleet instance that stopped, and the `stop:` line its tick log keeps; null where none ran — the host runner could not start it, or no host runner runs.
    /// - `host_runner`: the one that answered — its pid, its version and its log — or null where none runs. `<cache>` stands for the cache directory, whole in the answer.
    ///
    /// With `--all`, once the host runner has ended:
    ///
    /// ```json
    /// {"stopped": [{"checkout": "/Users/me/Developments/hkj", "instance": "me-h1-1a2b3c4d",
    ///   "stop": "stop: it left its heartbeat with the claims it holds: the next start on this host takes them"}],
    ///  "checkouts": 2,
    ///  "host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log"}}
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
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Stop {
        /// Stop every fleet instance and then the host runner, keeping the checkout list
        #[arg(long, conflicts_with = CHECKOUT_ARG)]
        all: bool,
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
    /// The checkout list: checkouts.json in the cache directory (`afkconvoy help cache` says where it is). The host runner, asked on its socket which checkouts it holds a fleet instance of: where none runs, the list alone answers. Each checkout's tick log, for whether its fleet instance is broken and for its last tick, and its config, for its concurrency. It writes nothing, asks neither GitHub nor the remote, and makes no startup probe: it is safe beside a live fleet, as often as you like.
    ///
    /// ## What it answers
    ///
    /// ```json
    /// {"host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log"},
    ///  "checkouts": [{"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj",
    ///   "instance": "me-h1-1a2b3c4d", "state": "running", "broken": null,
    ///   "ticked": 1767225600, "idle": false, "holding": 2, "frontier": 1,
    ///   "concurrency": {"in_force": 6, "said_by": "checkout"},
    ///   "tick_log": "<cache>/runs/hkj-0123456789abcdef.jsonl"}]}
    /// ```
    ///
    /// - `host_runner`: the one running — its pid, its version and its log — or null where none runs. `<cache>` stands for the cache directory, whole in the answer.
    /// - `checkouts`: each checkout on the list, in its order; [] with none:
    ///
    ///   - `checkout`: its root. `repo`: the repo its remote `origin` names, as `run` answers it; null where it is no git checkout any more
    ///   - `state`: `broken` — its fleet instance is broken (`afkconvoy run --help`), as its tick log says, whether or not a host runner runs to start it again; else `running` — the host runner holds its fleet instance; else `stopped` — no host runner runs, or it holds none of it. `afkconvoy run` in the checkout starts a broken or stopped one at once
    ///   - `broken`: as `afkconvoy doctor` answers it under `fleet`: {why, since, next, instance} — why, as it last broke; since when, and when the host runner starts it again, in epoch seconds, null where no host runner runs; and the fleet instance that broke, "" where none had started. null where it is not broken
    ///   - `instance`: its fleet instance id: the one running, null until its start is logged; else the last one its tick log names, null where it names none
    ///   - `ticked`: when that fleet instance's last tick ended, in epoch seconds; null with none
    ///   - `idle`, `holding`, `frontier`: as that tick ended: whether the fleet instance was idle — it held nothing, nothing of its was on the landing train, and its frontier was empty — the claims it held, and the frontier issues it left undispatched (`afkconvoy tick --help`). null with no tick, or one that failed whole; `idle` null too of a tick logged by an older release
    ///   - `concurrency`: {in_force, said_by}: how many claims its fleet instance holds at work at once on this host, and who says it, as `afkconvoy plan` answers it: `checkout` (`afkconvoy run --concurrency`), `config` (the repo config's `concurrency`) or `default`. {error} where its config cannot be read, or it is no git checkout any more
    ///   - `tick_log`: the file its fleet instance logs its ticks in, as `run` answers it; null where it is no git checkout any more
    ///
    /// It exits 0 whatever the checkouts' states. Errors, as {"error": …} with a non-zero exit: the checkout list cannot be read; the host runner cannot say which checkouts it holds — it is of an older afkconvoy, which `afkconvoy run` in any checkout moves onto this one; it ended before it answered; or it answered nothing within 10 seconds.
    ///
    /// ## What trips you up
    ///
    /// - `idle`, `holding` and `frontier` are of the last tick, not of now: those of a stopped or broken checkout are as its fleet instance last ticked, which `ticked` dates. `afkconvoy doctor`, in the checkout, reads GitHub as it is now.
    /// - A checkout `run` is adding is on no list until its first tick ends, and is not answered until then; one `stop` took off is not answered.
    /// - A broken checkout the host runner starts again is `running` once its tick log keeps that start: it is back once the start's first tick ends, and broken again if it ends before. `afkconvoy doctor` says the same of it.
    /// - It names no panel's address: `afkconvoy panel --open` in the checkout answers it.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Checkouts,
    /// Hold every checkout on this host's checkout list, as `run` starts it in the background
    #[command(hide = true)]
    HostRunner,
    /// Print the panel — the fleet as a web page — as one rebuild shows it now, or open a `run`'s live one
    ///
    /// ## When to run it
    ///
    /// To see the whole fleet at a glance, without a `run` to ask: every workflow as a line of its stages, the issues at work, queued and held back at each, the landing train, and what waits for a human. Save it and open it in a browser:
    ///
    /// ```
    /// afkconvoy panel --instance <id> > panel.html
    /// ```
    ///
    /// To watch a running fleet, do not run this in a loop: have the fleet serve its own panel, live — a page brought up to date after every tick, by itself. Start it with `afkconvoy run --panel <port>`, or ask one that is already running, which is not restarted for it:
    ///
    /// ```
    /// afkconvoy panel --open                the panel's port of the host settings, or any free port
    /// afkconvoy panel --open --port 7070
    /// ```
    ///
    /// and open the address it answers. Run it again whenever you want that address back.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo. `--open` reaches this host's fleet instance of the repo, through the host runner, from any checkout of it but a worker's worktree, and no other repo's.
    ///
    /// ## What it reads and writes
    ///
    /// It writes nothing, on GitHub or on the remote, and makes no startup probe. It makes the rebuild `plan` makes — the open issues and PRs, the claims and heartbeats, the landing train, and in a bound repo the team and every stage record — then asks orca after the worker of each claim of `--instance` on this host.
    ///
    /// With `--open` it reads nothing itself and takes no `--instance`: it speaks to the host runner, on its socket in the cache directory (host-runner.sock), naming its checkout, and has the fleet instance it holds of this repo serve the panel that fleet instance holds. That fleet instance goes on as it is — its claims, its workers and the tick it is in are untouched, and it answers whether it sleeps or ticks. A panel newly opened ends its sleep, as a `wake` does: the page is filled by the tick that follows, then by every tick after it. It is served to this host only.
    ///
    /// ## What it answers
    ///
    /// Not a JSON object: one HTML document on stdout, complete in itself, with no address in it that a browser would fetch. The same fleet state gives the same document.
    ///
    /// - Claims held: the claims of `--instance` that take a slot, of the concurrency
    /// - `Declared`: shown while there are any: the claims whose worker declared a verdict not acted on yet, which take no slot
    /// - a lane: one per workflow; an unbound repo has one, `backlog`, with the two stages `work` and `land`
    /// - a stage: the number in its stop is the workers at it; under it the issues at work — each with its worker's state: busy, stopped and for how long, or no worker — then the queued, which are the frontier, and in a bound repo those held back there, with why
    /// - Landing train: the issues whose PR is on it
    /// - `Patrols`: in a bound repo that names patrols: each, with what it is doing or what holds it back — one held by its `max_runs_a_day` says until when, as a time of day in UTC and how long from now
    /// - With a human: every open issue that waits for a human, with why and the line that answers it: what `doctor` answers as `waiting_on_human`
    /// - left alone: every other open issue, with the reason `plan` gives under `excluded`
    ///
    /// Errors, as {"error": …} with a non-zero exit: the config cannot be read; GitHub or the remote cannot be read.
    ///
    /// With `--open`, one JSON object:
    ///
    /// - `panel`: the address to open in a browser of this host: http://127.0.0.1:<port>/
    /// - `port`: the port in it
    /// - `opened`: true: this call had the fleet instance begin serving it. false: it was served already — since its start, or an earlier `--open` — and this is where
    /// - `instance`: the fleet instance that serves it
    ///
    /// Its errors, as {"error": …} with a non-zero exit, none of which changes anything of the host runner or its fleet instances, which work on as they were:
    ///
    /// - no host runner runs on this host, so there is no live panel to open: start one with `afkconvoy run`, or print the panel once with `afkconvoy panel` — what it answers when none runs
    /// - the panel was not opened, and nothing changed: this host runner (pid <pid>) holds no fleet instance of <repo>: `afkconvoy run <checkout>` adds it
    /// - the panel was not opened, and nothing changed: the fleet instance of <repo> is broken, and waits to be started again: `afkconvoy doctor` says why, and `afkconvoy run <checkout>` starts it again now
    /// - the panel was not opened, and nothing changed: its fleet instance is starting, and holds no panel yet: ask again once its first tick ended
    /// - the panel was not opened, and nothing changed: the panel cannot be served on port <port>: …: the port is taken, or not yours to take: name another, or none
    /// - the host runner did not answer …: ask again: one that opened it says so, with `opened` false
    ///
    /// ## What trips you up
    ///
    /// - Without `--instance` nobody's claims are its own: a running fleet's claims show as "held by <instance>", with no worker's state. `takeover --list` shows the ids.
    /// - A worker's state is this host's orca's to say. A claim whose worker runs on another host shows "no worker" here.
    /// - A panel is served at one address for as long as its `run` runs. `--open --port` on one already served answers the address it has, not the port asked: `opened` is false. To move it, stop that `run` and start it again.
    /// - A page just opened shows nothing of the fleet until the tick it asked for ends: a moment, or as long as the tick the `run` was in takes, and then one more.
    /// - `--port` is `--open`'s, and `--open` takes neither `--instance` nor `--host`: the page is the running fleet instance's own.
    /// - The page a `run` serves is not this one made again and again: it is made from what each tick already read, and adds what that tick did. It costs GitHub nothing.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Panel {
        /// The fleet instance whose claims are its own [default: nobody's]
        #[arg(long, value_parser = InstanceId::parse)]
        instance: Option<InstanceId>,
        #[arg(long)]
        host: Option<String>,
        /// Have this host's `run` for this repo serve its panel, live, and say where:
        /// it is not restarted, and one already serving it says the address it has
        #[arg(long, conflicts_with_all = ["instance", "host"])]
        open: bool,
        /// With --open: the port of this host to serve it on [default: any free one]
        #[arg(long, value_name = "PORT", requires = "open")]
        port: Option<u16>,
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
    /// ## What it answers
    ///
    /// {"did", "errors", "held", "frontier_remaining", "idle", "locks"} — and "held_back_by" when admission held back a worker it would have started
    ///
    /// - `did`: each step taken: {do, issue, outcome}. do is settle, forget, tend_train, cut_train, judge, watch, advance, escalate, end_patrol, patrol, end_proposal, propose, unlock, reclaim, dispatch, slow_gate, board, stage_label or heartbeat; outcome is done, nudged, waiting, continued, restarted, abandoned, nothing, lost, blocked, gone or failed. A step whose worker waits on its own background work says since when it was first seen to (epoch seconds): {do, issue, outcome, waiting_since}. A step on a patrol has a null issue and names it: {do, issue, patrol, outcome}. A step on the proposal of a precedent names the issue that was ruled on. An `advance` that read a verdict and left the issue at a stage names it: {do, issue, outcome, stage} — `land` once the last stage passed to the landing train. A step that escalated its issue says why, in a few words: {do, issue, outcome, escalated}. A `dispatch` that found an open blocker says which: {do, issue, outcome: "blocked", blocked_by}
    /// - `errors`: each step that failed: {do, issue, error}, with `patrol` as in `did` — and `see`, where a help topic explains what refused it: the command that prints the topic. A claimed issue the rebuild could not read is {do: "read", issue, error}: the rest of the tick is made, with it taken for open
    /// - `held`: the claims the instance holds after the pass, a patrol's run and a proposal still going among them
    /// - `frontier_remaining`: the frontier issues it left undispatched
    /// - `idle`: whether the fleet instance is idle after the pass: it holds nothing, nothing of its is on the landing train, and its frontier is empty. `afkconvoy checkouts` answers it of each checkout's last tick
    /// - `locks`: the fleet locks that stood as it ran: the open issues carrying the lock label — none: []. `run` opens its line with them: "FLEET LOCK #7 stands: only it is worked"
    /// - `held_back_by`: each threshold of the host past which admission started no new worker, as the line says it: "the load 31.20 is past 28.00, 200% of its 14 cores". A slot was free and something waited for it; it stays on the frontier (`afkconvoy run --help`, ADMISSION)
    ///
    /// Errors: a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run); the repo fails the startup probe; the config, GitHub or the remote cannot be read; the host kit cannot be read or lacks a required harness; the worker launch command of a harness the fleet could be asked for resolves to nothing, or — in a bound repo — does not take the flags that start a worker in its role; `--instance` is not an instance id ([a-z0-9][a-z0-9-]{0,39}).
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required: `run` mints an id, a tick has to be given one. Give every tick of one fleet the same id — a new id holds no claims, and sees the old id's as another instance's.
    /// - In a bound repo the tick that reads a verdict ends the claim its stage was performed under (`advance`) and, where the verdict leads to a stage a role performs, starts that stage's worker into the slot it freed, under a claim of its own (`dispatch`, on the same issue): a `pass` to the next stage, a `reject` to an earlier one, a ruling that runs a stage again. That worker goes ahead of everything else the tick takes a slot for. Only a fleet over its `concurrency` has no slot for it: the issue is then on the frontier of the tick after, ahead of the issues no stage has declared on yet.
    /// - A dispatch reads the issue's blockers once more, just before its claim: the rebuild can be minutes older (settling and tending the train run in between), and a blocker added since holds the issue back. Nothing is claimed, no worker starts and no status board is shown; the step says `blocked`, the line `skipped #68 (blocked by #71)`, and the slot it would have taken goes to the next issue on the frontier, in the same tick. The rebuild's own requests are as many as ever.
    /// - An issue GitHub answers 410 (deleted) or 404 (transferred) for is gone, like a closed one, and costs the tick nothing else. A claim of mine on it is let go (`forget`): released, its worktree removed when it holds nothing the remote does not — else kept, its worker's terminal closed — and nothing written on GitHub. A dispatch that finds it gone, before its claim or after, leaves no claim behind, says `gone` and takes no slot. The line says `released #231 (the issue was deleted)`, or `skipped #231 (…)` of a dispatch that found it gone with no claim to release; `afkconvoy doctor` names a claim on one whose fleet is not running, with its fix.
    /// - The last stage is different: the tick after a `pass` that leads to `land` starts a worker in the workflow's landing role, which gates the PR and joins the landing train; a `pass` to `land` with no open PR to join is handed to a human. A consult and a stage that runs again once a fleet lock closed are the tick after's too.
    /// - `run` does not sleep across that hop: after a tick that settled an issue or advanced one without starting what follows, and left a slot free, it starts the next tick at once (`next now` in its line), at most {const:BACK_TO_BACK} ticks back to back. Ticking by hand, the next `tick` is yours to run.
    /// - The step that ends a piece of work ends its worker too: no worker is left at its prompt once the tick has acted on what it declared, and with nothing claimed the only worker alive is the train's. What becomes of the terminal and the worktree:
    ///
    ///   - `end_patrol`: The terminal is closed and the checkout removed: the patrol's next run starts in a new one, cut from the base branch as it then stands. A run that failed keeps its checkout, with the terminal closed — the issue that reports it gives the path — until the patrol is run again.
    ///   - `end_proposal`: The terminal is closed and the checkout removed.
    ///   - advance, to a stage or a consult: The terminal is closed and the worktree kept: the next stage's worker is started in it, in a terminal of its own — a stage's by the same tick. The same where the issue then waits on a human.
    ///   - advance, to `done`: The terminal is closed and the worktree removed, with whatever was uncommitted in it. Its branch goes with it when it holds no commit past the base branch, and is kept when it does. The issue stays open until somebody closes it.
    ///   - an escalation: The terminal is closed and the worktree removed when the remote holds all of it — a clean tree, every commit pushed or on the base branch — or kept, when it holds what only this host has: the comment and the status board say what, the comment naming each commit the remote lacks. The PR and the branch are kept for the human. A park does the same, and a later dispatch recreates a removed worktree at the pushed tip.
    ///
    ///   An issue that lands loses its worktree, and its worker with it, as it is settled.
    ///
    /// - The train's worktree is kept with what its gate runs built until it has outgrown its fresh build, by what the last landing measured (`afkconvoy land --help`). A tick that finds the train idle then — nothing on it, and its worker not at work — removes it and cuts it again (`cut_train`), and its line says `cut the train's worktree again (built 4.2 GiB, its fresh build 1.1 GiB)`, or `no fresh build on record`. A train with a PR on it, or a train worker at work, is never cut again.
    /// - An `escalate` does not always hand the issue to a human. From a stage that names a consult (`escalate: { to, do }`), the tick that reads it ends the stage's claim, and the next one starts a worker in the consult's role — under a claim of its own, in one slot of `concurrency`, in the same worktree. Its ruling (`afkconvoy rule`) runs the stage that asked again, in the tick that reads it, and counts as one more of the issue's rework in total; its own `escalate` goes to a human. A consult that stays silent is nudged once and then escalated, like any worker.
    /// - A `reject` does not always start the earlier stage again. Where it would pass one of the workflow's breakers — `max_rework`, `max_consecutive_rework` — the tick escalates the issue straight to a human instead, whatever consult the stage names; the comment names the breaker and lists every rework's stage and reason in order. `afkconvoy rehearse` trips them at the same verdict.
    /// - A verdict is not carried out when its stage left what it may not have: a stage marked `writes: false` whose branch holds a commit that was not there when its worker started has failed, and the issue escalates whatever it declared. The same holds for a consult, whatever its stage is marked: it writes no code. The guard (`afkconvoy guard --help`) refuses such a commit as it is made; this is the check of what a guard missed.
    /// - A patrol the repo names (`patrols`, `afkconvoy config --help`) is run by the tick that finds it due: its interval has passed since its last recorded run, and no run of it is still going. The tick claims the run on the remote — of several hosts one wins, and the rest say `lost` — starts a worker in the patrol's role, in a checkout of the base branch as it stands, linked to no issue, and records the run. A run takes one slot of `concurrency`. A patrol is not run while its open issues have reached its `max_open`, nor while a fleet lock stands; one that missed runs is run once. `afkconvoy plan` says how each stands.
    /// - A patrol's worker declares no verdict: it declares its run over (`afkconvoy over --help`), with the line its brief ends with, and the next tick ends the run (`end_patrol`) and frees its slot. One that never does is ended once its worker is gone, or has been stopped for the grace period. Either way a run that left a commit in its checkout, or opened a PR, has failed: the tick opens an issue that says so, with the team's escalate label, and touches no other issue. The patrol is not run again until a human closes it. An operator reading orca sees a patrol's checkout only while its run is going, or has failed.
    /// - A human's ruling on an escalated issue is proposed as a precedent, where the team names who proposes (`precedents`, `afkconvoy check --help`): a tick starts one worker in that role, with that skill, in a checkout of the base branch linked to no issue (`propose`). The worker opens one PR on the team's repo that appends one entry to `precedents.md` — the ruling in a sentence, when it applies, and the link to the ruling. A human merging it is what accepts the precedent: the fleet merges nothing on the team's repo. A verdict a human declares for a `by: human` stage is no ruling, and neither is a consult's.
    /// - The issue does not wait for the proposal: the stage that asked is dispatched first, and a proposal takes a slot of `concurrency` only where one is left. A ruling is proposed at most once — a comment on the issue says it was, before the worker is started — and none is started while the open proposals on the team's repo have reached `max_open`, nor while a fleet lock stands. `afkconvoy plan` says how each stands.
    /// - A proposal's worker declares its run over as a patrol's does, and the next tick ends the proposal (`end_proposal`) — or a later one does, once its worker is gone or has been stopped for the grace period: a PR from its branch that changes anything but `precedents.md`, or does anything there but append that one entry, is closed with the reason in a comment; one that is a proposal is left for a human. The checkout is removed.
    /// - A tick that finds the gate slow opens one issue that says so (`slow_gate`). The gate is slow when, on one host, the last {const:SLOW_GATE_LANDINGS} landings on the base branch that have a recorded run of the config's `gate_command` with a time all took more than {const:SLOW_GATE_PERCENT}% of the gate limit, `gate_seconds`. The issue carries the config's `ready_label`, so the fleet works it, and the label `slow-gate`, which is how it is found again. Its body gives the limit, the host and the {const:SLOW_GATE_LANDINGS} runs, each with its commit and its seconds, and says what is not a fix: deleting or skipping tests, and raising `gate_seconds`. `afkconvoy plan` shows the same evidence, as `slow_gate`.
    /// - It opens no other while an issue carrying `slow-gate` is open, on any host, and the opening is claimed on the remote as a patrol's run is: of several hosts that found the gate slow one opens the issue, and the rest say `lost`. After that issue is closed only the landings made since count: the gate is slow again when {const:SLOW_GATE_LANDINGS} of those all took more than {const:SLOW_GATE_PERCENT}%. It takes no slot of `concurrency`.
    /// - With a fleet lock standing a tick works lock issues only (`afkconvoy lock --help`). It dispatches nothing else, runs no patrol, proposes no precedent, takes no stale claim of another issue, hands no other issue to a human, and leaves alone every claim of its own on another issue whose PR is not on the landing train: no judge, no watch, no advance, no nudge. What those workers declared is acted on by the first tick after the last lock issue closed. Their claims take no slot meanwhile, so `held` may pass the config's `concurrency` by as many; the line of `run` shows them apart, as `left alone`.
    /// - A claim whose worker declared a verdict under it takes no slot either: the slot is free the tick it is read, though the claim is held until the verdict is acted on — once its worker has been quiet the grace period, as ever. A verdict on the issue from before the claim was created is not one declared under it. So `held` may pass the config's `concurrency` by as many, and the line of `run` shows them apart, as `declared`. Where acting on the verdict starts a worker under the claim again — every blocker closed, a gone worker continued — that start waits for a free slot as any dispatch does, and is made by a later tick while none is free.
    /// - What a closed lock explains is run again, not carried out. In a bound repo, a stage whose own worker declared `escalate` before the lock closed and was not acted on since — and one whose worker stayed silent past a nudge sent before it closed — is not handed to its consult or to a human: the fleet leaves a stage record (`verdict=lock-closed by=fleet`), the claim is released (`advance`, `done`), and the next tick starts a worker on that stage, whose brief says a lock stood. It is no rework. A `pass`, a `blocked` and an `already-satisfied` stand as they are. What landed is still settled, and the train still tended.
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
    /// Run the gate in this worktree and put a green run on record (a worker's command)
    ///
    /// ## When to run it
    ///
    /// When the work is committed and before the PR joins the landing train, with `--train`; and again after every further commit. A worker's prompt names the exact command.
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree. The operator runs it only to find out why a worker's gate is red.
    ///
    /// ## What it reads and writes
    ///
    /// It runs the config's `gate_command` in this worktree, its log streaming to stderr, for no longer than the gate limit: the config's `gate_seconds` (default {const:GATE_SECONDS}). A run that reaches it is ended, with everything it started, and is red.
    ///
    /// With `--train` it first merges in what the PR will land behind — the landing train's tip, or the base branch when nothing is on the train — a merge: nothing is rebased. On the remote it writes only the recorded gate run: put on record for a green run of a committed tree, with the seconds it took and the host it ran on (`--host`, else this machine's name); taken off record for a red one. It pushes no branch and changes nothing on GitHub; after a red run it reads which fleet locks stand.
    ///
    /// After the first green run in a worktree it notes, in the worktree's own git dir, what git ignores there as the worktree's fresh build: what `run` judges its held disk by.
    ///
    /// ## What it answers
    ///
    /// {"status", "recorded", "detail", …}
    ///
    /// - `status`: green, red or conflict
    /// - `recorded`: true only when the run is on record as a pass of the commit's tree
    /// - `detail`: what to do next, in words
    /// - `exit_code`, `timed_out`, `command`, `head`: the run: the gate command, its exit code, and the commit it ran on. A run ended at the gate limit is red, with exit_code {const:TIMED_OUT_CODE} and timed_out true, and `detail` says so, and what the limit is
    /// - `seconds`: how long the whole gate command ran, in whole seconds, as this command measured it. The fleet does not know the gate's steps: which of them is slow is for the gate to print. A green run's record keeps it; a red run leaves no record, and so no time
    /// - `locks`: on a red run with a fleet lock standing: the locks. The base branch may be what is red: declare nothing of it, stop, and run this again when told to carry on
    /// - `merged`: with `--train`: the commit that was merged in
    /// - `uncommitted`: on a green run of a tree that was not the commit: the paths, {const:STRAYS_LISTED} at most
    ///
    /// not_recorded on a green run whose record the remote refused: why
    ///
    /// - `files`: on conflict: the conflicted files. No gate ran, and the merge is left in progress here
    ///
    /// Errors: the config cannot be read; with `--train`, tracked files have uncommitted changes ("has uncommitted changes to tracked files"), or the remote has no base branch.
    ///
    /// ## What trips you up
    ///
    /// - A red gate is an answer, not an error: the exit is 0. Read `status` and `recorded`.
    /// - Green is not enough. An untracked or modified file — the gate's own artifacts included, unless `.gitignore` covers them — makes the run green with `recorded` false: it proves nothing about a commit.
    /// - A recorded run is of one tree. Any further commit needs another run.
    /// - The gate limit is no flag. It is `gate_seconds` in the config of this worktree (`afkconvoy config --help`), the same for a worker's run and the train's.
    /// - A red run is the worker's own until it shows otherwise: the commit `merged` names, gated alone in a clean checkout of it (this command, without `--train`), red the same way. Then the defect is outside its issue, and its prompt says how it records it: on the open issue that already does, or on one it opens — with the ready label, or through `afkconvoy intake` in a bound repo — and parks its issue on that one.
    /// - A conflict is resolved by committing the merge, then running this again. A resolved merge left uncommitted is refused.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Gate {
        /// First merge in what the PR lands behind: the landing train, or the base branch
        #[arg(long)]
        train: bool,
        /// The host a green run is recorded as made on [default: this machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: At,
    },
    /// Put this worktree's PR on the landing train (a worker's command), or land the train
    ///
    /// ## When to run it
    ///
    /// `land --issue <n>`: once the PR is open and its gate run is green and on record, to join the PR to the landing train. A worker's prompt names the exact command.
    ///
    /// `land --train`: when the train's worker is told a PR joined, and again after each answer that says so, until the answer is `idle`.
    ///
    /// ## Who runs it
    ///
    /// `--issue`: the worker that wrote the PR, in its own worktree — in a bound repo, the worker in the landing role of the issue's workflow, in the issue's worktree.
    ///
    /// `--train`: the train worker, in the train's worktree — never anybody else, anywhere else.
    ///
    /// ## What it reads and writes
    ///
    /// `--issue` reads the open issues, the open PRs and the train. With a fleet lock standing it joins the PR of a lock issue only, and for any other changes nothing. Otherwise it pushes this branch when it is ahead of the remote's, then pushes one merge commit — the PR's head joined onto the train's tip — as the train's new tip. No gate runs, and nothing on GitHub changes. A PR that conflicts with the train gets the train's tip merged into this worktree, left in progress.
    ///
    /// `--train` puts this worktree on the train's tip, merges the base branch in when it moved, pushes that as the train's tip, and gates it — by the run on record for that tree, which it leaves as it is, else by a run made now, for no longer than the gate limit: the config's `gate_seconds` (default {const:GATE_SECONDS}), as for `afkconvoy gate`. That run is put on record with the seconds it took and the host (`--host`, else this machine's name). Green: it pushes the commit to the base branch as a fast-forward, closes the issue of every PR that landed, and deletes those PRs' branches once GitHub shows them merged. Red: it comments the failure on every PR on the train, and lands nothing. Either way it then measures this worktree: what git ignores in it, and the files git tracks; the first run here notes what it built.
    ///
    /// ## What it answers
    ///
    /// `--issue`: {"issue", "pr", "outcome", "detail", …}
    ///
    /// - `joined`: the PR is on the train — stop. With "head", "commit" (the join commit) and "again": true when it was there already
    /// - `merged`: the base branch already holds the PR's head — stop. With "head"
    /// - `conflict`: with "files" and "worktree": resolve every file, commit the merge, run the gate again, then this again
    /// - `locked`: a fleet lock stands, and this issue is not one: nothing joined and nothing was pushed — stop, and run this again when told to carry on. With "locks", the lock issues
    ///
    /// `--train`: {"train", "base", "outcome", "detail", …}
    ///
    /// - `idle`: nothing is on the train
    /// - `conflict`: with "files" and "worktree": resolve, commit the merge, run it again
    /// - `gate_red`: with "commit", "prs" and "gate" {status, exit_code, timed_out, seconds, command, excerpt}: fix the train with one more commit here, run it again. A run ended at the gate limit has exit_code {const:TIMED_OUT_CODE} and timed_out true, and "detail" says so, and what the limit is
    /// - `target_moved`: the base branch moved while the gate ran; nothing landed — run it again
    /// - `landed`: with "commit", "prs", "gate" {status, source, command, at, seconds, host}, "landed" [{issue, pr, commit}], "unmerged" (PRs GitHub still shows open) and "more": true when the train grew meanwhile. "seconds" and "host" are how long the gate command ran and where, as the run that answered says — the one on record ("source": "recorded") or the one made now ("run"); a run recorded with no time has neither
    ///
    /// gate_red and landed also say, in bytes, what this worktree holds once it is gated:
    ///
    /// - `built`: everything git ignores here — what gate runs built, every older artifact a build tool keeps beside the newest included
    /// - `fresh`: what this worktree held after the first gate run since it was cut: its fresh build. Noted once, in this worktree's own git dir, so a worktree cut again notes it anew; never noted when the worktree held ignored files before that run, and absent while none is on record
    /// - `source`: the files git tracks, for reference only: how many times this a build comes to differs from repo to repo, so it decides nothing
    ///
    /// This worktree is kept with what its gate runs built, landing after landing, until it has outgrown its fresh build: built more than {const:OUTGROWN_TIMES} times fresh and more than {const:OUTGROWN_GIB} GiB — more than {const:OUTGROWN_GIB} GiB alone while no fresh is on record. Then the next tick that finds the train idle — nothing on it, and its worker not at work — removes the worktree and cuts it again at the base branch's tip, its notes with it; the run log says so, with built and fresh. The next gate run here builds from nothing, and is slower.
    ///
    /// Errors: no open PR closes the issue; tracked files have uncommitted changes; the remote has no such branch; the train moved under the command too many times ("run this again"); for `--train`, untracked files in the worktree ("is not the commit that would land").
    ///
    /// ## What trips you up
    ///
    /// - `--train` moves the branch checked out here onto the train's tip, with a hard reset. Run in a checkout of your own it takes that checkout's branch with it.
    /// - Joined is not landed. `--issue` ends at `joined`; the train worker gates and lands.
    /// - A `gate_red` may be the base branch's, not the train's: the train worker gates `origin/<base>` alone, in a clean checkout, and red the same way is a defect it records — on the open issue that already does, or on one it opens. In an unbound repo that one carries the ready label; in a bound repo, where only a flow label dispatches, it takes it in with `afkconvoy intake --train`: to the team's first workflow, unless another fits it clearly better. Then it goes on with the train.
    /// - `--issue` runs no gate. It joins whatever is committed: run `gate --train` first.
    /// - `--merged-timeout` and `--host` are read by `--train` only. How long the gate may run is no flag: it is `gate_seconds` in the config (`afkconvoy config --help`).
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Land {
        /// The issue the PR closes
        #[arg(value_name = "ISSUE", conflicts_with_all = ["train", "issue"])]
        number: Option<u64>,
        /// The same, as a flag
        #[arg(long, required_unless_present_any = ["train", "number"], conflicts_with = "train")]
        issue: Option<u64>,
        /// Gate the landing train and land it on the base branch (the train worker's command)
        #[arg(long)]
        train: bool,
        /// Seconds GitHub is given to show a landed PR as merged
        #[arg(long, default_value_t = 60)]
        merged_timeout: u64,
        /// The host a green run made by `--train` is recorded as made on [default: this
        /// machine's name]
        #[arg(long)]
        host: Option<String>,
        #[command(flatten)]
        at: AtFlag,
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
    /// Opening a PR declares nothing: a stage that wrote one still declares `pass` — after `afkconvoy gate --train` says green on what it pushed, as its brief tells it.
    ///
    /// A consult also declares `close` or `park`, with its reason in the text. Close ends the issue; park leaves it open for new evidence, off the frontier until explicitly brought back (see `afkconvoy help humans`). Neither adds rework or resets its counts. `escalate` lets through to a human what the consult was asked to rule on. Its ruling uses `afkconvoy rule --help`. In an unbound repo the stock consult uses `--consult` and `--stage consult` with `close` or `park`, with no role or team commits.
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
    /// Errors, as {"error": …} with a non-zero exit: a word that is none of the five verdicts; no text, or an empty one; `--blocked-by` with another verdict than `blocked`; a worker's on an issue that is not claimed; one of `--role-commit` and `--team-commit` (or AFKCONVOY_ROLE_COMMIT and AFKCONVOY_TEAM_COMMIT) without the other; a `--harness` (or AFKCONVOY_HARNESS) that names no harness, or one with no commit; "is not in a git checkout".
    ///
    /// A human's is refused, with nothing written, when the issue "is claimed" — its stage is its worker's, and a worker's record names both commits; when it is at another stage, or has left its stages; when the verdict "cannot be declared" there; with `--blocked-by`; and when the issue carries no one flow label.
    ///
    /// ## What trips you up
    ///
    /// - One stage declares once under one claim. A second run changes nothing, whatever it says: the first record stands.
    /// - A record is a verdict only for the stage the claim was dispatched to. One that names another stage is written and counts for nothing: the worker is then treated as having declared no verdict.
    /// - It does not check that the verdict is one the stage may declare. A `reject` from a stage whose workflow gives it none is no valid verdict, and the issue escalates.
    /// - A ruling is not a verdict, and is not declared here: see `afkconvoy rule --help`.
    /// - An `escalate` reaches a human straight only from a stage that says `escalate: { to: human }`. From any other, the next tick starts its consult — the role it names, or the stock consult where it names none — which rules — and the stage runs again under the ruling — or declares `escalate` itself. That one always goes to a human: the chain is one role long.
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
    /// - An escalation made before its comment named the workflow cannot say which flow label to put back. Put the flow label on by hand, then rule.
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
    /// The train worker runs it with `--train` for the issue it opened for a defect of the base branch — the base branch gated alone red the same way as the train. Its prompt names the team's workflows: the first by default, another when it fits the defect clearly better.
    ///
    /// ```
    /// afkconvoy intake --train --issue 14 --workflow fix
    /// ```
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree: it starts in its role's directory, so it changes to the worktree first, as the line its brief gives for `afkconvoy verdict` does. The fleet put its role and the issue it is on in its environment (AFKCONVOY_ROLE, AFKCONVOY_ISSUE), and the command reads them from there; `--role` and `--from` say them by hand. A patrol's worker is on no issue: it has its patrol's name there instead (AFKCONVOY_PATROL), which `--patrol` says by hand. The train worker has no role and is on no issue and no patrol: it says `--train`, in the train's worktree, and nothing else of who it is.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the config of the checkout, the team it names, the issue, and — to count — the comments of every open issue that carries a flow label. It writes on the issue, in this order: a blocked-by edge to each issue `--blocked-by` names that GitHub does not record yet; one comment, the intake record, whose marker carries the workflow, the role and the issue the worker was on — or the patrol it ran, or, of the train worker's, `by=train` and nothing else; then the workflow's flow label, `flow:<workflow>`. Nothing but this command writes that format. Then it wakes the fleet, as `afkconvoy wake` does: the issue is on the frontier at the next tick — or, with an open issue to wait for, once that has closed. The edges go first, so the issue is never on the frontier ahead of what blocks it.
    ///
    /// The limit. Before it writes anything it counts the open issues taken in by workers: those that carry a flow label and an intake record for it. With that count at the repo's `intake_limit` (see `afkconvoy config --help`) it refuses and writes nothing. The count is taken from the records each time and stored nowhere, so closing such an issue makes room. An issue a human labelled has no record: it is never counted, and nothing here refuses a human's label.
    ///
    /// ## What it answers
    ///
    /// {"issue", "workflow", "taken", "role", "from", "blocked_by", "open", "limit", "woke"}
    ///
    /// - taken: true: the record and the flow label were written
    /// - `role`, `from`: who took it in: the worker's role, and the issue it was on. A patrol's worker answers `patrol`, the patrol's name, in place of `from`. The train worker answers "train": true in place of both
    /// - `blocked_by`: the issues it was said to wait for
    /// - `open`: the open issues taken in by workers, this one among them
    /// - `limit`: the repo's `intake_limit`
    /// - `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers
    ///
    /// {"issue", "workflow", "taken": false, "standing"}
    ///
    /// - taken: false: the issue follows that workflow already: nothing was written
    /// - `standing`: how it came in: {"by": "worker", "role", "from", "url"} with the comment of its intake record — `patrol` in place of `from` where a patrol's worker took it in, "train": true in place of both where the train worker did — or {"by": "human"}
    ///
    /// Errors, as {"error": …} with a non-zero exit: the intake limit is reached — the error says the count and the limit; the team has no workflow of that name — the error lists the ones it has; the team has no such role, or no such patrol; the repo is unbound; the issue is closed, is a pull request, or already carries the flow label of another workflow; the role, or both the issue the worker is on and its patrol, is neither in the environment nor given, and `--train` is not given; `--train` with a role, an issue or a patrol, given or in the environment; both `--from` and `--patrol` are given; `--blocked-by` names the issue itself, a pull request, or an issue that cannot be read; the team has a fault; "is not in a git checkout".
    ///
    /// ## What trips you up
    ///
    /// - It is the only way a worker's intake is on the record. A flow label a worker puts on by itself would read as a human's and escape the limit: the guard refuses that, and an intake record written by hand (`afkconvoy guard --help`).
    /// - An issue a patrol's worker takes in counts against two limits: the repo's `intake_limit`, which refuses here, and the patrol's own `max_open`, which refuses nothing here — the fleet does not run the patrol again while that many are open.
    /// - The limit bounds the work the fleet gives itself, not a day's work: it is set high, to stop a runaway. `afkconvoy plan` reports the count against it.
    /// - The record counts only while the issue carries the flow label it names. A human who moves the issue to another workflow has taken it in themselves.
    /// - A split is nothing but this and a `blocked` verdict: the parent is parked on its children, and the stage that split it runs again when all of them have closed. A child counts against the limit like any issue a worker takes in, and may be split in turn.
    /// - An issue the train worker takes in counts against the limit too. It is the only way the train worker of a bound repo puts a defect of the base branch before the fleet: there the ready label dispatches nothing.
    /// - A child has to close for its parent to return. One whose workflow ends in `done` stays open until somebody closes it.
    /// - `--blocked-by` is read only as the issue is taken in. On one that follows the workflow already, nothing is written.
    /// - Two workers taking in at the same moment can both pass the count: the limit can be overshot by as many as ran at once.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Intake {
        #[command(flatten)]
        issue: IssueArg,
        /// The workflow it is to follow, by name: what follows `flow:` in its flow label
        #[arg(long)]
        workflow: String,
        /// The role of the worker that takes it in
        #[arg(long, env = ROLE_ENV)]
        role: Option<String>,
        /// The issue that worker is on
        #[arg(long, env = ISSUE_ENV, conflicts_with = "patrol")]
        from: Option<u64>,
        /// The patrol that worker runs, when it is on no issue
        #[arg(long, env = PATROL_ENV)]
        patrol: Option<String>,
        /// Taken in by the train worker, for a defect of the base branch: no role, issue or patrol
        #[arg(long)]
        train: bool,
        /// The issues it is to wait for, as numbers separated by commas
        #[arg(long, value_delimiter = ',')]
        blocked_by: Vec<u64>,
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
    /// A worker, in its own worktree: a patrol that finds the base branch red, or a stage that finds it cannot be the only one to fail. The operator may run it too, in a checkout of the repo; a human may as well put the lock label on the issue on GitHub.
    ///
    /// ## What it reads and writes
    ///
    /// It reads the issue and the config's `lock_label` (default fleet-lock). On an open issue that does not carry the label it writes one comment — the record of the role that raised the lock, read from `--role` or from the worker's environment (AFKCONVOY_ROLE); a worker of an unbound repo has none — and then puts the label on the issue. Then it wakes the fleet, as `afkconvoy wake` does.
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
    /// Errors, as {"error": …} with a non-zero exit: the issue is closed; there is no such issue; the config cannot be read; "is not in a git checkout".
    ///
    /// ## What trips you up
    ///
    /// - A lock stops everything else, on every host: no other issue is dispatched, no stale claim of another issue is taken, no issue is handed to a human, and no PR of another issue joins the landing train (`afkconvoy land` answers `locked`).
    /// - Workers already running are left alone, and nobody asks after them: what they declare — a verdict, a PR — is acted on once no lock stands. Their claims take no slot meanwhile, so a lock issue is started even by a fleet at its bound.
    /// - Once the last lock has closed, a bound repo's stage that declared `escalate` or stayed silent past its nudge before then runs again, on the base branch as it is now: a broken base branch is what such a lock is raised for, and a human could only answer "run the gate again". Every other verdict stands.
    /// - What was on the landing train before the lock is not taken off it. The lock issue's PR joins behind it.
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
    /// Also: "is not in a git checkout"; the config, the team, the remote or GitHub cannot be read; the remote does not take the push.
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
    /// Declare a patrol's run or a proposal's over: the next tick ends it (a worker's command)
    ///
    /// ## When to run it
    ///
    /// Once, as the last command of a worker that is on no issue: a patrol's, when it has nothing more to open, and a proposal's, when its PR is open or there is nothing to propose. The worker's brief ends with the exact line, the run named in it.
    ///
    /// ## Who runs it
    ///
    /// A patrol's worker or a proposal's, in its own worktree: the checkout it was started in. Never a worker on an issue: its stage ends with a verdict (`afkconvoy verdict --help`).
    ///
    /// ## What it reads and writes
    ///
    /// It reads the claim on the run off the remote and pushes one record that names it — a ref of the fleet's, `over/<run>` — then wakes the `run` of this host (`afkconvoy wake --help`). The next tick finds the run declared over and ends it at once, as it would one whose worker is gone (`end_patrol`, `end_proposal`: `afkconvoy tick --help`): a patrol's checkout is still checked for a commit and a PR, a proposal's PR is still held against its ruling. The slot the run held is free from that tick on.
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
    /// - A worker that never runs it loses nothing but time: the fleet ends the run once the worker is gone, or has been stopped for the grace period — five minutes in which the run holds a slot of `concurrency` no issue can be worked in.
    /// - The record names the claim it was read from: left behind, it says nothing of the next run of the same patrol.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Over {
        /// The run, as its brief names it
        #[arg(long)]
        run: String,
        #[command(flatten)]
        at: At,
    },
    /// Say what an inspection of a hot worker or of the host found: its `run` types it at the worker and leaves it on the issue, or keeps it beside admission's reading (an inspection's command)
    ///
    /// ## When to run it
    ///
    /// Once, as the last command of an inspection. Its brief ends with the exact line. An inspection is of one of two kinds, and finds in words of its own.
    ///
    /// Of a hot worker — one whose processes take more than their share of the host — to find out what takes so much and why. It found one of three things:
    ///
    /// - `wrong`: the work is wrong: say what is wrong and where
    /// - `heavy`: the work really is that heavy: say which command is
    /// - `not-its`: what takes so much is not the worker's: a daemon outside its tree, another fleet's gate
    ///
    /// Of the host, once admission has held back new workers for one reason — the load, memory or disk — past its span (`afkconvoy run --help`, ADMISSION), to find out what holds what admission read and why. It found one of three things:
    ///
    /// - `reclaimable`: something on the disk can be rebuilt — anywhere under the home folder, not only the fleet's worktrees: free it with --reclaim
    /// - `fleet`: the fleet's own shape outgrows the host — its concurrency times one worktree's fresh build past the free disk, say: say the arithmetic
    /// - `not-its`: what holds it is outside this fleet and cannot be rebuilt — another fleet's work, a daemon, a user's files: name it
    ///
    /// A host inspection frees and names with paths under the home folder:
    ///
    /// - --reclaim <path>: with reclaimable only, and as often as wanted: frees the path. A git checkout no process uses — none has its working directory there or a file open there (`lsof`), the landing train's while a gate runs among them — and no worker holds: what git ignores in it (`git clean -fdX`: target/, node_modules, .build), never a file git tracks or a change no commit holds; and where it is the worktree of a closed issue holding no change, the worktree whole. A tool's cache, whole or a part: ~/Library/Caches, ~/Library/Developer/Xcode/DerivedData, ~/.cargo/registry/cache, ~/.cargo/git/checkouts, ~/.npm/_cacache, uv's and pip's caches, ~/.gradle/caches. Anything else is refused.
    /// - --name <path>: as often as wanted: names the path, with what it holds, for a human to free. What cannot be rebuilt or is expensive to fetch again — model weights (Hugging Face's and ModelScope's, ~/.ollama, ~/.lmstudio), Docker's images, ~/Downloads, media, a user's files — is named, and never deleted: --reclaim refuses it.
    ///
    /// Every path is checked before any is freed: one that may not be is refused, saying why, and nothing of the line is freed.
    ///
    /// ## Who runs it
    ///
    /// A worker started as an inspection, in its own worktree: the checkout it was started in (`afkconvoy-inspection`). Run anywhere else it is refused, and so is a word of the other kind.
    ///
    /// ## What it reads and writes
    ///
    /// It keeps what was found in that checkout's own git dir — a second run says it again, and the last one stands — and wakes the `run` of this host (`afkconvoy wake --help`). That `run` ends the inspection at its next look, within 30 seconds, and removes its checkout, its terminal with it.
    ///
    /// Of a hot worker, it types what was found at the worker's terminal — not `not-its` — telling it to fix the work, or to run the heavy command through `afkconvoy exclusive` from now on, and leaves a comment on the worker's issue saying what was found and why, which is no stage record. The issue stays at its stage, and the worker keeps working.
    ///
    /// Of the host, it keeps what was found in the cache every fleet of the host shares (host-inspection/; `afkconvoy help cache`), shown by the panel and `doctor` beside admission's reading, with what was freed and named. For `fleet` it opens one issue in the repo saying so — with the escalate label and the label fleet-outgrows-host — and none while one carrying that label is open.
    ///
    /// ## What it answers
    ///
    /// {"inspected": true, "found", "kind", "reclaimed", "freed", "named", "woke"} — what was found is kept; `kind` is worker or host; `reclaimed` and `named` are each [{path, bytes}] it freed and named, `freed` the bytes freed in all; `woke` is whether a `run` on this host was told.
    ///
    /// Errors: the text is empty; the checkout it is about is no inspection's; the word is not one its kind finds; --reclaim or --name of a worker's inspection, or --reclaim of another word than reclaimable; a path that may not be freed, or does not exist.
    ///
    /// ## What trips you up
    ///
    /// - An inspection is given {const:INSPECTION_MINUTES} minutes. One that has said nothing by then is ended: nothing is typed at the worker, and the comment, or the host's record, says it found nothing in time.
    /// - It types nothing itself, and opens no issue: the `run` does. An inspection that does as well does it twice.
    /// - An inspection is held as a consult: it commits, pushes and changes the host never (`afkconvoy guard --help`). What a host inspection frees, `inspected` frees for it.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Inspected {
        /// What it found: of a hot worker wrong, heavy or not-its; of the host reclaimable,
        /// fleet or not-its
        #[arg(long, value_parser = found)]
        found: Found,
        #[command(flatten)]
        text: TextArgs,
        /// Of the host, with reclaimable: a path under the home folder to free — what git
        /// ignores in a checkout no process uses, a closed issue's worktree, a tool's cache
        #[arg(long, value_name = "PATH")]
        reclaim: Vec<PathBuf>,
        /// Of the host: a path to name for a human, which is never deleted
        #[arg(long, value_name = "PATH")]
        name: Vec<PathBuf>,
        #[command(flatten)]
        at: At,
    },
    /// Run a heavy command alone on this host, once no other `exclusive` command runs (a worker's command)
    ///
    /// ## When to run it
    ///
    /// For work you know to be heavy: a build, a test suite or a benchmark that takes most of the host's cores, memory or disk for minutes. You know it from an inspection that typed at you that the work really is that heavy, or from your role's rules that name the command. Put `afkconvoy exclusive --` in front of that command, every time you run it from then on:
    ///
    /// ```
    /// afkconvoy exclusive -- cargo test --workspace
    /// ```
    ///
    /// Not for everyday commands — `git`, a single test, a quick build: the queue makes each one wait for every heavy one before it, of every fleet on this host.
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree. Anyone else on the host may too: the queue is the host's, not a fleet's.
    ///
    /// ## What it reads and writes
    ///
    /// It runs the command once no other `exclusive` command runs on this host — whatever its repo, whatever its fleet — and waits in a queue until then: the commands run one after the other, in the order they asked. The queue is the host runner's (`afkconvoy run --help`), held in its process: `exclusive` asks it on its socket, naming its checkout, and stays on the line while it waits and while its command runs. A process that leaves it — its command ended, or it was killed while waiting or running — frees its place for the next. The command runs in the directory `exclusive` is run in, or with `-C <dir>` in that one, as named: a checkout of the repo or a directory in one, and not that checkout's top.
    ///
    /// It writes a line to the tick log of this host for this repo as it starts to wait, when another command is ahead of it; as its command runs — in its turn, or at once in no queue, with why; as it waits again, in the next host runner's queue; and as it ends, with its exit status. `doctor` and the panel show what runs alone and what waits.
    ///
    /// While it waits or runs, the fleet counts the worker busy, not silent: it is not nudged for that. The wait and the run together count toward the gate limit (`gate_seconds`), as any background work of a worker's does: one that has been in the queue longer than that is silent, and its issue is escalated.
    ///
    /// ## What it answers
    ///
    /// Unlike every other command, no JSON of its own: what the command prints is printed, and it exits with the command's exit status — 128 and the signal for a command a signal ended. While it waits it says so on stderr, in one line beginning `[afkconvoy] exclusive:`.
    ///
    /// With no host runner on this host it runs the command at once, in no queue, and says so on stderr: `[afkconvoy] exclusive: runs `<command>` at once, in no queue: no host runner runs on this host`. So it does too where the host runner cannot hold it — one of an older afkconvoy, which cannot read the question — saying why. A host runner that is starting or stopping is waited for, as one replaced is.
    ///
    /// When the host runner is replaced — an upgrade moves it (`afkconvoy run --help`) — or ends, a command that waits asks the next one, within a minute, keeping when it first asked: it waits there before every command that asked after it, saying so on stderr. Where no host runner places it within the minute, it runs at once, saying so. A command that runs runs on to its end: the next host runner does not know of it, and may start the next command beside it.
    ///
    /// It exits 127 when the command cannot be started, and 125, with one JSON object {"error"} on stdout, when the host runner cannot be asked — the cache directory is not known — or what it said cannot be read, or `-C` names no directory of a checkout: the command did not run, and nothing waited.
    ///
    /// ## What trips you up
    ///
    /// - It may wait long: as long as every heavy command ahead of it runs. Run it as you would the gate — in the background, or with a time limit of the gate limit — not under a short tool time limit that kills it while it waits.
    /// - Killing it kills its place in the queue, not only its wait: run it again to wait from the end.
    /// - Only commands run through it wait for one another. A heavy command run without it runs beside the one that runs alone.
    /// - The gate, run by `afkconvoy gate`, does not go through the queue.
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
    /// Never by hand. It is the guard: the check made before each thing a worker of a bound repo does, refusing what no worker may do and what its stage may not. Read this to learn what a worker is refused, or why one was.
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
    /// It writes nothing. It reads two things:
    ///
    /// Standard input: one tool call, as Claude Code describes it to a `PreToolUse` hook — a JSON object with `tool_name` and `tool_input`. Only a `Bash` call is judged, by the words of each simple command of its `tool_input.command`. pi's is described as pi describes it to an extension — `toolName` and `input` — and only a `bash` call is judged, the same way. Codex describes its own as Claude Code does, and a `Bash` call is judged; its `apply_patch` hands the text of a patch as `tool_input.command`, which is no shell line and is not judged as one. DSH describes a call with `name` and `arguments`: `bash` and `bash_persistent` are judged by `arguments.command`. What is refused is decided once, for all.
    ///
    /// The environment: the facts of the worker's stage, which the fleet put there when it started the worker — AFKCONVOY_STAGE, AFKCONVOY_WRITES, AFKCONVOY_CONSULT, AFKCONVOY_BASE_BRANCH, AFKCONVOY_ESCALATE_LABEL, AFKCONVOY_LOCK_LABEL — and AFKCONVOY_HARNESS, the harness that asks: how the call is described is its own.
    ///
    /// Refused for every worker:
    ///
    /// - adding or removing a flow label, a `stage:` label, the escalate label or the lock label (`gh issue edit --add-label`, `--remove-label`, `gh issue create --label`, and any write of an issue's labels through `gh api`)
    /// - writing a stage record by hand: only `afkconvoy verdict` writes one
    /// - writing an intake record by hand: only `afkconvoy intake` writes one, and it is what puts a flow label on for a worker
    /// - merging a PR (`gh pr merge`, or the merge endpoint through `gh api`)
    /// - pushing the base branch (`git push` with a refspec that lands on it, `--all`, `--mirror`)
    ///
    /// Refused besides in a stage its workflow marks `writes: false`, for every consult, and for a patrol's worker (AFKCONVOY_PATROL, in place of a stage's facts):
    ///
    /// - committing and pushing (`git commit`, `merge`, `cherry-pick`, `revert`, `am`, `push`)
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
    /// - What it misses is checked after the stage: a commit left on the branch by a stage marked `writes: false`, or by a consult, is a failure, and the issue escalates; a commit or a PR a patrol left is a failed run, reported to a human — see `afkconvoy tick --help`.
    /// - An unbound repo's workers have no guard: their worker launch command is opaque.
    #[command(next_help_heading = HOOK, verbatim_doc_comment)]
    Guard,
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
    /// The terminal is opened on the worktree as ever, and the line it runs changes to the role's copy, puts the stage's facts in the environment, and starts the worker launch command with what puts the worker in its role: [`afkconvoy help worker-launch`](help:worker-launch) says what, for each harness. The worker's harness and model are its role's range's answer to the issue's casting (`afkconvoy help roster`), or this host's switch's.
    ///
    /// Where the stage declared `escalate` and names a consult, the worker is started as that consult instead: in the consult's role, on a brief that names the skill, the stage that asked and its reason, and the two lines it ends with — `afkconvoy rule`, or `afkconvoy verdict escalate` to let it through to a human. AFKCONVOY_CONSULT is `true` in its environment, the guard refuses its commits, and it runs its own role's range, not the stage's.
    ///
    /// Once the issue's last stage has passed to `land`, the worker is started in the workflow's landing role instead, on the join-only brief with that role's landing skill named in it: it gates the issue's open PR and joins the landing train. Its stage is `land`, and it declares no verdict.
    ///
    /// ## What it answers
    ///
    /// {"issue", "started": true, "claim", "tier", "action", "prompt", "reason", "worktree", "branch", "terminal"}
    ///
    /// - `claim`: won (claimed now) or held (the instance's already)
    /// - `tier`, `action`: 1 reuse_worktree — the worktree is still on this host; 2 recreate_at_tip — a new worktree at the issue's pushed branch; 3 dispatch_fresh — a new worktree at the base branch
    /// - `prompt`: the brief the worker got: fresh, continue, join (its PR is open: only the join is left), or take_up (its PR is open and no claim held it: an escalation or a park kept it — merge the base branch, gate, join) — in a bound repo, stage, consult, or join for a landing worker
    /// - `stage`: in a bound repo: {flow, name, role, skill, writes, consult, harness, model, role_commit, role_dir, team_commit, team_dir} — the stage the worker performs, and the copies and commits fixed for it; name is `land` for a landing worker. For a consult (`consult: true`), name is the stage that asked, and role and skill are the consult's
    /// - `reason`: why that tier, in words
    /// - `discarded`: with `--start fresh`: {closed_prs, deleted_branches, removed_worktree}
    ///
    /// {"issue", "started": false, "claim": "lost", "owner"} — another instance holds the claim. Nothing was started.
    ///
    /// {"issue", "started": false, "skipped": "blocked", "blocked_by"} — the issue has an open blocker: blocked_by lists them, lowest number first. Nothing was claimed and nothing was started.
    ///
    /// Errors: the issue is closed; in a bound repo, the issue carries no flow label, more than one, or one that names no workflow of the team, it is at no stage a worker performs (the error says where it is), it passed to `land` and no open PR closes it, the team does not load (the error carries its `faults`) — none of which claims the issue; the instance's claim on it is one a stage already declared under; the worker launch command resolves to nothing; orca fails, or the worker is not ready in time. After a failed step the claim is still held: the instance's next tick finds a claim with no worker and continues it.
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
    /// Errors: the config, GitHub or the remote cannot be read; the remote has no base branch.
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
    /// - It does not read the verdict. Any claim of the instance whose committed tree equals the base branch's current tree on the remote and has nothing uncommitted is closed, whatever commits the branch holds — one whose worker has only just started as well. A refusal names the differing files.
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
    /// To send a worker its nudge by hand. A tick does this on its own, once, for a worker that stopped with no PR and no verdict and stayed quiet for the grace period. Do it yourself when a tick is not running and such a worker sits idle.
    ///
    /// ## What counts as silent
    ///
    /// A worker is silent when it stopped, declared nothing, showed no sign of life — its terminal's output, a commit, a file touched, being started or nudged — for the grace period (300s), and waits on no background work of its own. Silent, it is nudged once; silent a grace period past that nudge, it is escalated (a joining worker is restarted first, a train worker's train abandoned).
    ///
    /// A worker whose turn ended while a shell or a monitor it started in the background still runs — its gate, say — is waiting, not silent: its harness shows that work in the footer under its prompt (Claude Code's `1 shell, 1 monitor`), which the tick reads off its screen, and the tick's line says `waiting on its own background work #7 (since …)`. It is neither nudged nor escalated while it waits. Once that work ends, its silence is counted as ever: a nudge, then escalated past it. Waiting longer than the gate limit (`gate_seconds`) is a silence at once, nudged or not, and the escalation says it waited on its own background work past the limit.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker runs on.
    ///
    /// ## What it reads and writes
    ///
    /// It asks orca for the worker's state and reads the issue's comments for a verdict. It types one line at the worker's terminal and records the nudge in the worktree. Nothing on GitHub or the remote changes.
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
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
    Nudge {
        #[command(flatten)]
        issue: IssueArg,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Replace the silent worker of one of my claims whose PR is joining the train, once
    ///
    /// ## When to run it
    ///
    /// To replace a silent worker by hand. A tick does this on its own, once, for a claim whose PR is open and not on the landing train, and whose worker stopped, was nudged, and stayed silent. Do it yourself when a plan keeps showing such a claim under `mine` as `joining`.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the worker runs on.
    ///
    /// ## What it reads and writes
    ///
    /// It records the restart in the fleet's comment on the PR, then starts a new worker in the same worktree, briefed only to join the landing train — in a bound repo, in the landing role of the issue's workflow. The PR, the branch and the worktree are untouched.
    ///
    /// ## What it answers
    ///
    /// {"issue", "pr", "action": "restarted", "restarted", "terminal", "prompt"} — restarted is the time, in epoch seconds; prompt is the brief the new worker got.
    ///
    /// Errors, each with nothing changed: "has no stopped worker" — the instance holds no claim on the issue, its worker is at work or gone, or the issue has no open PR; "is not restarted" — the PR's join already had its restart, the worker was not nudged yet, or it showed a sign of life within the grace period; the claim is another instance's.
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
    /// Move this host's workers of the repo onto another harness: from now on, and those of my claims now
    ///
    /// ## When to run it
    ///
    /// To move work onto another harness: a harness's login expired, its provider turned access off, or another agent should take the work over. One line does it all:
    ///
    /// ```
    /// afkconvoy switch --instance <id> --harness codex --all
    /// ```
    ///
    /// It works on a worker that is busy, idle at its prompt, blocked from receiving input, or already gone. It needs no nudge first, no grace period and no PR.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the workers run on.
    ///
    /// ## What it reads and writes
    ///
    /// The harness is this host's choice for the repo, kept in the repo's git dir on this host and never pushed: whoever else works the repo runs the harness they have, under their own accounts. From then on every worker this host starts for the repo — new issues, later stages, consults, patrols, the train worker, inspections — is of that harness, whatever the config's `harness` key, a stage, a patrol or an issue's casting names.
    ///
    /// A choice the host kit stops holding — the harness taken out with `afkconvoy kit`, or paused — is not followed while it does: each worker runs its range's first, the host kit's first in an unbound repo, and `plan` says `unmatched`, naming the switch. It is followed again once the host kit holds it.
    ///
    /// `--clear`, in place of `--harness`, forgets the choice: every worker started afterwards runs what its range and casting say again — in an unbound repo, the issue's `harness:` and `model:` labels within the host kit. The ISSUE, `--patrol` and `--all` it names are moved onto that, each worker onto its own, as a switch moves one. `--harness config` is gone: it is refused, naming `--clear`.
    ///
    /// With no ISSUE, `--patrol` or `--all`, only the choice is made or cleared: running workers are left on the harness they run.
    ///
    /// Then it moves the workers named:
    ///
    /// - `ISSUE`: the worker of the instance's claim on that issue
    /// - `--patrol`: the worker of the instance's run of that patrol, in the run's checkout
    /// - `--all`: every claim of the instance on this host — on an issue, on a patrol's run — one after another; a failure on one is reported and the rest go on
    ///
    /// For each, it ends the worker — its agent and its terminal — and starts one of the harness (cleared: the one its range and casting say) in the same worktree, on the same branch, at the same stage: a consult as a consult, a landing worker as a landing worker. The claim, the branch, the worktree and the PR are untouched. The new worker's brief says it continues earlier work, and names what is there: the commits on the branch ahead of the base branch, the open PR, and the last lines the old worker's terminal showed.
    ///
    /// The switch is recorded on the PR, in the fleet's comment that records its join: from which harness to which, by which instance, and when. It is no restart, no rework and no nudge: no breaker count moves, and a joining worker keeps its one restart.
    ///
    /// ## What it answers
    ///
    /// For ISSUE: {"issue", "from", "to", "worktree", "branch", "terminal", "pr", "stage", "chosen"} — from and to are the old and new harness; chosen is {"was", "now"}, this host's choice before and after (null: none, cleared).
    ///
    /// For --patrol: {"patrol", "from", "to", "worktree", "terminal", "chosen"}.
    ///
    /// For --all, and with nothing named: {"harness" (null when cleared), "chosen", "switched": [each as above], "failed": [{"issue" or "patrol", "error"}]}.
    ///
    /// Errors, each with nothing chosen and no worker ended: the harness chosen cannot be started here — missing, logged out, or its command not taking the flags its role needs ([`afkconvoy help worker-launch`](help:worker-launch) names them) — named with what is wrong; the instance holds no claim on the issue, or no run of the patrol; the claim is another instance's.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required, and must be the instance that holds the claims.
    /// - The choice holds on this host until a switch chooses again or `--clear` forgets it: a change of the config's `harness` key or of an issue's casting labels moves nothing here while it stands.
    /// - A claim whose stage has a verdict declared and not yet acted on is refused: the next tick acts on that verdict, and the worker it starts is of the chosen harness.
    #[command(next_help_heading = OPERATOR, verbatim_doc_comment)]
    Switch {
        /// The harness to move onto: claude-code, pi, codex or dsh
        #[arg(long, value_parser = chosen_harness, required_unless_present = "clear")]
        harness: Option<Harness>,
        /// Forget this host's choice: each worker runs what its range and casting say again
        #[arg(long, conflicts_with = "harness")]
        clear: bool,
        /// The issue whose worker to move
        #[arg(value_name = "ISSUE", conflicts_with_all = ["all", "patrol"])]
        issue: Option<u64>,
        /// The patrol whose running worker to move
        #[arg(long, conflicts_with = "all")]
        patrol: Option<String>,
        /// Move the worker of every claim of the instance on this host, one after another
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        agent: AgentArgs,
        #[command(flatten)]
        at: AtFlag,
    },
    /// Give up the landing train, its worker silent past a nudge, with nothing landed
    ///
    /// ## When to run it
    ///
    /// To give the landing train up by hand. A tick does this on its own when the train's worker stopped with the train not landed, was nudged, and stayed silent for a grace period more. Do it yourself when a tick is not running and the train stands still.
    ///
    /// ## Who runs it
    ///
    /// The operator, in a checkout of the repo, on the host the train worker runs on.
    ///
    /// ## What it reads and writes
    ///
    /// On the remote it deletes the landing train — only while its tip is still the one its worker went silent on. It clears the train's worktree and keeps it. For each PR of the instance that was on the train it records the abandonment in the fleet's comment on the PR and tells the PR's worker — in a bound repo, the one in its workflow's landing role — to join the next train; a PR taken off an abandoned train before is escalated instead. Nothing has landed, and what an earlier train put on the base branch is not touched.
    ///
    /// ## What it answers
    ///
    /// {"action": "abandoned", "train", "issues", "escalated", "worktree"}
    ///
    /// - `train`: the tip of the train given up
    /// - `issues`: the issues whose PRs are to join the next train
    /// - `escalated`: the issues escalated instead
    /// - `worktree`: the train's worktree, kept
    ///
    /// Errors, each with nothing abandoned: "nothing is on the landing train"; "has no worktree on this host"; "is not abandoned" — its worker is at work, gone, not told of what the train holds now, not nudged yet, or showed a sign of life within the grace period; "moved while it was being abandoned" — a PR joined meanwhile, and the train was kept.
    ///
    /// ## What trips you up
    ///
    /// - `--instance` is required. Only that instance's PRs are told to join again.
    /// - It is refused wherever a tick would not abandon: it cannot be used to clear a train whose worker is still at it.
    /// - It takes no `--issue`: a repo has one landing train.
    #[command(next_help_heading = BY_HAND, verbatim_doc_comment)]
    Abandon {
        #[command(flatten)]
        who: Who,
        #[command(flatten)]
        at: At,
    },
    /// Take another fleet instance's claims without waiting for the lease
    ///
    /// ## When to run it
    ///
    /// When a fleet instance on another host is dead and its claims should not wait out the lease. `--list` first, to find its id and see whether its heartbeat is fresh; then `--from <id>`. For this host's own claims it is not needed: `run` takes them at its start. A claim is on an issue, on a patrol's run, or on the proposal of a precedent: both ways take all three.
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
    /// - `instances`: one per instance the remote remembers: {instance, host, claims, runs, claim_count, heartbeat_ts, heartbeat_age, fresh, is_me}. claims: the issues it holds a claim on. runs: the runs it holds one on, by key — `patrol-<name>` a patrol's, `precedent-<issue>-c<comment>` a proposal's. claim_count: both. fresh: its heartbeat is within the lease
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
    /// - `--from` an instance whose heartbeat is still fresh takes nothing until `--yes`: the answer is `confirm`, with exit 0. Add `--yes` only when you know that fleet is dead — a live one goes on working the issues it lost.
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
    /// End the sleep of this host's `run` for this repo: its next tick starts now (a worker's command)
    ///
    /// ## When to run it
    ///
    /// Once, as a worker's last command: after its PR is on the landing train, or its verdict is posted on the issue. A worker's prompt names the exact command. Every command that changes fleet state wakes the `run` as it ends, so after `land` or `verdict` this one is redundant, and harmless.
    ///
    /// ## Who runs it
    ///
    /// A worker, in its own worktree — any worktree or checkout of the repo on the fleet's host reaches the same fleet instance.
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
    /// It exits 0 with each of them. Errors: only "is not in a git checkout". Failing to wake anybody is not an error, and costs at most the {const:WAKE_ANSWER_SECONDS} seconds it waits.
    ///
    /// ## What trips you up
    ///
    /// - It reports nothing. A worker's outcome is its PR on the train or its verdict comment: without one of them on GitHub, a wake only makes the fleet look sooner at a worker that stopped with nothing.
    /// - `woke: false` needs no second `wake`: the fleet finds the outcome at its next tick anyway.
    /// - It reaches a fleet instance of this host's runner only. A fleet driven by `tick` hears nothing.
    #[command(next_help_heading = WORKER, verbatim_doc_comment)]
    Wake {
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
    /// Errors: the remote cannot be read; "is not in a git checkout".
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
    /// Errors: the claim is another instance's ("is not this fleet's claim"); it "moved since it was read" — somebody took it meanwhile, and it was left alone; the delete failed.
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
#[derive(clap::Args, Default)]
struct ProxyArgs {
    /// Hosts reached directly, comma-separated, for the default harness or
    /// HARNESS; repeat for each harness. DSH defaults to api.deepseek.com.
    /// An empty list disables the stock addition. Extends no_proxy and NO_PROXY.
    #[arg(long, value_name = "[HARNESS=]HOSTS")]
    worker_no_proxy: Vec<String>,
}

/// How a command that starts a worker starts it.
#[derive(clap::Args)]
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

fn own_program() -> String {
    let exe = std::env::current_exe().ok().and_then(|path| path.to_str().map(str::to_string));
    exe.unwrap_or_else(|| "afkconvoy".to_string())
}

/// The commands that act: each logs a line and wakes the `run` as it ends
/// (`Command::acting`), and its `--help` says so (`acting::HELP`).
const ACTS: [&str; 25] = [
    "dispatch",
    "cast",
    "close",
    "escalate",
    "park",
    "nudge",
    "restart",
    "switch",
    "abandon",
    "takeover",
    "claim",
    "release",
    "heartbeat",
    "rule",
    "send",
    "land",
    "verdict",
    "intake",
    "lock",
    "patrol-now",
    "over",
    "inspected",
    "gate",
    "wake",
    "tick",
];

/// The commands a worker runs in its own worktree, as its brief, a stage's
/// consult's, the caster's and an inspection's give them: with those that
/// only read (`ONLY_READ`), the only commands a worker's worktree is taken
/// for (`checkout::resolve`).
const IN_ITS_WORKTREE: [&str; 15] = [
    "cast",
    "doctor",
    "plan",
    "show",
    "gate",
    "land",
    "verdict",
    "rule",
    "send",
    "intake",
    "lock",
    "over",
    "inspected",
    "exclusive",
    "wake",
];

/// The commands about one repo that only read — each but with the flag
/// beside it, which makes it act: they change nothing on GitHub, on the
/// remote, in the checkout or of a `run`. A worker's worktree is taken for
/// each, as for a worker's own commands: a consult's brief lets it run any
/// command that only reads, and what one reads from a worker's branch acts
/// for nobody.
const ONLY_READ: [(&str, Option<&str>); 7] = [
    ("config", None),
    ("check", None),
    ("rehearse", None),
    ("recovery", None),
    ("scan", None),
    ("panel", Some("open")),
    ("takeover", Some("from")),
];

/// Whether the command `name`, as `given`, only reads (`ONLY_READ`): it is
/// one of them, and is not given the flag that makes it act.
fn only_reads(name: &str, given: &clap::ArgMatches) -> bool {
    let acts =
        |flag: &str| given.value_source(flag) == Some(clap::parser::ValueSource::CommandLine);
    ONLY_READ.iter().any(|(reads, with)| *reads == name && !with.is_some_and(acts))
}

/// The commands that start the fleet's work: each checks this host before
/// anything else (`probe::startup`), and is refused on a host with no git
/// naming every command it lacks (`checkout::started`).
const STARTS: [&str; 3] = ["run", "tick", "plan"];

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
    for name in ACTS {
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
    let reads = ONLY_READ.iter().find(|(reads, _)| *reads == name).map(|(_, with)| with);
    let worker = match (IN_ITS_WORKTREE.contains(&name), reads) {
        (true, _) => "A worker runs it in its own worktree.".to_string(),
        (false, Some(None)) => "It only reads: a worker's worktree is one too.".to_string(),
        (false, Some(Some(flag))) => format!(
            "Without `--{flag}` it only reads, and a worker's worktree is one too. With `--{flag}`, \
             it is not: only a worker's own commands, and those that only read, are run there."
        ),
        (false, None) => refused.to_string(),
    };
    format!(
        "## Which checkout it is about\n\n{named}: a checkout of the repo, or any directory in \
         one, which names that checkout.{own} Naming none, it is about the checkout it is run in. \
         {worker}"
    )
}

/// The manual: `--help` answers its index, `help all` it whole, and `help
/// <topic>` one topic of it. Its text is the guide (`Cli`'s doc comment), each
/// command's and each topic's, in the order given here: the way in first, the
/// reference last.
pub fn manual() -> agent_help::Help {
    use topic::*;
    let mut help = agent_help::Help::new(cli())
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
        .topic::<WorkerProxy>()
        .topic::<Humans>()
        .topic::<Troubleshooting>()
        .topic::<Glossary>()
        .table::<doctor::Severity>("severity")
        .table::<claim_status::ClaimStatus>("claim_status")
        .constant("CONFIG_FILE", config::FILE)
        .constant("TEAM_FILE", afkconvoy_core::team::TEAM_FILE)
        .constant("ESCALATE_LABEL", afkconvoy_core::config::ESCALATE_LABEL)
        .constant("GATE_SECONDS", afkconvoy_core::config::GATE_SECONDS)
        .constant("INTAKE_LIMIT", afkconvoy_core::config::INTAKE_LIMIT)
        .constant("MAX_REWORK", afkconvoy_core::team::MAX_REWORK)
        .constant("MAX_CONSECUTIVE_REWORK", afkconvoy_core::team::MAX_CONSECUTIVE_REWORK)
        .constant("PATROL_NAME_MOST", afkconvoy_core::team::PATROL_NAME_MOST)
        .constant("SLOW_GATE_LANDINGS", afkconvoy_core::slow_gate::LANDINGS)
        .constant("SLOW_GATE_PERCENT", afkconvoy_core::slow_gate::PERCENT)
        .constant("HARNESS_PROBE_MINUTES", harness_down::PROBE_SECONDS / 60)
        .constant("LOG_KEPT_HOURS", log::KEPT_FOR / 3600)
        .constant("LOG_KEPT_LEAST", log::KEPT_LEAST)
        .constant("LOG_KEPT_MOST", log::KEPT_MOST)
        .constant("WAKE_ANSWER_SECONDS", wake::ANSWER_WITHIN.as_secs())
        .constant("BACK_TO_BACK", afkconvoy_core::tick::BACK_TO_BACK)
        .constant("INSPECTION_MINUTES", afkconvoy_core::inspection::TIME_LIMIT / 60)
        .constant("HOST_IDLE_MINUTES", afkconvoy_core::host_sleep::IDLE_WAIT / 60)
        .constant("SLEEP_HOURS", afkconvoy_core::host_sleep::SLEEP_HOURS.to_string())
        .constant("BACKOFF_FIRST_MINUTES", afkconvoy_core::host_runner::BACKOFF_FIRST / 60)
        .constant("BACKOFF_MOST_MINUTES", afkconvoy_core::host_runner::BACKOFF_MOST / 60)
        .constant("OUTGROWN_TIMES", afkconvoy_core::train::OUTGROWN_TIMES)
        .constant("OUTGROWN_GIB", afkconvoy_core::train::OUTGROWN_FLOOR >> 30)
        .constant("TIMED_OUT_CODE", gate::TIMED_OUT_CODE)
        .constant("STRAYS_LISTED", gate::STRAYS_LISTED)
        .constant("DOCTOR_FAILED_SHOWN", doctor::FAILED_SHOWN)
        .constant("DOCTOR_EVENTS_SHOWN", doctor::EVENTS_SHOWN);
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

fn found(text: &str) -> Result<Found, String> {
    Found::read(text).ok_or_else(|| {
        let words: Vec<&str> = Found::ALL.into_iter().map(Found::word).collect();
        format!("what an inspection found is one of: {}", words.join(", "))
    })
}

/// A harness `switch` moves onto, by its key. `config`, which once forgot
/// this host's choice, is refused for `--clear`, which does that now.
fn chosen_harness(text: &str) -> Result<Harness, String> {
    match text {
        "config" => Err("`--harness config` is gone: `--clear` forgets this host's choice".into()),
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

pub(crate) fn this_host() -> String {
    gethostname::gethostname().to_string_lossy().into()
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
        agent,
        panel,
        no_host_sleep,
        host_sleep,
        host_sleep_hours,
        host_sleep_after,
        hot_cores,
        hot_memory,
        hot_written,
        admit_load,
        admit_memory,
        admit_disk,
        admit_inspect_after,
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
        // Neither overrides the other in clap, which would refuse the line's
        // `--host-sleep` beside the variable's `--no-host-sleep`.
        no_host_sleep: match (no_host_sleep, host_sleep) {
            (_, true) => Some(false),
            (true, false) => Some(true),
            (false, false) => None,
        },
        host_sleep_hours,
        hot_cores,
        hot_memory,
        hot_written,
        admit_load,
        admit_memory,
        admit_disk,
        admit_inspect_after,
        panel,
    };
    let panel = match panel {
        Some(To::Value(port)) => Some(port),
        Some(To::Default) | None => None,
    };
    let ready_timeout = agent.ready_timeout;
    let panic_at = panic_at_tick;
    let knobs =
        instance::Knobs { ticks, second_ms, host_sleep_after, ready_timeout, now, panic_at };
    Some((foreground, instance::Start { named, concurrency, panel, knobs }, settings))
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
        Err(e) => return fail(2, e.render().to_string().trim()),
    };
    if matches!(cli.command, Command::Guard) {
        return guard::guard();
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
    let checkout = match about {
        Some((name, given)) => {
            let taken = IN_ITS_WORKTREE.contains(&name) || only_reads(name, given);
            match STARTS.contains(&name) {
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
    // not: where it cannot be had, the command is refused before it starts.
    let ran = match checkout {
        Err(error) if named.is_some() => Err(error),
        checkout => {
            let in_it = checkout.as_ref().ok().cloned();
            let acting = cli.command.acting().map(|acting| acting.in_checkout(in_it));
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
            Command::Restart { issue, who, .. } => of("restart", issue, Some(who)),
            Command::Switch { issue, patrol, all, who, .. } => {
                let acting = Acting::new("switch").as_instance(who.instance.as_str());
                match (issue, patrol, all) {
                    (Some(issue), _, _) => acting.on_issue(Some(*issue)),
                    (None, Some(patrol), _) => acting.on(format!("patrol {}", patrol.trim())),
                    (None, None, true) => acting.on("all".to_string()),
                    (None, None, false) => acting,
                }
            }
            Command::Abandon { who, .. } => as_who("abandon", who),
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
            Command::Land { number, issue, .. } => Acting::new("land").on_issue(issue.or(*number)),
            Command::Verdict { issue, .. } => of("verdict", issue, None),
            Command::Intake { issue, .. } => of("intake", issue, None),
            Command::Lock { issue, .. } => of("lock", issue, None),
            Command::PatrolNow { name, .. } => {
                Acting::new("patrol-now").on(name.trim().to_string())
            }
            Command::Over { run, .. } => Acting::new("over").on(format!("run {}", run.trim())),
            Command::Inspected { .. } => match std::env::var(INSPECTION_ENV) {
                Ok(worker) => Acting::new("inspected").on(worker),
                Err(_) => Acting::new("inspected"),
            },
            Command::Gate { .. } => Acting::new("gate").on_own_issue().saying_start(),
            Command::Wake { .. } => Acting::new("wake").on_own_issue(),
            Command::Cast { .. } => Acting::new("cast"),
            Command::Tick { who, .. } => as_who("tick", who),
            Command::Kit
            | Command::Config { .. }
            | Command::Check { .. }
            | Command::Scaffold { .. }
            | Command::Flows { .. }
            | Command::Skill
            | Command::Show { .. }
            | Command::Draft { .. }
            | Command::Rehearse { .. }
            | Command::Doctor { .. }
            | Command::Plan { .. }
            | Command::Run { .. }
            | Command::Stop { .. }
            | Command::HostRunner
            | Command::Checkouts
            | Command::Panel { .. }
            | Command::Guard
            | Command::Exclusive { .. }
            | Command::Recovery { .. }
            | Command::Scan { .. }
            | Command::Help { .. } => return None,
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
        Command::Kit => kit::write(now),
        Command::Cast { .. } => caster::cast(&checkout?, now),
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
        Command::Flows { .. } => flows::flows(&checkout?),
        Command::Skill => Ok(Value::String(SKILL.to_string())),
        Command::Draft { dir, team, yes } => draft::draft(dir, team, yes),
        Command::Show { workflow, issue, team, .. } => {
            // A workflow is not named by a number: one given is an issue's.
            let numbered = workflow.as_deref().and_then(|named| named.parse().ok());
            match issue.or(numbered) {
                // An issue is of the checkout's repo, whichever team reads it.
                Some(issue) => {
                    let repo = checkout?;
                    let loaded = team::load(team.from(), Some(&repo))?;
                    let gh = gh::GitHub { dir: repo.dir.clone() };
                    stage::show_issue(&gh, &loaded.team, issue)
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
        Command::Doctor { all: true, host, proxy, .. } => {
            doctor::all(now, host.as_deref(), &proxy.worker_no_proxy)
        }
        Command::Doctor { host, proxy, .. } => doctor::doctor_with_proxy(
            &checkout?,
            now,
            &host.unwrap_or_else(this_host),
            &proxy.worker_no_proxy,
        ),
        Command::Plan { instance, proxy, .. } => {
            let repo = checkout?;
            let launches = Launches::foreseen(&repo).with_no_proxy(&proxy.worker_no_proxy)?;
            let warnings = match probe::standing(&repo, now, &launches) {
                Some(warnings) => warnings,
                None => probe::startup(&repo, now, &launches)?,
            };
            probe::announce(&warnings);
            let (mut fleet, open) = Fleet::read(&repo, now)?;
            let (mut set, flows) = fleet.rebuild(&open, instance.as_ref())?;
            let admission = admission::Admission::read(&fleet.repo.dir)?;
            set.held_back = admission.holds();
            let mut plan = serde_json::to_value(set)?;
            if let Some(bound) = &fleet.config.bound {
                plan["intake"] = json!({"open": flows.taken_in(), "limit": bound.intake_limit});
                // What each role's range on this host leaves out, and why.
                if let (Ok(team), Ok((kit, _))) = (flows.team(), kit::running()) {
                    let kit = kit.range();
                    let left_out: Vec<Value> = team
                        .roles
                        .iter()
                        .map(|role| json!({"role": role.name, "left_out": team::left_out_json(role, &kit)}))
                        .collect();
                    plan["left_out"] = Value::Array(left_out);
                    // A roster entry that matches nothing, and a role that
                    // can run nothing safely: said, never refusing the plan.
                    let named = |each: Vec<(String, String)>| -> Vec<Value> {
                        each.into_iter()
                            .map(|(role, why)| json!({"role": role, "why": why}))
                            .collect()
                    };
                    let unmatched = named(afkconvoy_core::casting::unmatched_entries(team, &kit));
                    if !unmatched.is_empty() {
                        plan["unmatched"] = Value::Array(unmatched);
                    }
                    let nothing_safe = named(afkconvoy_core::casting::range_faults(team, &kit));
                    if !nothing_safe.is_empty() {
                        plan["nothing_safe"] = Value::Array(nothing_safe);
                    }
                    plan["unmatched_models"] = serde_json::to_value(
                        afkconvoy_core::casting::unmatched_models(team, &kit),
                    )?;
                }
            }
            plan["concurrency"] = concurrency::json(&fleet.config);
            plan["admission"] = admission.json();
            plan["host_kit"] = kit::show(now);
            plan["worker_no_proxy"] = launches.proxy_json();
            Ok(plan)
        }
        Command::Run { .. } => {
            let Some((foreground, start, settings)) = start_of(command, fixed_now) else {
                unreachable!()
            };
            match foreground {
                true => host_runner::foreground(&checkout?, &start, settings),
                false => host_runner::run(&checkout?, start, &settings, now),
            }
        }
        Command::Stop { all: true, .. } => host_runner::stop_all(),
        Command::Stop { .. } => host_runner::stop(&checkout?),
        Command::HostRunner => host_runner::serve(),
        Command::Checkouts => checkouts::checkouts(),
        Command::Panel { open: true, port, .. } => opening::open(&checkout?, port),
        Command::Panel { instance, host, .. } => {
            let (mut fleet, open) = Fleet::read(&checkout?, now)?;
            let host = host.unwrap_or_else(this_host);
            let me = instance.map(|instance| Me { instance, host, now });
            let (set, flows) = fleet.rebuild(&open, me.as_ref().map(|me| &me.instance))?;
            let shown = panel::read(&fleet, me.as_ref(), &set, &flows, &[]);
            Ok(Value::String(panel::render(&shown)))
        }
        Command::Tick { who, agent, .. } => {
            let repo = checkout?;
            let agent = agent.start(&repo, now, true)?;
            let ticked = tick::tick(&repo, &who.me(now), &agent, &stop::Stop::never(), None);
            probe::announce(&agent.launches.warned());
            Ok(ticked?.0.json())
        }
        Command::Gate { train, host, .. } => {
            let repo = checkout?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let host = host.unwrap_or_else(this_host);
            gate::gate(&repo, &config, train, &host, now)
        }
        Command::Land { number, issue, merged_timeout, host, .. } => {
            let repo = checkout?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub { dir: repo.dir.clone() };
            match issue.or(number) {
                Some(issue) => train::land(&repo, &config, &gh, issue, now),
                None => {
                    let merged = Duration::from_secs(merged_timeout);
                    let host = host.unwrap_or_else(this_host);
                    landing::land_train(&repo, &config, &gh, &host, now, merged)
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
        Command::Intake { issue, workflow, role, from, patrol, train, blocked_by, .. } => {
            use afkconvoy_core::intake::{By, On};
            let on = match (from, patrol) {
                (Some(issue), _) => Some(On::Issue(issue)),
                (None, Some(patrol)) => Some(On::Patrol(patrol)),
                (None, None) => None,
            };
            let by = match (train, role, on) {
                (true, None, None) => By::Train,
                (true, ..) => anyhow::bail!(
                    "the train worker has no role and is on no issue and no patrol: `--train` \
                     takes none of them ({ROLE_ENV}, {ISSUE_ENV} and {PATROL_ENV} unset)"
                ),
                (false, Some(role), Some(on)) => By::Role { role, on },
                (false, None, _) => anyhow::bail!(
                    "an intake names the role of its worker (--role, or {ROLE_ENV} in the \
                     environment) — or, the train worker's, `--train`"
                ),
                (false, Some(_), None) => anyhow::bail!(
                    "an intake names what its worker is on: the issue (--from, or {ISSUE_ENV} in \
                     the environment) or the patrol (--patrol, or {PATROL_ENV})"
                ),
            };
            let taking = intake::Taking { issue: issue.number(), workflow, by, blocked_by };
            intake::intake(&checkout?, taking)
        }
        Command::Lock { issue, role, .. } => {
            let repo = checkout?;
            let config = config::load(&repo.dir.join(config::FILE))?;
            let gh = gh::GitHub { dir: repo.dir.clone() };
            let role = role.as_deref().map(str::trim).filter(|role| !role.is_empty());
            lock::lock(&config, &gh, issue.number(), role)
        }
        Command::PatrolNow { name, .. } => patrol::ask(&checkout?, name.trim(), &this_host(), now),
        Command::Over { run, .. } => {
            if let Ok(issue) = std::env::var(ISSUE_ENV) {
                anyhow::bail!(
                    "this worker is on issue #{issue} ({ISSUE_ENV}), whose stage ends with \
                     `afkconvoy verdict`: only a patrol's run and a proposal's are declared over"
                );
            }
            claims::declare_over(&checkout?, run.trim())
        }
        Command::Inspected { found, text, reclaim, name, .. } => {
            inspection::inspected(&checkout?, found, &text.read()?, &reclaim, &name)
        }
        // `main` answers it: a hook's answer is no JSON object.
        Command::Guard => anyhow::bail!("guard answers with its exit code"),
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
        Command::Restart { issue, who, agent, .. } => {
            let repo = checkout?;
            let agent = agent.agent(&repo)?;
            let restarted =
                watch::restart(&mut Fleet::open(&repo, now)?, &who.me(now), &agent, issue.number());
            probe::announce(&agent.launches.warned());
            restarted
        }
        Command::Switch { harness, clear: _, issue, patrol, all, who, agent, .. } => {
            let which = match (issue, patrol, all) {
                (Some(issue), _, _) => switch::Which::Issue(issue),
                (None, Some(patrol), _) => switch::Which::Patrol(patrol.trim().to_string()),
                (None, None, true) => switch::Which::All,
                (None, None, false) => switch::Which::Nothing,
            };
            let repo = checkout?;
            let agent = agent.agent(&repo)?;
            let mut fleet = Fleet::open(&repo, now)?;
            let switched = switch::switch(&mut fleet, &who.me(now), &agent, harness, which);
            probe::announce(&agent.launches.warned());
            switched
        }
        Command::Abandon { who, .. } => {
            abandon::abandon_by_hand(&mut Fleet::open(&checkout?, now)?, &who.me(now))
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
        Command::Scan { .. } => Ok(Scan::read(&checkout?)?.json()),
        Command::Claim { issue, who, .. } => {
            let repo = checkout?;
            claims::take(&repo, &mut Scan::read(&repo)?, &who.me(now), issue.number(), Take::Fresh)
        }
        Command::Release { issue, who, .. } => {
            let repo = checkout?;
            claims::release(&repo, &mut Scan::read(&repo)?, &who.instance, issue.number())
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

    /// A command line of each command, as little as it parses with.
    const LINES: &[&[&str]] = &[
        &["kit"],
        &["config"],
        &["check"],
        &["scaffold", "team", "x"],
        &["flows"],
        &["skill"],
        &["show"],
        &["draft"],
        &["rehearse", "build"],
        &["doctor"],
        &["plan"],
        &["run"],
        &["stop"],
        &["checkouts"],
        &["host-runner"],
        &["panel"],
        &["tick", "--instance", "a-h1-1"],
        &["gate"],
        &["land", "7"],
        &["verdict", "pass", "7", "--stage", "build", "--text", "x"],
        &["rule", "7", "--text", "x"],
        &["send", "7", "--stage", "build", "--text", "x"],
        &["intake", "7", "--workflow", "w", "--role", "r", "--from", "3"],
        &["lock", "7"],
        &["patrol-now", "sweep"],
        &["over", "--run", "sweep"],
        &["inspected", "--found", "heavy", "--text", "x"],
        &["guard"],
        &["exclusive", "--", "true"],
        &["dispatch", "7", "--instance", "a-h1-1"],
        &["recovery", "7"],
        &["close", "7", "--instance", "a-h1-1"],
        &["escalate", "7", "--instance", "a-h1-1", "--reason", "x"],
        &["park", "7", "--instance", "a-h1-1"],
        &["nudge", "7", "--instance", "a-h1-1"],
        &["restart", "7", "--instance", "a-h1-1"],
        &["switch", "7", "--harness", "codex", "--instance", "a-h1-1"],
        &["abandon", "--instance", "a-h1-1"],
        &["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"],
        &["wake"],
        &["cast"],
        &["scan"],
        &["claim", "7", "--instance", "a-h1-1"],
        &["release", "7", "--instance", "a-h1-1"],
        &["heartbeat", "--instance", "a-h1-1"],
        &["help"],
    ];

    /// The manual is whole: every command has its text and its group, and
    /// every placeholder and every link to a topic names one there is.
    #[test]
    fn the_manual_is_whole() {
        agent_help::check(&manual()).unwrap_or_else(|faults| panic!("{faults}"));
    }

    /// The commands that act are those whose `--help` says what they log, and
    /// none else: a test of the text `--help` gives, not of `ACTS` alone.
    #[test]
    fn a_command_logs_and_wakes_where_its_help_says_so_and_only_there() {
        let manual = manual();
        let mut listed = vec![];
        for command in cli().get_subcommands() {
            let name = command.get_name().to_string();
            let help = manual.command_help(&[&name]).unwrap();
            let line = LINES.iter().find(|line| line[0] == name);
            let line = line.unwrap_or_else(|| panic!("no command line of `{name}` here"));
            let parsed =
                Cli::try_parse_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            let acts = parsed.unwrap_or_else(|e| panic!("{name}: {e}")).command.acting().is_some();
            assert_eq!(acts, help.contains("\n## What it logs\n"), "{name}");
            assert_eq!(acts, ACTS.contains(&name.as_str()), "{name}");
            listed.push(name);
        }
        assert_eq!(listed.len(), LINES.len(), "{listed:?}");
        // The listing with `--list` changes nothing, and says nothing.
        let list = Cli::try_parse_from(["afkconvoy", "takeover", "--list"]).unwrap();
        assert!(list.command.acting().is_none());
        // What one acts on is named, as its wake names it.
        let named = |line: &[&str]| {
            let cli = Cli::try_parse_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            cli.unwrap().command.acting().unwrap().named()
        };
        assert_eq!(named(&["park", "--issue", "46", "--instance", "a-h1-1"]), "park #46");
        assert_eq!(named(&["over", "--run", " sweep "]), "over run sweep");
        assert_eq!(named(&["patrol-now", "sweep"]), "patrol-now sweep");
        assert_eq!(
            named(&["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"]),
            "takeover from b-h2-2"
        );
        assert_eq!(named(&["land", "--train"]), "land");
    }

    /// The commands a worker's worktree is taken for are commands about one
    /// repo, and among them every one the manual says a worker runs.
    #[test]
    fn a_workers_worktree_is_taken_for_every_command_a_worker_runs() {
        let cli = Cli::command();
        let about = |command: &clap::Command| {
            command.get_arguments().any(|arg| arg.get_id() == CHECKOUT_ARG)
        };
        for name in IN_ITS_WORKTREE {
            let command = cli.find_subcommand(name).unwrap_or_else(|| panic!("no `{name}`"));
            assert!(about(command), "`{name}` is about no repo");
        }
        let workers = cli.get_subcommands().filter(|c| c.get_next_help_heading() == Some(WORKER));
        let workers: Vec<&str> = workers.map(clap::Command::get_name).collect();
        assert!(workers.len() >= 9, "{workers:?}");
        for name in workers {
            assert!(IN_ITS_WORKTREE.contains(&name), "a worker runs `{name}`");
        }
        // Those that only read are about one repo and no worker's own, and
        // the flag that makes one act is one of its own: one with none acts
        // never.
        for (name, with) in ONLY_READ {
            let command = cli.find_subcommand(name).unwrap_or_else(|| panic!("no `{name}`"));
            assert!(about(command), "`{name}` is about no repo");
            assert!(!IN_ITS_WORKTREE.contains(&name), "`{name}` is a worker's own");
            match with {
                Some(flag) => assert!(command.get_arguments().any(|arg| arg.get_id() == flag)),
                None => assert!(!ACTS.contains(&name), "`{name}` acts"),
            }
        }
        for name in STARTS {
            assert!(cli.find_subcommand(name).is_some_and(about), "`{name}` is about no repo");
        }
    }

    /// A command that only reads but for a flag is taken in a worker's
    /// worktree without that flag, and not with it.
    #[test]
    fn a_command_that_only_reads_but_for_a_flag_reads_only_without_it() {
        let reads = |line: &[&str]| {
            let matches = cli()
                .try_get_matches_from(std::iter::once("afkconvoy").chain(line.iter().copied()));
            let matches = matches.unwrap();
            let (name, given) = matches.subcommand().unwrap();
            only_reads(name, given)
        };
        assert!(reads(&["takeover", "--list"]));
        assert!(!reads(&["takeover", "--from", "b-h2-2", "--instance", "a-h1-1"]));
        assert!(reads(&["panel"]));
        assert!(!reads(&["panel", "--open"]));
        assert!(reads(&["scan"]));
        assert!(!reads(&["claim", "7", "--instance", "a-h1-1"]));
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
        // of `--host-sleep` and `--no-host-sleep`, `--host-sleep` holds
        assert_eq!(asked(&["--no-host-sleep"]).1.no_host_sleep, Some(true));
        assert_eq!(asked(&["--host-sleep", "--no-host-sleep"]).1.no_host_sleep, Some(false));
        let line = ["--hot-cores", "2.5", "--admit-disk", "0", "--admit-load", "default"];
        let (start, named) = asked(&[&line[..], &["--panel", "7070"]].concat());
        assert_eq!(named.hot_cores, Some(To::Value(2500)));
        assert_eq!(named.admit_disk, Some(To::Value(0)), "0 never holds for disk");
        assert_eq!(named.admit_load, Some(To::Default));
        assert_eq!(named.panel, Some(To::Value(7070)));
        // the panel is served from this start on the port named, and on none
        // where it is set back to its default
        assert_eq!(start.panel, Some(7070));
        let (start, named) = asked(&["--panel", "default"]);
        assert_eq!((start.panel, named.panel), (None, Some(To::Default)));
        assert!(Cli::try_parse_from(["afkconvoy", "run", "--hot-cores", "0"]).is_err());
        assert!(Cli::try_parse_from(["afkconvoy", "run", "--admit-load", "5%"]).is_err());
        let night = afkconvoy_core::host_sleep::Hours::parse("22:00-06:00").unwrap();
        let named = asked(&["--host-sleep-hours", "22:00-06:00"]).1;
        assert_eq!(named.host_sleep_hours, Some(To::Value(night)));
        let named = asked(&["--host-sleep-hours", "default"]).1;
        assert_eq!(named.host_sleep_hours, Some(To::Default));
        assert_eq!(asked(&[]).1.host_sleep_hours, None, "named none, it changes none");
        // hours that cover nothing are refused, naming what they take
        let line = ["afkconvoy", "run", "--host-sleep-hours", "06:00-06:00"];
        let refused = Cli::try_parse_from(line).err().unwrap().to_string();
        assert!(refused.contains("covers no time of the day"), "{refused}");
        assert!(refused.contains("HH:MM-HH:MM"), "{refused}");
    }
}
