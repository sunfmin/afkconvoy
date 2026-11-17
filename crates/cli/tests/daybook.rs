//! `afkconvoy daybook`: an entry appended to the host's own daybook,
//! `daybook.db` in afkconvoy's data directory (docs/adr/0118), read back by
//! day and span, its lessons listed and distilled — and GitHub asked
//! nothing of it.

use serde_json::{Value, json};

use crate::common::World;

/// 2026-11-13 12:00 UTC: a day in the host's local time whatever its zone
/// but the farthest.
const NOON: u64 = 1_794_571_200;

fn daybook(world: &World, args: &[&str]) -> Value {
    let mut all = vec!["daybook"];
    all.extend(args);
    world.afk(&all).ok()
}

/// Writes an entry of `acme/dev` at the epoch second `at`.
fn write(world: &World, at: u64, args: &[&str]) -> Value {
    let at = at.to_string();
    let mut all = args.to_vec();
    all.extend(["--repo", "acme/dev", "--now", &at]);
    daybook(world, &all)
}

fn read(world: &World, args: &[&str]) -> Vec<Value> {
    let mut all = vec!["--read"];
    all.extend(args);
    daybook(world, &all)["entries"].as_array().cloned().unwrap()
}

fn texts(entries: &[Value]) -> Vec<String> {
    entries.iter().map(|entry| entry["text"].as_str().unwrap().to_string()).collect()
}

/// No call to `gh` was about the daybook: none asked for a discussion.
fn asked_github_nothing(world: &World) {
    let calls: Vec<String> = world.gh_calls().iter().map(|call| call.join(" ")).collect();
    assert!(!calls.iter().any(|call| call.contains("iscussion")), "{calls:?}");
}

#[test]
fn an_entry_is_written_to_the_host_s_database_and_read_back_by_its_day_and_its_span() {
    let world = World::new();
    let first = write(&world, NOON, &["installed jq on the host"]);
    let day = afkconvoy::daybook::day_at(NOON);
    assert_eq!(first["day"], day.as_str(), "{first}");
    assert_eq!(first["at"], NOON);
    assert!(first["url"].as_str().unwrap().starts_with("daybook:"), "{first}");
    assert!(world.data().join("afkconvoy/daybook.db").is_file());
    let second = write(
        &world,
        NOON + 60,
        &[
            "--did",
            "ruled on #12",
            "--why",
            "asked in the asking session",
            "--link",
            "https://x/12",
        ],
    );
    assert_ne!(second["url"], first["url"]);
    let next = write(&world, NOON + 24 * 60 * 60, &["the next day"]);
    let next_day = next["day"].as_str().unwrap();
    assert_ne!(next_day, day);

    let of_day = read(&world, &["--day", &day]);
    assert_eq!(of_day.len(), 2, "{of_day:?}");
    assert_eq!(
        of_day[0],
        json!({"url": first["url"], "day": day, "at": NOON, "lesson": null,
            "text": "**Repo** `acme/dev` · **Session** interactive\n\n### Did\n\ninstalled jq on the host\n"})
    );
    let said = &texts(&of_day)[1];
    assert!(said.contains("### Did\n\nruled on #12\n\n### Why\n\nasked in the asking session\n"));
    assert!(said.ends_with("### Links\n\n- https://x/12\n"), "{said}");
    assert!(!said.contains("Undo"), "an omitted heading is left out: {said}");

    let span = read(&world, &["--since", &day, "--until", next_day]);
    let urls: Vec<&Value> = span.iter().map(|entry| &entry["url"]).collect();
    assert_eq!(urls, [&first["url"], &second["url"], &next["url"]]);
    assert_eq!(
        read(&world, &["--since", "2000-01-01", "--until", "2000-01-02"]),
        Vec::<Value>::new()
    );
    let today = daybook(&world, &["--read", "--now", &(NOON + 24 * 60 * 60).to_string()]);
    assert_eq!(today["since"], next_day);
    assert_eq!(texts(today["entries"].as_array().unwrap()).len(), 1, "{today}");
    asked_github_nothing(&world);
}

