//! `afkconvoy daybook`: an entry appended to the day's discussion of the
//! tracker's `Daybook` category.

use serde_json::{Value, json};

use crate::common::World;

const NAMED: [&str; 4] = ["--repo", "acme/dev", "--tracker", "acme/control"];

fn with_category(world: &World) {
    world.with_gh(|gh| gh["discussion_categories"] = json!(["General", "Daybook"]));
}

fn daybook(world: &World, args: &[&str]) -> Value {
    let mut all = vec!["daybook"];
    all.extend(args);
    all.extend(NAMED);
    world.afk(&all).ok()
}

fn discussions(world: &World) -> Vec<Value> {
    world.gh()["discussions"].as_array().cloned().unwrap_or_default()
}

fn bodies(discussion: &Value) -> Vec<String> {
    discussion["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|comment| comment["body"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_first_entry_of_a_day_opens_its_discussion_and_the_next_is_added_to_it() {
    let world = World::new();
    with_category(&world);
    let first = daybook(&world, &["installed jq on the host"]);
    assert_eq!(first["created"], true, "{first}");
    let day = first["day"].as_str().unwrap().to_string();
    let second = daybook(
        &world,
        &[
            "--did",
            "ruled on #12",
            "--why",
            "asked in the asking session",
            "--link",
            "https://x/12",
        ],
    );
    assert_eq!(second["created"], false, "{second}");
    assert_eq!(second["day"], day.as_str());
    assert_eq!(second["discussion"], first["discussion"]);
    assert_ne!(second["url"], first["url"]);
    assert!(second["url"].as_str().unwrap().starts_with(first["discussion"].as_str().unwrap()));

    let found = discussions(&world);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["title"], day.as_str());
    assert_eq!(found[0]["category"], "Daybook");
    let bodies = bodies(&found[0]);
    assert_eq!(
        bodies[0],
        "**Repo** `acme/dev` · **Session** interactive\n\n### Did\n\ninstalled jq on the host\n"
    );
    assert!(
        bodies[1].contains("### Did\n\nruled on #12\n\n### Why\n\nasked in the asking session\n")
    );
    assert!(bodies[1].ends_with("### Links\n\n- https://x/12\n"), "{}", bodies[1]);
    assert!(!bodies[1].contains("Undo"), "an omitted heading is left out: {}", bodies[1]);
}

#[test]
fn two_first_entries_of_a_day_at_once_leave_one_discussion_holding_both() {
    let world = World::new();
    with_category(&world);
    let race = world.root.join("daybook-race");
    let program = env!("CARGO_BIN_EXE_afkconvoy");
    let runs: Vec<_> = ["first", "second"]
        .iter()
        .map(|text| {
            world
                .command(program, &world.clone)
                .args(["daybook", text])
                .args(NAMED)
                .env("CONVOY_FAKE_GH_RACE", format!("{}:2", race.display()))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let answers: Vec<Value> = runs
        .into_iter()
        .map(|run| {
            let out = run.wait_with_output().unwrap();
            let said = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(out.status.success(), "{said} {}", String::from_utf8_lossy(&out.stderr));
            serde_json::from_str(said.trim()).unwrap()
        })
        .collect();
    // Both found none and created one: the race was run, not dodged.
    assert_eq!(world.gh()["discussions_made"], 2);
    let found = discussions(&world);
    assert_eq!(found.len(), 1, "{found:?}");
    let mut texts = bodies(&found[0]);
    texts.sort();
    assert!(texts[0].ends_with("### Did\n\nfirst\n") && texts[1].ends_with("### Did\n\nsecond\n"));
    for answer in &answers {
        assert_eq!(answer["discussion"], found[0]["url"], "{answer}");
    }
    let created: Vec<&Value> = answers.iter().map(|answer| &answer["created"]).collect();
    assert!(created.contains(&&json!(true)) && created.contains(&&json!(false)), "{created:?}");
}

#[test]
fn a_tracker_with_no_daybook_category_is_told_how_to_make_one_and_nothing_is_written() {
    let world = World::new();
    world.with_gh(|gh| gh["discussion_categories"] = json!(["General"]));
    let error = world.afk(&[&["daybook", "x"][..], &NAMED].concat()).error();
    assert!(
        error.contains("https://github.com/acme/control/discussions/categories/new"),
        "{error}"
    );
    assert!(error.contains("`Daybook`") && error.contains("nothing was written"), "{error}");
    world.with_gh(|gh| gh["discussions_enabled"] = json!(false));
    let error = world.afk(&[&["daybook", "x"][..], &NAMED].concat()).error();
    assert!(error.contains("gh repo edit acme/control --enable-discussions"), "{error}");
    assert!(discussions(&world).is_empty());
    let wrote = world.gh_calls().into_iter().any(|call| call.join(" ").contains("mutation"));
    assert!(!wrote, "{:?}", world.gh_calls());
}

#[test]
fn an_entry_names_the_kind_of_session_that_wrote_it() {
    let world = World::new();
    with_category(&world);
    let program = env!("CARGO_BIN_EXE_afkconvoy");
    for (name, value, kind) in [
        ("AFKCONVOY_WATCH", "host", "watch"),
        ("AFKCONVOY_ASKING", "1", "asking session"),
        ("AFKCONVOY_ISSUE", "7", "worker"),
    ] {
        let out = world
            .command(program, &world.clone)
            .args(["daybook", "x"])
            .args(NAMED)
            .env(name, value)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let last = bodies(&discussions(&world)[0]).pop().unwrap();
        assert!(last.starts_with(&format!("**Repo** `acme/dev` · **Session** {kind}\n")), "{last}");
    }
    let error = world.afk(&[&["daybook"][..], &NAMED].concat()).error();
    assert!(error.contains("says nothing"), "{error}");
}
