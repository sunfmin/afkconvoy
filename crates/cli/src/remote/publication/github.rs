//! Publication to a GitHub HTTPS origin, where no raw receive-pack is at hand.
//!
//! A plain `git push --atomic` carries every new object to scratch refs in
//! the repo's own ref layout. One `updateRefs` call then checks every ref's
//! old value — an unchanged source head included — and applies every update
//! and deletes the scratch refs, all or none. git's credential helper and
//! `gh` authenticate; nothing here reads a credential.
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::Update;
use crate::gh::GitHub;
use crate::git::Repo;

const PUSH_LIMIT: Duration = Duration::from_secs(300);
const READ_LIMIT: Duration = Duration::from_secs(60);

/// A push that would prompt for a login fails instead.
const NO_PROMPT: [(&str, &str); 1] = [("GIT_TERMINAL_PROMPT", "0")];

/// Startup: `git push` reaches origin's receive-pack with a login, and `gh`
/// may write the repo's refs. Nothing is written.
pub(super) fn check(repo: &Repo) -> Result<()> {
    let head = repo.git(&["rev-parse", "HEAD"])?;
    let target = format!("{}:{}", head.trim(), repo.layout().under("publication-check"));
    let ran = crate::git::run_within(
        &repo.dir,
        &["push", "--dry-run", "--porcelain", "origin", &target],
        &NO_PROMPT,
        READ_LIMIT,
    )?;
    if !ran.ok {
        bail!(
            "conditional publication authentication failed: `git push` to the GitHub HTTPS origin needs a login git's credential helper gives without a prompt (`gh auth setup-git`): {}",
            ran.stderr.trim()
        );
    }
    let name = own(repo)?;
    let permission =
        repository(repo, &name)?["viewerPermission"].as_str().unwrap_or("").to_string();
    if !matches!(permission.as_str(), "WRITE" | "MAINTAIN" | "ADMIN") {
        bail!(
            "conditional publication needs `gh` to write {name}'s refs; its login holds {:?} there",
            permission
        );
    }
    Ok(())
}

