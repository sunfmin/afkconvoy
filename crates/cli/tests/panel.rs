//! The panel, rendered: the real `afkconvoy panel` over the stand-in GitHub
//! and orca, its markup asserted on and its picture written beside it — and
//! the page `run --panel` serves, fetched from a running fleet, as is the one
//! `panel --open` has a fleet that is already running serve.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use common::{Rendered, World, bookkeeping, card, carded, git, write};
use serde_json::json;

const NOW: u64 = 10_000;
const DESK: (u32, u32) = (1440, 1000);

/// The world's config, with room for three claims and a gate that passes.
fn configured(world: &World, more: &str) {
    let config =
        format!("```yaml\nbase_branch: main\ngate_command: \"true\"\nconcurrency: 3\n{more}```\n");
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "configure"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
}

fn afk(world: &World, command: &[&str], now: u64) {
    let now = now.to_string();
    world.afk(&[command, &["--now", &now]].concat()).ok();
}

fn tick(world: &World, now: u64) {
    let who = ["tick", "--instance", "alpha", "--host", "h1", "--worker-command", "agent"];
    afk(world, &who, now);
}

/// Gives each issue the title a real backlog would.
fn title(world: &World, titles: &[(u64, &str)]) {
    world.with_gh(|gh| {
        for issue in gh["issues"].as_array_mut().unwrap() {
            if let Some((_, title)) = titles.iter().find(|(number, _)| issue["number"] == *number) {
                issue["title"] = json!(title);
            }
        }
    });
}

/// Has the worker in the `at`-th terminal report `state` since `since`.
fn worker(world: &World, at: usize, state: &str, since: u64) {
    world.with_orca(|orca| {
        let terminal = &mut orca["terminals"][at];
        terminal["agent"] = json!({"state": state, "stateStartedAt": since * 1000});
        terminal["lastOutputAt"] = json!(since * 1000);
    });
}

fn shown(world: &World, name: &str, size: (u32, u32)) -> Rendered {
    let now = NOW.to_string();
    world.render_panel(name, &["--instance", "alpha", "--host", "h1", "--now", &now], size)
}

/// A fleet in the middle of its work: two workers out, one PR joining, one on
/// the train, three issues queued, a peer's claim, and two the fleet leaves.
fn busy() -> World {
    let world = World::new();
    configured(&world, "");
    world.issue(3, &["ready-for-agent"]);
    world.issue(5, &["ready-for-agent"]);
    tick(&world, NOW - 900);
    worker(&world, 0, "working", NOW - 20);
    worker(&world, 1, "done", NOW - 725);
    let branch = world.orca()["worktrees"][1]["branch"].as_str().unwrap_or("issue-5").to_string();
    world.pr(50, 5, branch.trim_start_matches("refs/heads/"));

    let joined = world.worker(7, "seven.txt", "seven\n");
    afk(&world, &["claim", "--issue", "7", "--instance", "alpha", "--host", "h1"], NOW - 600);
    world.afk_in(&joined, &["land", "--issue", "7", "--now", "9500"]).ok();

    for queued in [9, 12, 14] {
        world.issue(queued, &["ready-for-agent"]);
    }
    world.issue(30, &["ready-for-agent"]);
    afk(&world, &["claim", "--issue", "30", "--instance", "beta", "--host", "h2"], NOW - 60);
    afk(&world, &["heartbeat", "--instance", "beta", "--host", "h2"], NOW - 60);
    world.issue(20, &["needs-triage"]);
    world.issue(21, &["ready-for-agent", "epic"]);
    title(
        &world,
        &[
            (3, "Retry the gate once when the runner loses its network"),
            (5, "Say which blocker holds an issue back"),
            (7, "A worker's brief names the base branch"),
            (9, "Trim the status board to what changed"),
            (12, "`plan` prints the lease left on every claim"),
            (14, "Refuse a config with two gate commands"),
            (20, "Should a patrol ever write code?"),
            (21, "Patrols"),
            (30, "Heartbeats survive a clock set back"),
        ],
    );
    world
}

