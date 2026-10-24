//! Which repo a checkout is of, and whether a host holds one of that repo
//! already: one fleet instance per host per repo (docs/adr/0003, docs/adr/0023).

use crate::tools::remote_parts;

/// The repo a remote's URL names, the same however it is written:
/// `https://github.com/Owner/Name.git`, `git@github.com:owner/name` and
/// `ssh://git@github.com:22/owner/name/` are each `github.com/owner/name`, in
/// lower case as GitHub takes it. `None` for a remote that is a path on this
/// host (`/srv/git/w.git`, `../w.git`, `file:///srv/git/w.git`): the caller
/// makes it canonical against its checkout.
pub fn repo_of(url: &str) -> Option<String> {
    let (host, path) = remote_parts(url)?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    let repo = format!("{host}/{path}");
    Some(repo.trim_end_matches('/').to_ascii_lowercase())
}

/// Where a checkout stands against the checkouts a host knows.
#[derive(Debug, PartialEq, Eq)]
pub enum OnList<'l> {
    /// No checkout of its repo.
    No,
    /// This very checkout.
    Itself,
    /// Another checkout of the same repo: the one it names.
    Other(&'l str),
}

/// Where the checkout `checkout`, of the repo `repo`, stands against `known`:
/// each checkout listed or held, with its repo where that could be read. The
/// checkout itself is found by its path, before any repo is compared; one
/// whose repo could not be read is of no repo.
pub fn on_list<'l>(
    known: &'l [(String, Option<String>)],
    checkout: &str,
    repo: &str,
) -> OnList<'l> {
    if known.iter().any(|(listed, _)| listed == checkout) {
        return OnList::Itself;
    }
    match known.iter().find(|(_, of)| of.as_deref() == Some(repo)) {
        Some((first, _)) => OnList::Other(first),
        None => OnList::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repo_is_its_remote_however_the_url_is_written() {
        for url in [
            "https://github.com/Owner/Name.git",
            "https://github.com/owner/name",
            "https://github.com/owner/name/",
            "https://github.com/owner/name.git/",
            "https://user@github.com:443/owner/name.git",
            "ssh://git@github.com:22/owner/name/",
            "ssh://git@github.com/owner/name.git",
            "git@github.com:owner/name",
            "git@github.com:Owner/Name.git\n",
            "git@GitHub.com:/owner/name.git",
        ] {
            assert_eq!(repo_of(url).as_deref(), Some("github.com/owner/name"), "{url:?}");
        }
        // a fork is another owner's, and another host's is another repo
        assert_eq!(
            repo_of("git@github.com:fork/name.git").as_deref(),
            Some("github.com/fork/name")
        );
        let enterprise = repo_of("https://ghe.example.com:8443/team/name.git");
        assert_eq!(enterprise.as_deref(), Some("ghe.example.com/team/name"));
    }

    #[test]
    fn a_remote_that_is_a_path_is_the_callers_to_make_canonical() {
        for url in ["/srv/git/w.git", "../w.git", "file:///srv/git/w.git", "C:/repos/w", ""] {
            assert_eq!(repo_of(url), None, "{url:?}");
        }
    }

    #[test]
    fn a_checkout_is_itself_by_its_path_and_another_of_its_repo_is_named() {
        let known = [
            ("/a/hkj".to_string(), Some("github.com/me/hkj".to_string())),
            ("/a/gone".to_string(), None),
            ("/a/moonfish".to_string(), Some("github.com/me/afkconvoy".to_string())),
            ("/b/hkj".to_string(), Some("github.com/me/hkj".to_string())),
        ];
        assert_eq!(on_list(&known, "/c/w", "github.com/me/w"), OnList::No);
        assert_eq!(on_list(&[], "/c/w", "github.com/me/w"), OnList::No);
        // the path decides, whatever its repo is now
        assert_eq!(on_list(&known, "/a/hkj", "github.com/me/moved"), OnList::Itself);
        assert_eq!(on_list(&known, "/a/gone", "github.com/me/hkj"), OnList::Itself);
        // the first of the same repo is named
        assert_eq!(on_list(&known, "/c/hkj", "github.com/me/hkj"), OnList::Other("/a/hkj"));
        assert_eq!(
            on_list(&known, "/c/afk", "github.com/me/afkconvoy"),
            OnList::Other("/a/moonfish")
        );
        // one whose repo could not be read matches none
        let unread = [("/a/gone".to_string(), None)];
        assert_eq!(on_list(&unread, "/c/w", ""), OnList::No);
    }
}
