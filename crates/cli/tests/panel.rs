//! The panel, rendered: the real `afkconvoy panel` over the stand-in GitHub
//! and orca, its markup asserted on and its picture written beside it — and
//! the page the host runner serves from its start, fetched as its fleet
//! instances tick, at the address `panel --open` answers.

use crate::common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{Rendered, World, card, carded, git, write};
use serde_json::{Value, json};

const NOW: u64 = 10_000;
const DESK: (u32, u32) = (1440, 1000);

/// The world's config, with a gate that passes: its fleet holds three
/// claims at once, the default.
fn configured(world: &World, more: &str) {
    let config = format!("```yaml\nbase_branch: main\ngate_command: \"true\"\n{more}```\n");
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "configure"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
}

fn afk(world: &World, command: &[&str], now: u64) {
    let now = now.to_string();
    world.afk(&[command, &["--now", &now]].concat()).ok();
}

fn tick(world: &World, now: u64) {
    let who = ["tick", "--instance", "alpha", "--host", "h1"];
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

    world.worker(7, "seven.txt", "seven\n");
    afk(&world, &["claim", "--issue", "7", "--instance", "alpha", "--host", "h1"], NOW - 600);

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
    assert_eq!(carded(land, "at-work"), [5, 7]);
    assert_eq!(card(land, 5), "#5 Say which blocker holds an issue back stopped 12m ago PR #50");

    assert!(!page.html.contains(r#"class="train""#));
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
    for said in ["Claims held</dt><dd>1<", "Taking no slot</dt><dd>2<", "Queued</dt><dd>3<"] {
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
    world.list_checkout(&world.clone);
    let page = shown(&world, "empty", (1440, 520));
    assert_eq!(page.html.matches("data-issue=").count(), 0);
    for stage in ["work", "land"] {
        let node = page.node(stage);
        assert!(
            node.contains("<span>0</span>") && node.contains("<span>nothing here</span>"),
            "{node}"
        );
    }
    assert!(!page.html.contains("class=\"train\""));
    assert!(page.part("dl", "gauges").contains("Claims held</dt><dd>0<"));
    assert!(!page.html.contains("class=\"off\"") && !page.html.contains("class=\"lock\""));
    // no host runner made it: it says what it is, and where the live one is
    assert!(page.html.contains("one rebuild of each checkout, read just now"));
    assert!(page.html.contains("<code>afkconvoy panel --open</code> says where"));
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
    let reason = world.comments(7).pop().unwrap();
    let reason_url = world.comment_url(7, &reason);
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
    let answer = format!(
        "stage sign-off is by a human: done — afkconvoy verdict -C '{}' pass|reject --issue 7 \
         --stage sign-off --text-file &lt;file&gt;",
        world.clone.display()
    );
    assert!(with_a_human.contains(&answer), "{with_a_human}");
    assert!(
        with_a_human.contains(&format!("<a href=\"{reason_url}\">Reason</a>")),
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

/// A `run` of the first repo on the host `h1`, its sleeps short: handed to
/// the host runner, detached, which the world kills as it ends.
const RUN: [&str; 5] = ["run", "--host", "h1", "--second-ms", "100"];

/// How long anything the host runner should do is waited for.
const SOON: Duration = Duration::from_secs(30);

/// The port of the panel's address `panel`: `http://127.0.0.1:<port>/`.
fn port_in(panel: &Value) -> u16 {
    let address = panel.as_str().unwrap_or_else(|| panic!("no address: {panel}"));
    let port = address.strip_prefix("http://127.0.0.1:").and_then(|rest| rest.strip_suffix('/'));
    port.unwrap_or_else(|| panic!("{address}")).parse().unwrap()
}

/// The page served on `port` once it holds `said`.
fn page_with(port: u16, said: &str) -> Rendered {
    let deadline = Instant::now() + SOON;
    let mut page = get(port, "/");
    while !page.contains(said) {
        assert!(Instant::now() < deadline, "{said:?} was never shown: {page}");
        std::thread::sleep(Duration::from_millis(50));
        page = get(port, "/");
    }
    Rendered { html: page, png: None }
}

/// A port of this host nothing listens on.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

/// What `afkconvoy panel` prints, run in `dir` with `args`.
fn printed(world: &World, dir: &Path, args: &[&str]) -> Rendered {
    let program = env!("CARGO_BIN_EXE_afkconvoy");
    let out = world.command(program, dir).arg("panel").args(args).output().unwrap();
    let html = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && html.starts_with("<!doctype html>"), "{html}{stderr}");
    Rendered { html, png: None }
}

/// The parts of the repos on `page`, by the checkout each is of.
fn checkouts(page: &Rendered) -> Vec<String> {
    let parts = page.html.split("<section class=\"repo\" id=\"checkout-").skip(1);
    let checkout = |part: &str| {
        let at = part.split("data-checkout=\"").nth(1).unwrap();
        at.split('"').next().unwrap().to_string()
    };
    parts.map(checkout).collect()
}

#[test]
fn the_host_runner_serves_one_page_of_every_checkout_from_its_start_and_after_every_tick() {
    let world = World::new();
    world.list_checkout(&world.clone);
    world.issue(3, &["ready-for-agent"]);
    let port = free_port();
    world.say_host_settings(json!({"panel": port}));
    let first = world.afk(&RUN).ok();
    let address = format!("http://127.0.0.1:{port}/");
    // served from its start, on the port the host settings say, and said so
    assert_eq!(first["host_runner"]["panel"], address, "{first}");
    let log = world.runner_log();
    assert!(log.contains(&format!("panel: {address}")), "{log:#?}");
    let page = page_with(port, "<b>Tick 1</b>");
    assert_eq!(carded(page.node("work"), "at-work"), [3], "{}", page.html);
    assert!(page.html.contains("dispatched #3 · holding 1 · frontier 0"), "{}", page.html);
    assert!(page.html.contains("Claims held</dt><dd>1<"), "{}", page.html);

    // a second checkout is on the same page: the host once, then each repo,
    // in the order of their roots, each named in the row at the top
    let clone2 = world.second_repo();
    let second = world.afk_in(&clone2, &RUN).ok();
    assert_eq!(second["host_runner"]["panel"], address, "{second}");
    assert_eq!(second["host_runner"]["pid"], first["host_runner"]["pid"]);
    let page = page_with(port, "id=\"checkout-2\"");
    assert_eq!(checkouts(&page), [world.clone.display().to_string(), clone2.display().to_string()]);
    assert_eq!(page.html.matches("<header class=\"top host\">").count(), 1);
    assert!(page.html.find("class=\"human\"").unwrap() < page.html.find("<nav").unwrap());
    let row = page.part("nav", "checkouts");
    assert!(row.contains("<a href=\"#checkout-1\">"), "{row}");
    assert!(row.contains("<b>at work</b> · holding 1 · frontier 0"), "{row}");
    assert!(row.contains("<a href=\"#checkout-2\">"), "{row}");
    assert!(row.contains("<b>idle</b> · holding 0"), "{row}");

    // an open page is told of every tick of any fleet instance, and the page
    // it then reads shows what that tick read
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
    let page = page_with(port, "data-issue=\"9\"");
    assert_eq!(carded(page.node("work"), "at-work"), [3, 9], "{}", page.html);

    // `panel --open` answers where, from anywhere, and starts nothing
    for dir in [&world.root, &clone2, &world.clone] {
        let answer = world.afk_in(dir, &["panel", "--open"]).ok();
        assert_eq!(answer["panel"], address, "{answer}");
        assert_eq!(answer["host_runner"]["pid"], first["host_runner"]["pid"], "{answer}");
    }
    let pid = first["host_runner"]["pid"].as_u64().map(|pid| pid as u32);
    assert_eq!(world.host_runner(), pid, "the host runner it asked runs on");
    // every key of the answer is one the help names, as is each way it fails
    let answer = world.afk(&["panel", "--open"]).ok();
    let help = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
    let help = { help }.args(["panel", "--help"]).output().unwrap().stdout;
    let help = String::from_utf8(help).unwrap();
    for key in answer.as_object().unwrap().keys() {
        assert!(help.contains(&format!("\n- `{key}`: ")), "`panel --help` never names {key:?}");
    }
    for said in ["no host runner runs on this host", "did not answer within 10 seconds"] {
        assert!(help.contains(said), "`panel --help` never says {said:?}");
    }
    assert!(help.contains("the host runner serves no panel"), "{help}");
}

#[test]
fn a_panel_port_that_is_taken_is_served_around_and_run_panel_sets_the_next_host_runner_s() {
    let world = World::new();
    let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    world.say_host_settings(json!({"panel": port}));
    let first = world.afk(&RUN).ok();
    // any free port will do, and the log says why it is that one
    let panel = first["host_runner"]["panel"].clone();
    assert_ne!(port_in(&panel), port, "{first}");
    let line = format!("panel: {}, port {port} being taken", panel.as_str().unwrap());
    assert!(world.runner_log().contains(&line), "{:#?}", world.runner_log());
    assert!(page_with(port_in(&panel), "<b>Tick 1</b>").html.contains("nothing to do"));
    assert_eq!(world.afk(&["panel", "--open"]).ok()["panel"], panel);

    // `run --panel` sets the host setting: the host runner that runs stays
    // where it is, and the next serves on the port set
    let other = free_port();
    let set = world.afk(&[&RUN[..], &["--panel", &other.to_string()]].concat()).ok();
    assert_eq!(set["settings"], json!({"panel": {"from": port, "to": other}}), "{set}");
    assert_eq!(set["host_runner"]["panel"], panel, "{set}");
    let address = format!("http://127.0.0.1:{other}/");
    for _ in 0..2 {
        // …and one started again takes it back: the one before let it go
        world.afk(&["stop", "--all"]).ok();
        let next = world.afk(&RUN).ok();
        assert_eq!(next["host_runner"]["panel"], address, "{next}");
        assert!(page_with(other, "<b>Tick 1</b>").html.contains("nothing to do"));
    }
}

#[test]
fn with_no_port_said_the_panel_s_port_is_7070() {
    // No host runner is started: 7070 is the panel's port of the host the
    // tests run on, which a test leaves alone (sunfmin/control#820).
    let world = World::new();
    let mut list = world.checkout_list();
    list["settings"].as_object_mut().unwrap().remove("panel");
    std::fs::write(world.cache().join("afkconvoy/checkouts.json"), list.to_string()).unwrap();
    let doctor = world.afk(&["doctor", "--host", "h1"]).json;
    assert_eq!(doctor["host"]["settings"]["in_force"]["panel"], 7070, "{doctor}");
    assert_eq!(doctor["host"]["settings"]["said"]["panel"], Value::Null, "{doctor}");
    assert_eq!(world.host_runner(), None, "{doctor}");
}

#[test]
fn panel_open_with_no_host_runner_says_so_and_starts_nothing() {
    let world = World::new();
    let error = world.afk(&["panel", "--open"]).error();
    assert!(error.starts_with("no host runner runs on this host, so no panel is served"));
    assert!(error.contains("`afkconvoy run`") && error.contains("`afkconvoy panel`"), "{error}");
    assert_eq!(world.host_runner(), None, "none was started");
    assert!(world.runner_log().is_empty(), "{:#?}", world.runner_log());
    // it takes no port, no checkout and no instance: the page is the host runner's
    let clone = world.clone.display().to_string();
    for refused in [&["--port", "7070"][..], &[&clone], &["--instance", "alpha"]] {
        let refused = world.afk(&[&["panel", "--open"][..], refused].concat());
        assert_eq!(refused.code, 2, "{refused:?}");
    }
    assert_eq!(world.host_runner(), None);
}

#[test]
fn a_fleet_instance_held_in_the_foreground_serves_the_panel_as_a_host_runner_does() {
    let world = World::new();
    world.issue(3, &["ready-for-agent"]);
    let running = world.running(&["--instance", "alpha", "--host", "h1"]);
    let first = running.line(SOON).expect("the first tick");
    assert!(first.starts_with("tick 1: dispatched #3"), "{first}");
    // said where on its socket alone: what it prints is its ticks, as ever
    let answer = world.afk_in(&world.root, &["panel", "--open"]).ok();
    assert_eq!(answer["host_runner"]["foreground"], true, "{answer}");
    let page = page_with(port_in(&answer["panel"]), "<b>Tick 1</b>");
    assert_eq!(checkouts(&page), [world.clone.display().to_string()]);
    assert_eq!(carded(page.node("work"), "at-work"), [3], "{}", page.html);
}

#[test]
fn the_exclusive_queue_its_host_runner_holds_is_on_the_page_printed_and_the_page_served() {
    let world = World::new();
    let running = world.running(&["--instance", "alpha", "--host", "h1"]);
    running.line(SOON).expect("the first tick");
    let exclusive = |script: &str| {
        let afkconvoy = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
        { afkconvoy }
            .env("CACHE", world.cache())
            .env("AFKCONVOY_ISSUE", "12")
            .args(["exclusive", "--", "sh", "-c", script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    };
    let printed_with = |said: &str| {
        let deadline = Instant::now() + SOON;
        let mut page = printed(&world, &world.clone, &[]);
        while !page.html.contains(said) {
            assert!(Instant::now() < deadline, "{said:?} was never printed: {}", page.html);
            std::thread::sleep(Duration::from_millis(50));
            page = printed(&world, &world.clone, &[]);
        }
        page
    };
    let alone = exclusive("until [ -e \"$CACHE/done\" ]; do sleep 0.05; done");
    printed_with("running, for ");
    let after = exclusive("true");

    // printed: `panel` asks the host runner on its socket
    let page = printed_with("waits behind 1");
    assert!(page.html.contains("<h2>Command lanes</h2>"), "{}", page.html);
    assert!(page.html.contains("running, for "), "{}", page.html);
    assert!(page.html.contains(" · #12 in "), "{}", page.html);

    // served: the page the next tick makes reads it where it is held
    world.afk(&["wake"]).ok();
    let answer = world.afk_in(&world.root, &["panel", "--open"]).ok();
    let page = page_with(port_in(&answer["panel"]), "waits behind 1");
    assert!(page.html.contains("<h2>Command lanes</h2>"), "{}", page.html);

    write(&world.cache(), "done", "");
    for command in [alone, after] {
        assert!(command.wait_with_output().unwrap().status.success());
    }
    let page = printed(&world, &world.clone, &[]);
    assert!(page.html.contains("1 worker lanes"), "the idle page keeps the counts: {}", page.html);
}

/// Has the remote lose the base branch the config names: a start of the
/// fleet instance of the first repo fails its startup probe.
fn base_branch_gone(world: &World) {
    let config = "```yaml\nbase_branch: release\ngate_command: ./gate.sh\n```\n";
    common::commit(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["push", "-q", "origin", "main"]);
}

#[test]
fn a_broken_fleet_instance_waits_with_a_human_on_the_page_served_and_the_page_printed() {
    let world = World::new();
    world.afk(&RUN).ok();
    let clone2 = world.second_repo();
    // An escalated issue of the second repo waits for an answer.
    let gh2 = world.root.join("gh2.json");
    let mut state: Value = serde_json::from_str(&std::fs::read_to_string(&gh2).unwrap()).unwrap();
    let parked = json!({"number": 4, "title": "Pick a name", "labels": ["ready-for-human"],
        "state": "open"});
    state["issues"].as_array_mut().unwrap().push(parked);
    state["comments"]["4"] = json!([{"id": 2000, "body": "<!--convoy:escalation claim=old-->\nPick a name.", "html_url": "https://gh/c/2000"}]);
    std::fs::write(&gh2, state.to_string()).unwrap();
    world.afk_in(&clone2, &RUN).ok();
    world.kill_host_runner("KILL");
    base_branch_gone(&world);
    // started again from the list, the first is broken; the second works on
    let other = world.afk_in(&clone2, &["run"]).ok();
    let page = page_with(port_in(&other["host_runner"]["panel"]), "data-broken=");
    let clone = world.clone.display().to_string();
    for page in [page, printed(&world, &world.root, &["--host", "h1"])] {
        let human = page.part("section", "human");
        let broken = human.split(&format!("data-broken=\"{clone}\"")).nth(1);
        let broken = broken.unwrap_or_else(|| panic!("{human}")).split("</li>").next().unwrap();
        assert!(broken.contains("its fleet instance is broken"), "{broken}");
        assert!(broken.contains("the remote has no branch"), "why: {broken}");
        assert!(broken.contains("· tried again in "), "{broken}");
        assert!(broken.contains(&format!("<code>afkconvoy run {clone}</code>")), "{broken}");
        // each issue that waits for a human, in every repo, is labelled with it
        assert_eq!(carded(human, "asked"), [4], "{human}");
        assert!(human.contains("<span class=\"repo\">"), "{human}");
        let row = page.part("nav", "checkouts");
        assert!(row.contains("<b class=\"broken\">broken</b>"), "{row}");
        assert_eq!(checkouts(&page), [clone.clone(), clone2.display().to_string()]);
    }

    // a checkout named is printed alone
    let alone = printed(&world, &world.root, &["--host", "h1", clone2.to_str().unwrap()]);
    assert_eq!(checkouts(&alone), [clone2.display().to_string()]);
    assert!(!alone.html.contains("data-broken="), "{}", alone.html);
}