#[test]
fn a_busy_fleet_shows_every_issue_once_where_it_is() {
    let world = busy();
    let page = shown(&world, "busy", DESK);

    // the two stages of an unbound repo's one lane, and who is at each
    let work = page.node("work");
    assert_eq!(carded(work, "at-work"), [3, 30], "{work}");
    assert_eq!(carded(work, "queued"), [9, 12, 14], "the frontier, lowest number first");
    assert!(work.contains("<div class=\"stop\"><span>2</span>"), "{work}");
    let three = card(work, 3);
    assert_eq!(three, "#3 Retry the gate once when the runner loses its network busy");
    assert_eq!(card(work, 30), "#30 Heartbeats survive a clock set back held by beta");
    let land = page.node("land");
    assert_eq!(carded(land, "at-work"), [5]);
    assert_eq!(card(land, 5), "#5 Say which blocker holds an issue back stopped 12m ago PR #50");

    // …the train they end on, and what the fleet leaves
    let train = page.part("section", "train");
    assert_eq!(carded(train, "cars"), [7]);
    assert!(train.contains("1 PR, gated, then landed on main together"), "{train}");
    let off = page.part("details", "off");
    assert_eq!(carded(off, "left"), [20, 21]);
    assert_eq!(card(off, 20), "#20 Should a patrol ever write code? no ready-for-agent label");
    assert_eq!(card(off, 21), "#21 Patrols epic label (epic)");

    // every open issue is on the page exactly once
    for issue in [3, 5, 7, 9, 12, 14, 20, 21, 30] {
        let cards = page.html.matches(&format!("data-issue=\"{issue}\"")).count();
        assert_eq!(cards, 1, "#{issue}");
    }
    let gauges = page.part("dl", "gauges");
    for said in ["Claims held</dt><dd>3 <small>of 3", "Queued</dt><dd>3<", "train</dt><dd>1<"] {
        assert!(gauges.contains(said), "{said}: {gauges}");
    }
    assert!(page.html.contains("Fleet instance <b>alpha</b> on h1, landing on <b>main</b>"));
    assert!(page.png.is_none_or(|png| png.metadata().unwrap().len() > 20_000), "a blank picture");
}

#[test]
fn a_busy_fleet_reads_on_a_phone_too() {
    let page = shown(&busy(), "busy-phone", (500, 1300));
    assert_eq!(carded(page.node("work"), "queued"), [9, 12, 14]);
}

#[test]
fn a_fleet_with_nothing_to_do_says_so_at_every_stage() {
    let world = World::new();
    let page = shown(&world, "empty", (1440, 520));
    assert_eq!(page.html.matches("data-issue=").count(), 0);
    for stage in ["work", "land"] {
        let node = page.node(stage);
        assert!(
            node.contains("<span>0</span>") && node.contains("<span>nothing here</span>"),
            "{node}"
        );
    }
    assert!(page.part("section", "train").contains("No PR is on the train."));
    assert!(page.part("dl", "gauges").contains("Claims held</dt><dd>0 <small>of 2"));
    assert!(!page.html.contains("class=\"off\"") && !page.html.contains("class=\"lock\""));
    // no `run` made it: it says what it is, and how to get one kept fresh
    assert!(page.html.contains("<code>afkconvoy run --panel</code>"));
}

#[test]
fn a_fleet_lock_is_said_first_and_holds_back_all_but_its_issue() {
    let world = World::new();
    world.issue(4, &["ready-for-agent"]);
    world.issue(6, &["ready-for-agent", "fleet-lock"]);
    world.issue(8, &["ready-for-agent"]);
    let page = shown(&world, "locked", (1440, 620));
    assert!(page.html.contains("<b>A fleet lock stands: #6.</b>"), "{}", page.html);
    assert_eq!(carded(page.node("work"), "queued"), [6]);
    let off = page.part("details", "off");
    assert_eq!(carded(off, "left"), [4, 8]);
    assert_eq!(card(off, 4), "#4 issue 4 a fleet lock stands (#6)");
}

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: plan
    by: dev
    do: plan
    on: { pass: build }
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: sign-off, reject: build }
  - name: sign-off
    by: human
    on: { pass: land, reject: plan }
