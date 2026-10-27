//! The manual: `docs/manual.md` is what `afkconvoy help all` prints, written by
//! an example of this package and by nothing else. The gate runs its check.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    let top = page.lines().next().unwrap();
    assert!(top.starts_with("<!-- ") && top.ends_with(" -->"), "{top}");
    for said in ["Generated", COMMAND, "`afkconvoy help all`", "Do not edit"] {
        assert!(top.contains(said), "the top of the page lost {said:?}: {top}");
    }
}

#[test]
fn the_page_is_the_manual_that_help_prints() {
    let (_dir, _, page) = written();
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(["help", "all"]).output();
    let out = out.unwrap();
    assert!(out.status.success());
    let manual = String::from_utf8(out.stdout).unwrap();
    let (top, rest) = page.split_once("\n\n").unwrap();
    assert!(top.starts_with("<!-- "));
    assert_eq!(rest, manual);
    assert!(manual.starts_with("# `afkconvoy`\n") && manual.contains("\n## Topics\n"));
    assert!(page.ends_with('\n') && !page.ends_with("\n\n"));
}

#[test]
fn a_page_that_is_not_what_the_topics_generate_fails_the_check_with_the_command() {
    let (dir, file, page) = written();
    assert!(manual(&["--check"], &file).status.success());
    let stale = format!("{page}\nStale content.\n");
    std::fs::write(&file, &stale).unwrap();
    let out = manual(&["--check"], &file);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        said.contains(&format!("run `{COMMAND}`")) && said.contains("differs at line"),
        "{said}"
    );
    // The check wrote nothing, and a page that is not there fails it too.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), stale);
    let missing = manual(&["--check"], &dir.path().join("none.md"));
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains(COMMAND));
}
