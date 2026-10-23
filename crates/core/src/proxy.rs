//! A host's per-harness additions to the worker login shell's proxy bypass.
use crate::harness::Harness;
use crate::launch::{Supplied, supplied};
use crate::line::quoted;

/// Reads the same bare/default and named forms as a worker launch command.
/// An empty list explicitly leaves the inherited environment alone.
pub fn lists(given: &[String], default: Harness) -> Result<Supplied, String> {
    supplied(given, default).map_err(|why| {
        why.replace("--worker-command", "--worker-no-proxy")
            .replace("worker launch command", "worker proxy bypass list")
            .replace("<command>", "<hosts>")
            .replace("command", "list")
    })
}

/// The host's addition, or the harness's stock list. None leaves both
/// inherited spellings untouched; an explicitly empty list turns stock off.
pub fn addition(said: &Supplied, harness: Harness) -> Option<&str> {
    said.of(harness)
        .or(if harness == Harness::Dsh { Some("api.deepseek.com") } else { None })
        .filter(|list| !list.is_empty())
}

/// Extends both inherited spellings at the worker's terminal, after its
/// login shell has read the host user's exports. No other variable changes.
pub fn line(command: &str, addition: Option<&str>) -> String {
    match addition {
        None => command.to_string(),
        Some(list) => format!(
            "export no_proxy=\"${{no_proxy:+$no_proxy,}}${{NO_PROXY:+$NO_PROXY,}}\"{} && export NO_PROXY=\"$no_proxy\" && {command}",
            quoted(list)
        ),
    }
}

/// What both spellings will hold when there is an addition. Without one,
/// each keeps its own inherited value.
pub fn effective(lower: &str, upper: &str, addition: Option<&str>) -> (String, String) {
    match addition {
        None => (lower.to_string(), upper.to_string()),
        Some(list) => {
            let joined = [lower, upper, list]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(",");
            (joined.clone(), joined)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_names_each_harness_once_and_the_bare_list_is_the_defaults() {
        let said = lists(&["internal".into(), "dsh=other".into()], Harness::Codex).unwrap();
        assert_eq!(addition(&said, Harness::Codex), Some("internal"));
        assert_eq!(addition(&said, Harness::Dsh), Some("other"));
        assert_eq!(addition(&said, Harness::Pi), None);
        for given in [vec!["unknown=host".into()], vec!["a".into(), "codex=b".into()]] {
            assert!(lists(&given, Harness::Codex).unwrap_err().contains("--worker-no-proxy"));
        }
    }

    #[test]
    fn dsh_alone_has_a_stock_addition_which_a_host_can_turn_off() {
        let stock = Supplied::default();
        for harness in Harness::ALL {
            assert_eq!(
                addition(&stock, harness),
                (harness == Harness::Dsh).then_some("api.deepseek.com")
            );
        }
        let off = lists(&["dsh=".into()], Harness::ClaudeCode).unwrap();
        assert_eq!(addition(&off, Harness::Dsh), None);
    }

    #[test]
    fn both_inherited_spellings_are_kept_and_extended_only_with_an_addition() {
        assert_eq!(
            effective("lower", "upper", Some("api")),
            ("lower,upper,api".into(), "lower,upper,api".into())
        );
        assert_eq!(effective("", "", Some("api")), ("api".into(), "api".into()));
        assert_eq!(effective("lower", "upper", None), ("lower".into(), "upper".into()));
        assert_eq!(line("opaque  ; command", None), "opaque  ; command");
    }
}
