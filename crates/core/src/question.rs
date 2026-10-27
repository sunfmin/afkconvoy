//! A question put to the login shell a worker gets — `$SHELL -lic`, with no
//! terminal opened — and its answer as read back from that shell's stdout
//! and stderr together. The question is typed between two marks: what it
//! printed is the lines between them, and the second says how it exited.
//! Whatever the shell prints before the first — its rc files, its prompt —
//! is no part of the answer. Raw output between the marks is what a harness
//! parses; a terminal's line wrapping is not required.

/// The mark an answer starts after, on a line of its own.
const ASKED: &str = "AFKCONVOY_ASKED";

/// The mark an answer ends at, with the question's exit status after it.
const ANSWERED: &str = "AFKCONVOY_ANSWERED";

/// What is typed before a question. Each mark is printed as two words run
/// together, so the line typed, which a terminal may show, holds neither.
const BEFORE: &str = "printf '%s%s\\n' AFKCONVOY_ ASKED; ( ";

/// What is typed after a question: the line break before the end mark puts
/// it on a line of its own, whatever the question printed last.
const AFTER: &str = " ); printf '\\n%s%s %s\\n' AFKCONVOY_ ANSWERED \"$?\"";

/// The line typed into a terminal to ask `script`.
pub fn typed(script: &str) -> String {
    format!("{BEFORE}{script}{AFTER}")
}

/// The question a line `typed` asks; `None` of any other line.
pub fn script(typed: &str) -> Option<&str> {
    typed.strip_prefix(BEFORE)?.strip_suffix(AFTER)
}

/// The answer in `lines`, what a question's terminal shows, line by line:
/// whether the question exited zero, and what it printed. `None` while it has
/// not answered.
pub fn answer(lines: &[String]) -> Option<(bool, String)> {
    let mut after = lines.iter().map(|line| line.trim_end()).skip_while(|line| *line != ASKED);
    after.next()?;
    let mut printed: Vec<&str> = vec![];
    for line in after {
        if let Some(status) = line.strip_prefix(ANSWERED) {
            // The line break typed before the mark.
            if printed.last() == Some(&"") {
                printed.pop();
            }
            return Some((status.trim() == "0", printed.join("\n")));
        }
        printed.push(line);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_question_is_typed_between_marks_it_does_not_hold_itself() {
        let typed = typed("type -- 'pi'");
        assert_eq!(script(&typed), Some("type -- 'pi'"));
        assert_eq!(script("type -- 'pi'"), None);
        assert!(!typed.contains(ASKED) && !typed.contains(ANSWERED), "{typed}");
    }

    #[test]
    fn the_answer_is_what_was_printed_between_the_marks_with_how_it_exited() {
        let shown = "rc noise\n% printf … AFKCONVOY_ ASKED; ( pi ) …\nAFKCONVOY_ASKED\nOK\n\n\
                     AFKCONVOY_ANSWERED 0\n% ";
        assert_eq!(answer(&lines(shown)), Some((true, "OK".to_string())));
        let failed = "AFKCONVOY_ASKED\npi: not found\n\nAFKCONVOY_ANSWERED 127\n";
        assert_eq!(answer(&lines(failed)), Some((false, "pi: not found".to_string())));
        let silent = "AFKCONVOY_ASKED\n\nAFKCONVOY_ANSWERED 0\n";
        assert_eq!(answer(&lines(silent)), Some((true, String::new())));
    }

    #[test]
    fn a_question_still_running_has_no_answer() {
        assert_eq!(answer(&lines("rc noise\n")), None);
        assert_eq!(answer(&lines("AFKCONVOY_ASKED\nthinking…\n")), None);
    }
}