/// Publishes `updates` all together, or none → whether they took.
pub(super) fn publish(repo: &Repo, name: &str, updates: &[Update<'_>]) -> Result<bool> {
    let tag = crate::unique();
    // Each new object a ref is moved to rides to a scratch ref of its own.
    let scratch: Vec<(String, &str)> = updates
        .iter()
        .filter(|u| !zero(u.new) && u.new != u.old)
        .enumerate()
        .map(|(i, u)| (repo.layout().under(&format!("publish-{tag}-{i}")), u.new))
        .collect();
    if !scratch.is_empty() {
        let mut args = vec!["push".to_string(), "--atomic".into(), "--porcelain".into()];
        args.push("origin".into());
        args.extend(scratch.iter().map(|(name, sha)| format!("{sha}:{name}")));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let ran = crate::git::run_within(&repo.dir, &args, &NO_PROMPT, PUSH_LIMIT)?;
        if !ran.ok {
            // A push that may have taken leaves nothing behind it.
            sweep(repo, &scratch);
            bail!("cannot carry the publication's objects to origin: {}", ran.stderr.trim());
        }
    }

    let id = repository(repo, name)?["id"]
        .as_str()
        .context("GitHub named no id for the repository")?
        .to_string();
    let mut refs: Vec<String> =
        updates.iter().map(|u| ref_update(u.name, u.old, u.new)).collect::<Result<_>>()?;
    for (name, sha) in &scratch {
        refs.push(ref_update(name, sha, &"0".repeat(sha.len()))?);
    }
    let query = format!(
        "mutation($r:ID!){{updateRefs(input:{{repositoryId:$r,refUpdates:[{}]}}){{clientMutationId}}}}",
        refs.join(",")
    );
    let query = format!("query={query}");
    let r = format!("r={id}");
    let sent = GitHub::of(repo)?.raw(&["api", "graphql", "-f", &query, "-f", &r]);
    let Err(error) = sent else {
        return Ok(true);
    };
    // GitHub says nothing typed of a stale old value, and a failed answer may
    // follow a batch it applied: the refs say which it was.
    sweep(repo, &scratch);
    match outcome(&live(repo, updates)?, updates) {
        Outcome::Applied => Ok(true),
        Outcome::Stale => {
            eprintln!("[afkconvoy] publication: a ref moved before it took: {error:#}");
            Ok(false)
        }
        Outcome::Refused => Err(error.context("GitHub refused the publication; no ref moved")),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Every ref is at its new value.
    Applied,
    /// Some ref is neither where it was expected nor where it was sent: work
    /// that read the old value is out of date.
    Stale,
    /// Every ref is still at its old value: nothing took, and nothing moved.
    Refused,
}

/// What the refs, as read after a failed answer, say of the batch. A ref
/// absent from `seen` is at the zero object.
fn outcome(seen: &[(String, String)], updates: &[Update<'_>]) -> Outcome {
    let at = |name: &str| seen.iter().find(|(n, _)| n == name).map(|(_, sha)| sha.as_str());
    let is = |name: &str, sha: &str| match at(name) {
        Some(found) => found == sha,
        None => zero(sha),
    };
    if updates.iter().all(|u| is(u.name, u.new)) && updates.iter().any(|u| u.new != u.old) {
        Outcome::Applied
    } else if updates.iter().all(|u| is(u.name, u.old)) {
        Outcome::Refused
    } else {
        Outcome::Stale
    }
}

fn zero(sha: &str) -> bool {
    sha.bytes().all(|c| c == b'0')
}

/// One `RefUpdate` literal. Its strings are JSON strings, which GraphQL reads
/// alike; a zero old value says the ref must not exist. No `force`: a ref is
/// moved fast-forward only (docs/adr/0037), or deleted.
fn ref_update(name: &str, old: &str, new: &str) -> Result<String> {
    Ok(format!(
        "{{name:{},beforeOid:{},afterOid:{}}}",
        serde_json::to_string(name)?,
        serde_json::to_string(old)?,
        serde_json::to_string(new)?
    ))
}

/// The repo's id and what `gh`'s login may do there.
fn repository(repo: &Repo, name: &str) -> Result<Value> {
    let (owner, short) = name.split_once('/').context("a GitHub repo is owner/name")?;
    let query =
        "query=query($o:String!,$n:String!){repository(owner:$o,name:$n){id viewerPermission}}";
    let (o, n) = (format!("o={owner}"), format!("n={short}"));
    let body = GitHub::of(repo)?.raw(&["api", "graphql", "-f", query, "-f", &o, "-f", &n])?;
    let body: Value = serde_json::from_str(&body).context("GitHub's answer is not JSON")?;
    let found = body["data"]["repository"].clone();
    if found.is_null() {
        bail!("GitHub shows `gh` no repository {name}");
    }
    Ok(found)
}

fn own(repo: &Repo) -> Result<String> {
    let url = repo.git(&["remote", "get-url", "--push", "origin"])?;
    super::github_https(url.trim()).context("origin is not a GitHub HTTPS URL")
}

/// Every one of `updates`' refs as origin has it now.
fn live(repo: &Repo, updates: &[Update<'_>]) -> Result<Vec<(String, String)>> {
    let mut args = vec!["ls-remote", "origin"];
    args.extend(updates.iter().map(|u| u.name));
    let ran = crate::git::run_within(&repo.dir, &args, &NO_PROMPT, READ_LIMIT)?;
    if !ran.ok {
        bail!("cannot read origin after a failed publication: {}", ran.stderr.trim());
    }
    Ok(ran
        .stdout
        .lines()
        .filter_map(|row| row.split_once('\t'))
        .map(|(sha, name)| (name.to_string(), sha.to_string()))
        .collect())
}

/// Deletes the scratch refs, as far as it can: one left behind is named for
/// its process and holds nothing the base branch needs.
fn sweep(repo: &Repo, scratch: &[(String, &str)]) {
    if scratch.is_empty() {
        return;
    }
    let mut args = vec!["push".to_string(), "--porcelain".into(), "origin".into()];
    args.extend(scratch.iter().map(|(name, _)| format!(":{name}")));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Ok(ran) = crate::git::run_within(&repo.dir, &args, &NO_PROMPT, READ_LIMIT)
        && !ran.ok
        && !ran.stderr.contains("remote ref does not exist")
    {
        eprintln!("[afkconvoy] publication: scratch refs not deleted: {}", ran.stderr.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, outcome, ref_update};
    use crate::remote::Update;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";
    const Z: &str = "0000000000000000000000000000000000000000";

    fn batch() -> [Update<'static>; 3] {
        [
            Update { name: "refs/heads/pr", old: A, new: A },
            Update { name: "refs/convoy/candidates/main", old: Z, new: B },
            Update { name: "refs/heads/main", old: A, new: C },
        ]
    }

    fn seen(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter().map(|(n, s)| (n.to_string(), s.to_string())).collect()
    }

    #[test]
    fn refs_at_their_new_values_after_a_failed_answer_took() {
        let rows = seen(&[
            ("refs/heads/pr", A),
            ("refs/convoy/candidates/main", B),
            ("refs/heads/main", C),
        ]);
        assert_eq!(outcome(&rows, &batch()), Outcome::Applied);
    }

    #[test]
    fn refs_all_at_their_old_values_were_refused() {
        let rows = seen(&[("refs/heads/pr", A), ("refs/heads/main", A)]);
        assert_eq!(outcome(&rows, &batch()), Outcome::Refused);
    }

    #[test]
    fn a_moved_source_head_is_stale_even_with_the_rest_unmoved() {
        let rows = seen(&[("refs/heads/pr", B), ("refs/heads/main", A)]);
        assert_eq!(outcome(&rows, &batch()), Outcome::Stale);
    }

    #[test]
    fn a_half_seen_batch_is_stale_never_applied() {
        let rows = seen(&[
            ("refs/heads/pr", A),
            ("refs/convoy/candidates/main", B),
            ("refs/heads/main", A),
        ]);
        assert_eq!(outcome(&rows, &batch()), Outcome::Stale);
    }

    #[test]
    fn a_ref_update_is_a_graphql_literal_with_json_strings() {
        assert_eq!(
            ref_update("refs/heads/a\"b", A, B).unwrap(),
            format!("{{name:\"refs/heads/a\\\"b\",beforeOid:\"{A}\",afterOid:\"{B}\"}}")
        );
    }
}