land: { by: dev, do: land }
```
";

/// Declares `verdict` for `stage` of `issue` under a claim made for it, and
/// ends the claim: one whole stage, as the fleet and a worker go through it.
fn through(world: &World, issue: u64, stage: &str, verdict: &str, now: u64) {
    let number = issue.to_string();
    afk(world, &["claim", "--issue", &number, "--instance", "alpha", "--host", "h1"], now);
    let commits = ["--role-commit", "r1", "--team-commit", "t1", "--text", "done"];
    world
        .afk(&[&["verdict", verdict, "--issue", &number, "--stage", stage], &commits[..]].concat())
        .ok();
    world.afk(&["release", "--issue", &number, "--instance", "alpha"]).ok();
}

#[test]
fn a_bound_repo_shows_each_workflow_as_its_stages_with_the_issues_at_each() {
    let world = World::new();
    let skill = |name: &str| format!(".claude/skills/{name}/SKILL.md");
    let dev = [skill("plan"), skill("implement"), skill("land")];
    let dev: Vec<(&str, &str)> = dev.iter().map(|file| (file.as_str(), "do it\n")).collect();
    world.repo("acme/dev", &dev);
    world.repo("acme/reviewer", &[(skill("review").as_str(), "review\n")]);
    world.repo("acme/team", &[("team.md", TEAM), ("workflows/feature.md", FEATURE)]);
    configured(&world, "team: acme/team\n");
    for issue in [7, 8, 9, 10, 11] {
        world.issue(issue, &["flow:feature"]);
    }
    world.issue(15, &["needs-triage"]);
    title(
        &world,
        &[
            (7, "Patrols open issues on a schedule"),
            (8, "A consult rules before a human is asked"),
            (9, "A stage splits its issue"),
            (10, "Precedents are proposed by a worker"),
            (11, "`show` draws a workflow's breakers"),
            (15, "Should a patrol ever write code?"),
        ],
    );
    // #7 is through to the human's stage, #8 waits for review, #9 for its build
    through(&world, 7, "plan", "pass", NOW - 900);
    through(&world, 7, "build", "pass", NOW - 800);
    through(&world, 7, "review", "pass", NOW - 700);
    through(&world, 8, "plan", "pass", NOW - 600);
    through(&world, 8, "build", "pass", NOW - 500);
    through(&world, 9, "plan", "pass", NOW - 400);
    // …and a worker is planning #10
    afk(&world, &["claim", "--issue", "10", "--instance", "alpha", "--host", "h1"], NOW - 30);

    let page = shown(&world, "bound", DESK);
    let stages: Vec<&str> =
        page.html.split("data-stage=\"").skip(1).map(|at| at.split('"').next().unwrap()).collect();
    assert_eq!(stages, ["plan", "build", "review", "sign-off", "land"]);
    assert!(page.html.contains("<h2>feature</h2>"));
    assert!(page.node("review").contains("<p class=\"by\">reviewer: review</p>"));
    assert!(page.node("sign-off").contains("<p class=\"by\">a human</p>"));

    assert_eq!(carded(page.node("plan"), "at-work"), [10]);
    assert_eq!(carded(page.node("plan"), "queued"), [11]);
    assert_eq!(carded(page.node("build"), "queued"), [9]);
    assert_eq!(carded(page.node("review"), "queued"), [8]);
    let sign_off = page.node("sign-off");
    assert_eq!(carded(sign_off, "held"), [7], "{sign_off}");
    assert!(card(sign_off, 7).starts_with("#7 Patrols open issues on a schedule "), "{sign_off}");
    assert_eq!(carded(page.part("details", "off"), "left"), [15]);
    assert!(
        sign_off.contains("class=\"node asking\"")
            || page.html.contains("node asking\" data-stage=\"sign-off\"")
    );
    assert!(sign_off.contains("<span><b>1</b> with a human</span>"), "{sign_off}");
    assert!(
        page.node("plan").contains("<span><b>1</b> at work</span><span><b>1</b> queued</span>")
    );
    assert!(page.part("dl", "gauges").contains("With a human</dt><dd>1<"));
    assert!(page.part("dl", "gauges").contains("Queued</dt><dd>3<"));
    // What waits at a human's stage is listed with the line that answers it.
    let with_a_human = page.part("section", "human");
    assert_eq!(carded(with_a_human, "asked"), [7], "{with_a_human}");
    assert!(
        with_a_human.contains(
            "stage sign-off is by a human — afkconvoy verdict pass|reject --issue 7 --stage \
             sign-off --text-file &lt;file&gt;"
        ),
        "{with_a_human}"
    );
}

fn get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut answer = String::new();
    // An `events` answer never ends: what came before the wait ran out is it.
    let _ = stream.read_to_string(&mut answer);
    answer
}

#[test]
fn a_running_fleet_serves_its_panel_and_brings_it_up_to_date_after_each_tick() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let args = ["run", "--instance", "alpha", "--host", "h1", "--worker-command", "agent"];
    let mut run = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args)
        .args(["--panel", "0", "--second-ms", "20"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(run.stdout.take().unwrap());
    let mut line = String::new();
    lines.read_line(&mut line).unwrap();
    let port: u16 = line
        .trim()
        .strip_prefix("panel: http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or_else(|| panic!("the first line names the panel: {line:?}"))
        .parse()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut page = get(port, "/");
    while !page.contains("<b>Tick 1</b>") {
        assert!(Instant::now() < deadline, "no tick was ever shown: {page}");
        std::thread::sleep(Duration::from_millis(50));
        page = get(port, "/");
    }
    let _ = run.kill();
    let _ = run.wait();

    // the tick that started the worker shows the issue at work, and says so
    let page = Rendered { html: page, png: None };
    let work = page.node("work");
    assert_eq!(carded(work, "at-work"), [3], "{work}");
    assert_eq!(carded(work, "queued"), [0u64; 0]);
    assert_eq!(card(work, 3), "#3 issue 3 started this tick");
    assert!(page.html.contains("<span class=\"did\">dispatched #3 · holding 1/2 · frontier 0"));
    assert!(page.html.contains("Claims held</dt><dd>1 <small>of 2"));
}

/// How long anything a running fleet should do is waited for. Its sleep
/// between two ticks is far longer, so "within this" means "without it".
const SOON: Duration = Duration::from_secs(30);

/// A `run` as alpha with a real sleep between its ticks, 90 seconds at the
/// least, until it is dropped. Its stdout comes line by line.
struct Running {
    child: Child,
    lines: Receiver<String>,
}

fn run(world: &World, more: &[&str]) -> Running {
    let mut child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["run", "--instance", "alpha", "--host", "h1", "--worker-command", "agent"])
        .args(more)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let (send, lines) = channel();
    std::thread::spawn(move || {
        // What it prints whatever it does is none of these tests'.
        let lines = BufReader::new(out).lines().map_while(Result::ok);
        for line in lines.filter(|line| !bookkeeping(line)) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    Running { child, lines }
}

impl Running {
    /// The next line `run` prints, which comes soon.
    fn line(&self) -> String {
        self.lines.recv_timeout(SOON).expect("a line of the run")
    }

    fn sleeps(&mut self) {
        assert_eq!(self.lines.recv_timeout(Duration::from_millis(300)).ok(), None, "it sleeps");
        assert!(self.child.try_wait().unwrap().is_none(), "the run ended");
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `afkconvoy panel --open` in `dir`, which succeeds → its answer.
fn open(world: &World, dir: &Path, more: &[&str]) -> serde_json::Value {
    world.afk_in(dir, &[&["panel", "--open"], more].concat()).ok()
}

/// The page at `port` once it shows the tick `count`.
fn page_of_tick(port: u16, count: u64) -> Rendered {
    let deadline = Instant::now() + SOON;
    let mut page = get(port, "/");
    while !page.contains(&format!("<b>Tick {count}</b>")) {
        assert!(Instant::now() < deadline, "tick {count} was never shown: {page}");
        std::thread::sleep(Duration::from_millis(50));
        page = get(port, "/");
    }
    Rendered { html: page, png: None }
}

fn port_of(answer: &serde_json::Value) -> u16 {
    let port = answer["port"].as_u64().unwrap() as u16;
    assert_eq!(answer["panel"], format!("http://127.0.0.1:{port}/"), "{answer}");
    port
}

/// A port of this host nothing listens on.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

#[test]
fn a_fleet_running_without_its_panel_opens_it_when_asked_and_keeps_it_fresh() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let mut running = run(&world, &[]);
    let first = running.line();
    assert!(first.starts_with("tick 1: dispatched #3"), "no panel is named: {first}");
    assert!(first.ends_with("next in 90s"), "{first}");
    running.sleeps();

    // asked from the worker's worktree: the repo's fleet instance is what is reached
    let worktree = world.orca()["worktrees"][0]["path"].as_str().unwrap().to_string();
    let answer = open(&world, Path::new(&worktree), &[]);
    assert_eq!(answer["opened"], true, "{answer}");
    assert_eq!(answer["instance"], "alpha", "{answer}");
    let port = port_of(&answer);
    assert_eq!(running.line(), format!("panel: http://127.0.0.1:{port}/"));
    // every key of the answer is one the help names, as is each way it fails
    let help = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    let help = { help }.args(["panel", "--help"]).output().unwrap().stdout;
    let help = String::from_utf8(help).unwrap();
    for key in answer.as_object().unwrap().keys() {
        assert!(help.contains(&format!("\n    {key} ")), "`panel --help` never names {key:?}");
    }
    for said in [
        "afkconvoy panel --open --port 7070",
        "no afkconvoy run is running for this repo on this host",
        "the panel cannot be served on port <port>",
        "did not answer",
    ] {
        assert!(help.contains(said), "`panel --help` never says {said:?}");
    }

    // the sleep ends for it, and the tick that follows fills the page
    let second = running.line();
    assert!(second.starts_with("tick 2: nothing to do · holding 1/2"), "{second}");
    let page = page_of_tick(port, 2);
    assert_eq!(carded(page.node("work"), "at-work"), [3]);
    assert!(page.html.contains("Fleet instance <b>alpha</b> on h1"), "{}", page.html);

    // an open page is told of the next tick, which shows what that one read
    let mut events = TcpStream::connect(("127.0.0.1", port)).unwrap();
    events.set_read_timeout(Some(SOON)).unwrap();
    write!(events, "GET /events HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut events = BufReader::new(events);
    let mut told = String::new();
    while !told.ends_with("\r\n\r\n") {
        assert_ne!(events.read_line(&mut told).unwrap(), 0, "the events ended: {told}");
    }
    world.issue(9, &["ready-for-agent"]);
    world.afk(&["wake"]).ok();
    while !told.contains("event: tick\n") {
        assert_ne!(events.read_line(&mut told).unwrap(), 0, "the events ended: {told}");
    }
    let third = running.line();
    assert!(third.starts_with("tick 3: dispatched #9"), "{third}");
    assert_eq!(carded(page_of_tick(port, 3).node("work"), "at-work"), [3, 9]);

    // asked again, from anywhere and for any port, it is where it was
    let again = open(&world, &world.clone, &[]);
    assert_eq!((again["opened"].clone(), port_of(&again)), (json!(false), port), "{again}");
    let other = free_port().to_string();
    let moved = open(&world, Path::new(&worktree), &["--port", &other]);
    assert_eq!((moved["opened"].clone(), port_of(&moved)), (json!(false), port), "{moved}");
    assert!(TcpStream::connect(("127.0.0.1", free_port())).is_err());
    assert!(TcpStream::connect(("127.0.0.1", other.parse::<u16>().unwrap())).is_err());

    // …and the fleet instance is the one it was: no tick more, no claim moved
    running.sleeps();
    assert_eq!(world.claims(), [(3, "alpha".to_string()), (9, "alpha".to_string())]);
    assert_eq!(world.open_terminals().len(), 2, "its workers run on");
}

#[test]
fn a_panel_asked_for_during_a_tick_is_opened_at_once_and_the_tick_goes_on() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    // every GitHub call takes a while, so the first tick does
    world.with_gh(|gh| gh["slow_ms"] = json!(150));
    let running = run(&world, &[]);
    let deadline = Instant::now() + SOON;
    while !world.gh()["calls"].to_string().contains("graphql") {
        assert!(Instant::now() < deadline, "the first tick never read");
        std::thread::sleep(Duration::from_millis(5));
    }

    let answer = open(&world, &world.clone, &[]);
    assert_eq!(answer["opened"], true, "{answer}");
    let port = port_of(&answer);
    // answered while the tick still runs: the address comes before its line
    assert_eq!(running.line(), format!("panel: http://127.0.0.1:{port}/"));
    assert!(get(port, "/").starts_with("HTTP/1.1 200 OK"), "it is served before any tick ends");
    // its opening wakes the run, to fill the page at once
    assert_eq!(running.line(), "woken during tick 1: tick 2 follows");
    let first = running.line();
    assert!(first.starts_with("tick 1: dispatched #7 · holding 1/2"), "it was not cut: {first}");

    // the tick that follows is the first made for the page, and comes with no sleep
    let second = running.line();
    assert!(second.starts_with("tick 2: "), "{second}");
    assert_eq!(carded(page_of_tick(port, 2).node("work"), "at-work"), [7]);
}

#[test]
fn a_panel_served_from_the_start_is_the_one_found() {
    let world = World::new();
    let running = run(&world, &["--panel", "0"]);
    let named = running.line();
    assert!(running.line().starts_with("tick 1: nothing to do"));

    let other = free_port().to_string();
    let found = open(&world, &world.clone, &["--port", &other]);
    assert_eq!(found["opened"], false, "{found}");
    assert_eq!(named, format!("panel: {}", found["panel"].as_str().unwrap()));
    assert!(page_of_tick(port_of(&found), 1).html.contains("nothing to do"));
    assert_eq!(running.lines.recv_timeout(Duration::from_millis(300)).ok(), None, "no tick more");
}

#[test]
fn a_panel_that_cannot_be_opened_says_why_and_the_fleet_works_on() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let quiet = World::new();
    let refs = quiet.remote(&["for-each-ref"]);

    // no fleet instance runs for either repo
    for repo in [&world, &quiet] {
        let error = repo.afk(&["panel", "--open"]).error();
        assert!(error.starts_with("no afkconvoy run is running for this repo on this host"));
        assert!(error.contains("`afkconvoy run`"), "{error}");
        assert!(!repo.clone.join(".git/convoy-panel.sock").exists(), "it left nothing");
    }

    let mut running = run(&world, &[]);
    assert!(running.line().starts_with("tick 1: dispatched #3"));
    // …and one that runs for another repo is not this repo's
    let error = quiet.afk(&["panel", "--open"]).error();
    assert!(error.starts_with("no afkconvoy run is running for this repo"), "{error}");
    assert_eq!(quiet.remote(&["for-each-ref"]), refs);
    assert_eq!(quiet.gh_calls(), [] as [Vec<String>; 0]);

    // a port something else has
    let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let error = world.afk(&["panel", "--open", "--port", &port]).error();
    assert!(error.starts_with("the panel was not opened, and the run works on as it was"));
    assert!(error.contains(&format!("the panel cannot be served on port {port}")), "{error}");
    running.sleeps();
    assert_eq!(world.claims(), [(3, "alpha".to_string())]);

    // a port that is none is refused before anything is asked
    let none = world.afk(&["panel", "--open", "--port", "70000"]);
    assert_eq!(none.code, 2, "{none:?}");
    let alone = world.afk(&["panel", "--port", "7070"]);
    assert_eq!(alone.code, 2, "--port is --open's: {alone:?}");
    running.sleeps();

    // what was refused opened nothing: it is opened now, on a port that is free
    let answer = open(&world, &world.clone, &[]);
    assert_eq!(answer["opened"], true, "{answer}");
    assert_eq!(running.line(), format!("panel: {}", answer["panel"].as_str().unwrap()));
    assert!(running.line().starts_with("tick 2: "));

    // the run over, its socket is nobody's
    drop(running);
    let error = world.afk(&["panel", "--open"]).error();
    assert!(error.starts_with("no afkconvoy run is running for this repo"), "{error}");
}
