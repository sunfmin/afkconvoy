//! Read-only migration of a line left by an older release.
use crate::gh::Tracker;
use crate::git::{Listing, Repo};
use afkconvoy_core::config::Config;
use afkconvoy_core::landing::LandedPr;
use afkconvoy_core::legacy::{Line, joins_onto, train_refs};
use anyhow::{Result, anyhow};
/// The landing train as one read of the remote saw it, its commits in the
/// repo that asked.
pub struct Legacy {
    /// What the next PR joins onto: the line's tip, or the base branch's when
    /// the line holds nothing the base branch lacks.
    pub tip: String,
    /// The base branch's tip.
    pub base: String,
}

impl Legacy {
    /// The train as the remote has it now.
    pub fn read(repo: &Repo, config: &Config) -> Result<Legacy> {
        Legacy::listed(repo, config, &repo.listing(&config.base_branch)?)
    }

    /// The train as `listing` saw it.
    pub fn listed(repo: &Repo, config: &Config, listing: &Listing) -> Result<Legacy> {
        let branch = &config.base_branch;
        let base =
            listing.base.clone().ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
        repo.bring(&base, &listing.base_ref)?;
        let line_ref = train_refs(repo.layout(), branch).line;
        let line = listing.sha(&line_ref).map(str::to_string);
        if let Some(tip) = &line {
            repo.bring(tip, &line_ref)?;
        }
        let on_base = match &line {
            Some(tip) => repo.within(tip, &base)?,
            None => false,
        };
        let line = line.as_deref().map(|tip| Line { tip, on_base });
        let tip = joins_onto(line, &base).to_string();
        Ok(Legacy { tip, base })
    }

    /// The joinings on the train that the base branch does not hold yet,
    /// oldest first, as the fleet whose tracker is `tracker` reads them.
    pub fn joins(&self, repo: &Repo, tracker: &Tracker) -> Result<Vec<LandedPr>> {
        if self.tip == self.base {
            return Ok(vec![]);
        }
        crate::landing::joins_in(repo, tracker, &[&format!("{}..{}", self.base, self.tip)])
    }
}