#[test]
fn an_entry_names_the_kind_of_session_that_wrote_it() {
    let world = World::new();
    let program = env!("CARGO_BIN_EXE_afkconvoy");
    for (name, value, kind) in [
        ("AFKCONVOY_WATCH", "host", "watch"),
        ("AFKCONVOY_ASKING", "1", "asking session"),
        ("AFKCONVOY_ISSUE", "7", "worker"),
    ] {
        let out = world
            .command(program, &world.clone)
            .args(["daybook", "x", "--repo", "acme/dev", "--now", &NOON.to_string()])
            .env(name, value)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let day = afkconvoy::daybook::day_at(NOON);
        let last = texts(&read(&world, &["--day", &day])).pop().unwrap();
        assert!(last.starts_with(&format!("**Repo** `acme/dev` · **Session** {kind}\n")), "{last}");
    }
    let error = world.afk(&["daybook", "--repo", "acme/dev"]).error();
    assert!(error.contains("says nothing"), "{error}");
}

#[test]
fn a_lesson_is_a_heading_of_its_entry_and_listed_from_the_span_s_entries() {
    let world = World::new();
    let day = afkconvoy::daybook::day_at(NOON);
    write(&world, NOON, &["installed jq"]);
    let taught = write(
        &world,
        NOON + 1,
        &["--did", "fixed the gate", "--lesson", "the gate on this host\nneeds jq"],
    );
    let entries = read(&world, &["--day", &day]);
    assert!(!texts(&entries)[0].contains("Lesson"));
    assert!(
        texts(&entries)[1].ends_with(
            "### Did\n\nfixed the gate\n\n### Lesson\n\nthe gate on this host needs jq\n"
        ),
        "{entries:?}"
    );
    assert_eq!(entries[1]["lesson"], "the gate on this host needs jq");
    // An older day's lesson, outside and then inside the span.
    let old = write(&world, NOON - 30 * 24 * 60 * 60, &["--did", "old", "--lesson", "old"]);
    let old_day = old["day"].as_str().unwrap();
    let listed = daybook(&world, &["--lessons", "--since", &day]);
    assert_eq!(
        listed["lessons"],
        json!([{"day": day, "url": taught["url"], "lesson": "the gate on this host needs jq"}])
    );
    let listed = daybook(&world, &["--lessons", "--since", old_day, "--until", old_day]);
    assert_eq!(listed["lessons"], json!([{"day": old_day, "url": old["url"], "lesson": "old"}]));
    asked_github_nothing(&world);
}

#[test]
fn reading_needs_days_said_as_dates_and_writes_nothing() {
    let world = World::new();
    let error = world.afk(&["daybook", "--lessons"]).error();
    assert!(error.contains("--since"), "{error}");
    let error = world.afk(&["daybook", "--lessons", "--since", "yesterday"]).error();
    assert!(error.contains("YYYY-MM-DD"), "{error}");
    let error = world.afk(&["daybook", "x", "--lessons", "--since", "2026-01-01"]).error();
    assert!(error.contains("cannot be used with"), "{error}");
    let error = world.afk(&["daybook", "x", "--read"]).error();
    assert!(error.contains("cannot be used with"), "{error}");
    let error =
        world.afk(&["daybook", "--read", "--day", "2026-01-01", "--since", "2026-01-01"]).error();
    assert!(error.contains("cannot be used with"), "{error}");
    let error = world.afk(&["daybook", "--since", "2026-01-01"]).error();
    assert!(error.contains("--lessons") || error.contains("--read"), "{error}");
    let error = world.afk(&["daybook", "--read", "--lessons", "--since", "2026-01-01"]).error();
    assert!(error.contains("cannot be used with"), "{error}");
    let listed = world.afk(&["daybook", "--lessons", "--since", "2026-01-01"]).ok();
    assert_eq!(listed["lessons"], json!([]));
    assert_eq!(read(&world, &["--since", "2000-01-01"]), Vec::<Value>::new());
}

