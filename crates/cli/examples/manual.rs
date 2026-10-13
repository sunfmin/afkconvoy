//! Writes the manual, `docs/manual.md`: the help topics as one Markdown page,
//! in the order `--help` lists them, under a table of contents. The topics are
//! the one place afkconvoy is explained, and the page is nothing but them: it
//! is never edited, and the gate fails while it is not what this writes.
//!
//!   cargo run -p afkconvoy --example manual               write the page
//!   cargo run -p afkconvoy --example manual -- --check    fail unless the page is that
//!
//! A path after it names another file than the repository's.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use afkconvoy::topic::{TOPICS, Topic};

/// What writes the page: the line the page and a failed check both give.
const COMMAND: &str = "cargo run -p afkconvoy --example manual";
/// The page, from this package.
const PAGE: &str = "../../docs/manual.md";
/// What a topic's text is fenced with: longer than any fence a topic names.
const FENCE: &str = "````";

fn main() -> ExitCode {
    let mut check = false;
    let mut file = Path::new(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--check" => check = true,
            path => file = PathBuf::from(path),
        }
    }
    let page = page();
    if !check {
        return match std::fs::write(&file, page) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&format!("{} cannot be written: {error}", file.display())),
        };
    }
    match std::fs::read_to_string(&file) {
        Ok(written) if written == page => ExitCode::SUCCESS,
        Ok(written) => {
            let same = written.lines().zip(page.lines()).take_while(|(a, b)| a == b).count();
            fail(&format!(
                "{} is not what the help topics generate: it differs at line {}. The page is \
                 never edited: run `{COMMAND}` and commit what it wrote.",
                file.display(),
                same + 1
            ))
        }
        Err(error) => {
            fail(&format!("{} cannot be read ({error}): run `{COMMAND}`.", file.display()))
        }
    }
}

fn fail(why: &str) -> ExitCode {
    eprintln!("manual: {why}");
    ExitCode::FAILURE
}

/// The heading a topic is under, and what the contents link to.
fn heading(topic: &Topic) -> String {
    format!("afkconvoy help {}", topic.name)
}

/// The whole page.
fn page() -> String {
    let mut page = format!(
        "# afkconvoy manual\n\n\
         > This page is generated, by `{COMMAND}`, from afkconvoy's help topics: the texts \
         `crates/cli/src/topic.rs` lists, each as `afkconvoy help <topic>` prints it, in the \
         order `afkconvoy --help` lists them. Do not edit it: edit a topic, and run that \
         command. `afkconvoy --help` is the way in, and `afkconvoy <command> --help` says \
         what one command reads, writes and answers.\n\n\
         ## Contents\n\n"
    );
    for topic in TOPICS {
        let heading = heading(topic);
        let anchor = heading.replace(' ', "-");
        writeln!(page, "- [`{heading}`](#{anchor}) — {}", topic.line).unwrap();
    }
    for topic in TOPICS {
        write!(page, "\n## {}\n\n", heading(topic)).unwrap();
        // The glossary is Markdown already; every other topic is plain text.
        let body =
            if topic.text.starts_with("# ") { markdown(topic.text) } else { plain(topic.text) };
        page.push_str(&body);
    }
    page
}

/// A topic that is Markdown, under the heading the page gave it: without its
/// own title, and each heading of it one level down.
fn markdown(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines().skip(1).skip_while(|line| line.is_empty()) {
        fenced ^= line.starts_with("```");
        if !fenced && line.starts_with('#') {
            out.push('#');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A topic that is plain text. A line that starts at the margin is a heading
/// — the first is the topic's title, which the page's own heading stands for —
/// and what is under one is kept as it is written, fenced: its columns mean
/// something.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut title = true;
    let mut head: Vec<&str> = vec![];
    let mut body: Vec<&str> = vec![];
    for line in text.lines() {
        if line.is_empty() || line.starts_with(' ') {
            if !head.is_empty() && !title {
                write!(out, "### {}\n\n", head.join(" ")).unwrap();
            }
            if !head.is_empty() {
                (title, head) = (false, vec![]);
            }
            body.push(line);
        } else {
            out.push_str(&fenced(&body));
            body.clear();
            head.push(line);
        }
    }
    out + &fenced(&body)
}

/// The lines under one heading as a block of text, without the blank lines
/// around them and the margin they share.
fn fenced(lines: &[&str]) -> String {
    let written = |line: &&str| !line.is_empty();
    let (Some(first), Some(last)) =
        (lines.iter().position(written), lines.iter().rposition(written))
    else {
        return String::new();
    };
    let lines = &lines[first..=last];
    let written = lines.iter().filter(|line| !line.is_empty());
    let margin = written.map(|line| line.len() - line.trim_start().len()).min().unwrap_or(0);
    let mut out = format!("{FENCE}text\n");
    for line in lines {
        out.push_str(line.get(margin..).unwrap_or(""));
        out.push('\n');
    }
    out + FENCE + "\n\n"
}