/// Finishes a line left by an old release. Its green recorded tip lands by
/// fast-forward. Otherwise its PRs take ordinary turns, oldest first. The
/// line stays until every head is on the base; a conflict loses no PR.
pub fn migrate(
    repo: &Repo,
    config: &Config,
    gh: &crate::gh::GitHub,
    host: &str,
    now: u64,
) -> Result<serde_json::Value> {
    use crate::exclusive::Permit;
    use crate::git::REMOTE;
    use afkconvoy_core::gate::{GateCommand, Proven, Tree};
    use serde_json::json;
    let refs = train_refs(repo.layout(), &config.base_branch);
    let Some(old) = repo.remote_sha(&refs.line)? else {
        return Ok(json!({"outcome":"idle"}));
    };
    let legacy = Legacy::read(repo, config)?;
    // An older worker cannot keep writing this host's obsolete line.
    let source = crate::orca::Source::of(repo)?;
    if let Some(worktree) = crate::orca::Worktree::of_train(&source)? {
        worktree.stop_for_continuation()?;
    }
    if legacy.tip == legacy.base {
        let _ = repo.try_git(&[
            "push",
            "--quiet",
            &format!("--force-with-lease={}:{}", refs.line, old),
            REMOTE,
            &format!(":{}", refs.line),
        ]);
        return Ok(json!({"outcome":"merged", "legacy":old}));
    }
    let declared = crate::config::parse(
        &repo.git_raw(&["show", &format!("{}:{}", legacy.tip, crate::config::FILE)])?,
    )?;
    let command = GateCommand::new(&declared.gate_command);
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{}^{{tree}}", legacy.tip)])?);
    let joins = legacy.joins(repo, &gh.tracker)?;
    let permit = Permit::take(repo, "migrate recorded legacy work", false, true)?;
    if let Some(run) = crate::gate::recorded(repo, &tree, &command, now)?
        && let Some(proven) = Proven::by(&tree, &command, &run, now)
        && crate::landing::push_base(repo, &config.base_branch, &legacy.tip, &proven)?
    {
        for issue in joins.iter().filter_map(|join| join.issue()) {
            if !gh.issue(issue)?.is_closed() {
                gh.close_issue(issue)?;
            }
        }
        let _ = repo.try_git(&[
            "push",
            "--quiet",
            &format!("--force-with-lease={}:{}", refs.line, old),
            REMOTE,
            &format!(":{}", refs.line),
        ]);
        return Ok(
            json!({"outcome":"merged", "commit":legacy.tip, "source":"recorded", "legacy":old}),
        );
    }
    drop(permit);
    // Certification must not wait inside the tick that dispatches repairs.
    let base_config = crate::config::parse(
        &repo.git_raw(&["show", &format!("{}:{}", legacy.base, crate::config::FILE)])?,
    )?;
    if !crate::turn::certify(repo, &base_config, &legacy.base, host, now)? {
        return Ok(json!({"outcome":"base_red", "legacy":old}));
    }
    for join in &joins {
        let open = gh.open_prs()?;
        let Some(pr) = open.iter().find(|pr| pr.number == join.pr) else {
            continue;
        };
        if repo.within(&pr.head_sha, &crate::turn::base(repo, config)?)? {
            continue;
        }
        let path = std::env::temp_dir().join(format!("afkconvoy-migrate-{}", crate::unique()));
        repo.bring(&pr.head_sha, &format!("refs/heads/{}", pr.head_branch))?;
        repo.git(&[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &path.to_string_lossy(),
            &pr.head_sha,
        ])?;
        let checkout = Repo::new(path.clone());
        let landed =
            crate::turn::join(&checkout, config, &gh.tracker, pr, &join.of, json!({}), (host, now));
        // A conflict is retained for its worker, never discarded on migration.
        if let Ok(answer) = &landed
            && answer["outcome"] == "conflict"
        {
            return Ok(
                json!({"outcome":"conflict", "pr":pr.number, "worktree":path, "legacy":old}),
            );
        }
        let removed = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
        let answer = landed?;
        removed?;
        if answer["outcome"] != "merged" {
            return Ok(answer);
        }
    }
    let base = crate::turn::base(repo, config)?;
    if joins.iter().all(|join| repo.within(&join.head, &base).unwrap_or(false)) {
        // A train worker could have committed a repair on top of its merges.
        // Preserve that history too, through one final ordinary turn, rather
        // than dropping commits that did not belong to an issue PR.
        let branch = format!("convoy-migrate-{}", &old[..12.min(old.len())]);
        let path = std::env::temp_dir().join(format!("afkconvoy-migrate-{}", crate::unique()));
        repo.git(&["push", "--quiet", REMOTE, &format!("{}:refs/heads/{branch}", legacy.tip)])?;
        let open = gh.open_prs()?;
        let pr = match open.iter().find(|pr| pr.head_branch == branch) {
            Some(pr) => pr.clone(),
            None => {
                let number = gh.open_pr(
                    &config.base_branch,
                    &branch,
                    "Preserve legacy landing history",
                    "Preserves work left by an older release through a recorded landing turn.",
                )?;
                afkconvoy_core::plan::PullRequest {
                    number,
                    title: "Preserve legacy landing history".into(),
                    head_branch: branch,
                    head_sha: legacy.tip.clone(),
                    closes: vec![],
                }
            }
        };
        repo.git(&[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &path.to_string_lossy(),
            &legacy.tip,
        ])?;
        let checkout = Repo::new(path.clone());
        let landed = crate::turn::join(
            &checkout,
            config,
            &gh.tracker,
            &pr,
            &afkconvoy_core::landing::Of::Inspection(host.into()),
            json!({}),
            (host, now),
        );
        let removed = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
        let answer = landed?;
        removed?;
        if answer["outcome"] == "merged" {
            let _ = repo.try_git(&[
                "push",
                "--quiet",
                &format!("--force-with-lease={}:{}", refs.line, old),
                REMOTE,
                &format!(":{}", refs.line),
            ]);
        }
        return Ok(answer);
    }
    Ok(
        json!({"outcome":"pending", "legacy":old, "detail":"legacy work remains available for its PRs' turns"}),
    )
}