#[test]
fn a_distillation_carries_each_lesson_of_its_span_once_and_writes_one_entry_mapping_them() {
    let world = World::new();
    let day = afkconvoy::daybook::day_at(NOON);
    let url = |written: Value| written["url"].as_str().unwrap().to_string();
    let one = url(write(&world, NOON, &["--did", "a", "--lesson", "the gate needs jq"]));
    let two = url(write(&world, NOON + 1, &["--did", "b", "--lesson", "orca forgets trust"]));
    let three = url(write(&world, NOON + 2, &["the same fault"]));
    let four = url(write(&world, NOON + 3, &["the same fault again"]));
    let file = world.root.join("distilled.json");
    let distil = |carried: Value| {
        std::fs::write(&file, json!({"since": day, "until": day, "carried": carried}).to_string())
            .unwrap();
        world.afk(&[
            "daybook",
            "--distilled",
            file.to_str().unwrap(),
            "--repo",
            "acme/dev",
            "--now",
            &(NOON + 10).to_string(),
        ])
    };
    let ticket = json!({"entries": [one], "home": "ticket", "link": "https://gh/issues/9"});
    let error = distil(json!([ticket])).error();
    assert!(error.contains("carried nowhere") && error.contains("orca forgets trust"), "{error}");
    let error = distil(json!([ticket, ticket, {"entries": [two], "home": "covered",
        "link": "https://gh/x/GLOSSARY.md"}]))
    .error();
    assert!(error.contains("carried 2 times"), "{error}");
    let lone = json!({"entries": [three], "lesson": "twice", "home": "ticket",
        "link": "https://gh/issues/10"});
    let covered =
        json!({"entries": [two], "home": "covered", "link": "https://gh/x/blob/main/GLOSSARY.md"});
    let error = distil(json!([ticket, covered, lone])).error();
    assert!(error.contains("two entries"), "{error}");
    let stranger = json!({"entries": [three, "daybook:999"], "lesson": "twice",
        "home": "ticket", "link": "https://gh/issues/10"});
    let error = distil(json!([ticket, covered, stranger])).error();
    assert!(error.contains("daybook:999 is no entry"), "{error}");
    let error = distil(json!([ticket, covered, {"entries": [one], "home": "memory",
        "link": "https://gh/y"}]))
    .error();
    assert!(error.contains("memory"), "{error}");
    assert_eq!(read(&world, &["--day", &day]).len(), 4, "a refused distillation writes nothing");

    let pattern = json!({"entries": [three, four], "lesson": "the same fault, twice",
        "home": "draft", "link": "https://gh/dotfiles/pull/3"});
    let done = distil(json!([ticket, covered, pattern])).ok();
    assert_eq!((done["lessons"].clone(), done["carried"].clone()), (json!(2), json!(3)), "{done}");
    let entries = texts(&read(&world, &["--day", &day]));
    assert_eq!(entries.len(), 5, "one entry for the run: {entries:?}");
    for said in [
        format!("### Did\n\nDistilled the daybook of {day} to {day}"),
        format!("- the gate needs jq (`{one}`) → ticket: https://gh/issues/9"),
        format!(
            "- orca forgets trust (`{two}`) → already covered: https://gh/x/blob/main/GLOSSARY.md"
        ),
        "→ draft PR, for the operator to read: https://gh/dotfiles/pull/3".to_string(),
    ] {
        assert!(entries[4].contains(&said), "{said}\n{}", entries[4]);
    }
    asked_github_nothing(&world);
}

#[test]
fn the_operator_asks_the_watch_for_a_distillation_outside_its_week() {
    let world = World::new();
    let asked = world.afk(&["daybook", "--distill"]).ok();
    assert_eq!(asked["asked"], true, "{asked}");
    let state: Value = world.kept("watch/state.json");
    assert_eq!(state["distilling"]["asked"], asked["at"], "{state}");
    let error = world.afk(&["daybook", "--distill", "--repo", "acme/control"]).error();
    assert!(error.contains("cannot be used with"), "{error}");
}

#[test]
fn the_help_says_the_daybook_is_the_host_s_and_names_no_discussion_category() {
    let world = World::new();
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["daybook", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("daybook.db") && help.contains("--read"), "{help}");
    assert!(!help.contains("Discussions") && !help.contains("category"), "{help}");
}
