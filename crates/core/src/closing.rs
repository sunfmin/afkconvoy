//! Which issues a pull request closes, read off its body.
//!
//! GitHub links a PR to the issues its body names only when the PR is against
//! the repo's default branch. A fleet may land on any branch, so the body is
//! read here and the answer is the same whatever the base branch is.

/// The issues a PR closes: the ones GitHub linked to it, and the ones its body
/// names after a closing keyword (`Closes #7`, `fixes: GH-7`) — sorted, each
/// once. Text GitHub does not format (code) names nothing. A reference into
/// another repository (`owner/name#7`) is not one of them.
pub fn issues_closed_by(body: &str, linked: &[u64]) -> Vec<u64> {
    let mut numbers = linked.to_vec();
    let text = formatted(body);
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if !is_keyword(word.trim_end_matches(':')) {
            continue;
        }
        if let Some(number) = words.peek().and_then(|next| reference(next)) {
            numbers.push(number);
        }
    }
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

fn is_keyword(word: &str) -> bool {
    const KEYWORDS: [&str; 9] =
        ["close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved"];
    KEYWORDS.iter().any(|keyword| word.eq_ignore_ascii_case(keyword))
}

/// `#7` or `GH-7`, whatever punctuation follows it → 7.
fn reference(word: &str) -> Option<u64> {
    let digits = match word.strip_prefix('#') {
        Some(rest) => rest,
        None => word.get(..3).filter(|p| p.eq_ignore_ascii_case("gh-")).and(word.get(3..))?,
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let (number, after) = digits.split_at_checked(end)?;
    if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    crate::record::fleet_number(number)
}

/// The body without what GitHub shows as code: fenced blocks and `inline` spans.
fn formatted(body: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in body.lines() {
        let fence = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if fence {
            fenced = !fenced;
        } else if !fenced {
            let mut code = false;
            for part in line.split('`') {
                if !code {
                    out.push_str(part);
                }
                out.push(' ');
                code = !code;
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closing_keyword_names_the_issue() {
        assert_eq!(issues_closed_by("Closes #7", &[]), [7]);
        assert_eq!(issues_closed_by("this fixes: #12.\nResolved GH-3, see #9", &[]), [3, 12]);
        assert_eq!(issues_closed_by("CLOSES #7 and closes #7", &[7, 2]), [2, 7]);
    }

    #[test]
    fn what_is_not_a_closing_reference_closes_nothing() {
        for body in [
            "see #7",
            "closes owner/name#7",
            "closes #7a",
            "closes #x",
            "closes",
            "encloses #7",
            "`closes #7`",
            "```\ncloses #7\n```",
            "closes #007",
        ] {
            assert_eq!(issues_closed_by(body, &[]), [] as [u64; 0], "{body:?}");
        }
    }
}
