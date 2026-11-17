//! A host's own daybook (docs/adr/0118) as another program uses it: through
//! the crate's public API alone, at a path of its choosing. Entries are
//! written and read back by day and span, a span's lessons and what it holds
//! are read from its entries, the same `Store` a GitHub daybook answers, and
//! two writers of one file at once — each with its own connection, as two
//! programs — lose nothing.

use std::sync::Barrier;
use std::thread;

use afkconvoy_daybook::{Entry, Held, Lesson, Local, Said, Store, entry};

fn did(what: &str) -> Said {
    Said { did: Some(what.to_string()), ..Said::default() }
}

fn taught(what: &str, lesson: &str) -> Said {
    Said { lesson: Some(lesson.to_string()), ..did(what) }
}

#[test]
fn an_entry_is_written_and_read_back_by_its_day_and_its_span() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not/yet/made/daybook.db");
    let book = Local::open(&path).unwrap();
    let first = entry("acme/dev", "agent", &did("installed jq"));
    let url = book.write("2026-11-13", 1_000, &first).unwrap();
    assert_eq!(url, "daybook:1");
    let second =
        book.write("2026-11-13", 1_001, &entry("acme/dev", "agent", &did("ruled"))).unwrap();
    let next = book.write("2026-11-14", 2_000, &entry("acme/dev", "agent", &did("next"))).unwrap();
    assert_ne!(second, url);

    let day = book.entries("2026-11-13", "2026-11-13").unwrap();
    assert_eq!(day.len(), 2, "{day:?}");
    assert_eq!(
        day[0],
        Entry { url: url.clone(), day: "2026-11-13".into(), at: 1_000, text: first.clone() }
    );
    assert_eq!(day[1].url, second);
    let span: Vec<String> =
        book.entries("2026-11-12", "2026-11-14").unwrap().into_iter().map(|e| e.url).collect();
    assert_eq!(span, [url, second, next]);
    assert!(book.entries("2026-11-15", "2026-11-30").unwrap().is_empty());

    // Opened again, as by another program: the same entries, the schema
    // converged once.
    drop(book);
    let again = Local::open(&path).unwrap();
    assert_eq!(again.entries("2026-11-13", "2026-11-14").unwrap().len(), 3);
}

#[test]
fn a_span_s_lessons_and_what_it_holds_are_read_from_its_entries() {
    let dir = tempfile::tempdir().unwrap();
    let book = Local::open(&dir.path().join("daybook.db")).unwrap();
    book.write("2026-11-12", 1, &entry("acme/dev", "agent", &did("looked"))).unwrap();
    let lesson = taught("installed jq", "the gate\n needs jq");
    let url = book.write("2026-11-13", 2, &entry("acme/dev", "agent", &lesson)).unwrap();
    book.write("2026-11-13", 3, &entry("acme/dev", "agent", &did("again"))).unwrap();

    let want = Lesson { day: "2026-11-13".into(), url, lesson: "the gate needs jq".into() };
    assert_eq!(book.lessons("2026-11-12", None).unwrap(), vec![want.clone()]);
    assert_eq!(book.lessons("2026-11-13", Some("2026-11-13")).unwrap(), vec![want]);
    assert_eq!(book.lessons("2026-11-12", Some("2026-11-12")).unwrap(), vec![]);
    assert_eq!(book.held("2026-11-12", "2026-11-13").unwrap(), Held { lessons: 1, unlessoned: 2 });
    assert_eq!(book.held("2026-11-14", "2026-11-20").unwrap(), Held { lessons: 0, unlessoned: 0 });
}

#[test]
fn a_day_said_wrongly_is_refused_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let book = Local::open(&dir.path().join("daybook.db")).unwrap();
    let error = book.write("yesterday", 1, "x").unwrap_err();
    assert!(format!("{error:#}").contains("YYYY-MM-DD"), "{error:#}");
    assert!(book.entries("0000-01-01", "9999-12-31").unwrap().is_empty());
}

/// The same `Store` as a GitHub daybook: a program that writes through the
/// interface writes either.
#[test]
fn it_is_a_store() {
    fn write_one(store: &dyn Store) -> String {
        store
            .write("2026-11-13", 7, &entry("acme/dev", "agent", &did("through the store")))
            .unwrap()
    }
    let dir = tempfile::tempdir().unwrap();
    let book = Local::open(&dir.path().join("daybook.db")).unwrap();
    let url = write_one(&book);
    assert_eq!(book.entries("2026-11-13", "2026-11-13").unwrap()[0].url, url);
}

#[test]
fn two_writers_of_one_file_at_once_lose_nothing() {
    const EACH: usize = 100;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daybook.db");
    let together = Barrier::new(2);
    let written: Vec<Vec<String>> = thread::scope(|scope| {
        let writers: Vec<_> = ["one", "two"]
            .map(|who| {
                let (path, together) = (&path, &together);
                scope.spawn(move || {
                    // Each opens the file not yet made, at once: both
                    // converge its schema, and neither waits for the other.
                    together.wait();
                    let book = Local::open(path).unwrap();
                    (0..EACH)
                        .map(|n| {
                            let said = did(&format!("{who} {n}"));
                            book.write("2026-11-13", n as u64, &entry("acme/dev", who, &said))
                                .unwrap()
                        })
                        .collect()
                })
            })
            .into_iter()
            .collect();
        writers.into_iter().map(|writer| writer.join().unwrap()).collect()
    });
    let held = Local::open(&path).unwrap().entries("2026-11-13", "2026-11-13").unwrap();
    assert_eq!(held.len(), 2 * EACH);
    let mut urls: Vec<String> = written.concat();
    urls.sort();
    urls.dedup();
    assert_eq!(urls.len(), 2 * EACH, "each entry its own");
    for who in ["one", "two"] {
        let mine: Vec<&Entry> =
            held.iter().filter(|e| e.text.contains(&format!("Session** {who}"))).collect();
        assert_eq!(mine.len(), EACH, "{who}");
        let order: Vec<u64> = mine.iter().map(|e| e.at).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "a writer's entries in its order");
    }
}
