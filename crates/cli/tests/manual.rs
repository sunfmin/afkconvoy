//! The manual: `docs/manual.md` is the help topics as one page, written by an
//! example of this package and by nothing else. The gate runs its check.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use afkconvoy::topic::TOPICS;

const COMMAND: &str = "cargo run -p afkconvoy --example manual";

/// The example, run on `file` in place of the repository's page.
fn manual(args: &[&str], file: &Path) -> Output {
    let built = Path::new(env!("CARGO_BIN_EXE_afkconvoy")).parent().unwrap().join("examples");
    Command::new(built.join("manual")).args(args).arg(file).output().unwrap()
}

/// A page it wrote, and the directory that holds it.
fn written() -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("manual.md");
    assert!(manual(&[], &file).status.success());
    let page = std::fs::read_to_string(&file).unwrap();
    (dir, file, page)
}

#[test]
fn the_page_says_at_its_top_that_it_is_generated_and_from_what() {
    let (_dir, _, page) = written();
    let top: Vec<_> = page.lines().take(3).collect();
    assert_eq!(top[0], "# afkconvoy manual");
    for said in ["generated", COMMAND, "help topics", "crates/cli/src/topic.rs", "Do not edit"] {
        assert!(top[2].contains(said), "the top of the page lost {said:?}: {}", top[2]);
    }
}

#[test]
fn the_page_holds_every_topic_whole_in_the_order_the_help_lists_them() {
    let (_dir, _, page) = written();
    let (contents, topics) = page.split_once("\n## afkconvoy help ").expect("a first topic");
    let entries: Vec<_> = contents.lines().filter(|line| line.starts_with("- [")).collect();
    let heads: Vec<_> = page.lines().filter_map(|line| line.strip_prefix("## ")).collect();
    assert_eq!(heads.len(), TOPICS.len() + 1, "{heads:?}");
    for (at, topic) in TOPICS.iter().enumerate() {
        let name = topic.name;
        let entry = format!("- [`afkconvoy help {name}`](#afkconvoy-help-{name}) — {}", topic.line);
        assert_eq!(entries[at], entry);
        assert_eq!(heads[at + 1], format!("afkconvoy help {name}"));
        // Every line of its text but its title, whatever the page made of its margin.
        for line in topic.text.lines().skip(1).filter(|line| !line.trim().is_empty()) {
            let line = line.trim().trim_start_matches('#').trim_start();
            assert!(topics.contains(line), "the page lost a line of {name}: {line:?}");
        }
    }
    assert!(page.ends_with('\n') && !page.ends_with("\n\n"));
}

#[test]
fn a_plain_topics_headings_are_headings_and_its_text_keeps_its_columns() {
    let (_dir, _, page) = written();
    assert!(
        page.contains("\n### THE FOUR THINGS\n\n````text\nThe fleet          A fleet instance")
    );
    // A heading two lines long is one heading.
    assert!(page.contains("\n### WHAT `doctor` FINDS — warning: one issue or one patrol is held up. A warning may be the fleet mid-step"));
    // The glossary is Markdown already: its headings go under the page's.
    assert!(page.contains("\n## afkconvoy help glossary\n\nA command-line tool"));
    assert!(page.contains("\n### Language\n") && page.contains("\n#### The fleet\n"));
}

#[test]
fn a_page_that_is_not_what_the_topics_generate_fails_the_check_with_the_command() {
    let (dir, file, page) = written();
    assert!(manual(&["--check"], &file).status.success());
    std::fs::write(&file, page.replacen("A claim ", "A lease ", 1)).unwrap();
    let out = manual(&["--check"], &file);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        said.contains(&format!("run `{COMMAND}`")) && said.contains("differs at line"),
        "{said}"
    );
    // The check wrote nothing, and a page that is not there fails it too.
    assert!(std::fs::read_to_string(&file).unwrap().contains("A lease "));
    let missing = manual(&["--check"], &dir.path().join("none.md"));
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains(COMMAND));
}

/// What makes a topic edited and a page not written again fail the gate.
#[test]
fn the_gate_checks_the_repositorys_page() {
    let gate = include_str!("../../../gate.sh");
    assert!(gate.contains(&format!("{} -- --check\n", COMMAND.replace("run", "run --quiet"))));
}
