//! Read-only migration of a line left by an older release.
use crate::gh::Tracker;
use crate::git::Repo;
use crate::remote::{Fresh, Listing, Push};
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
        let remote = crate::remote::Remote::open(repo, &config.base_branch)?;
        Legacy::listed(repo, config, remote.listing()?)
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

/// A retained workspace is linked to the issue that owns its resolution.
/// The next tick and worker find it through the same Orca lookup as any
/// other continuation, including after this process dies.
fn workspace(
    repo: &Repo,
    gh: &crate::gh::GitHub,
    issue: u64,
    head: &str,
) -> Result<crate::orca::Worktree> {
    let source = crate::orca::Source::of(repo)?;
    if let Some(worktree) = crate::orca::Worktree::of_issue(&source, issue)? {
        return Ok(worktree);
    }
    let read = gh.issue(issue)?;
    let name = gh.tracker.worktree_name(issue, &read.title);
    let worktree = crate::orca::Worktree::cut(&source, &name, head, Some(issue))?;
    gh.comment(issue, &afkconvoy_core::worker::branch_comment(&worktree.branch))?;
    Ok(worktree)
}

/// Gives legacy work without a live issue an ordinary repair issue and PR.
/// It is outside product workflows, so no human stage can strand migration.
fn recovery(
    repo: &Repo,
    config: &Config,
    gh: &crate::gh::GitHub,
    old: &str,
    key: &str,
    head: &str,
) -> Result<(u64, afkconvoy_core::plan::PullRequest)> {
    use afkconvoy_core::plan::{REPAIR_LABEL, closing_pr};
    let marker = format!("<!--convoy:migration tip={old} part={key}-->");
    let mut issue = None;
    for (number, _) in gh.carrying(REPAIR_LABEL)? {
        if gh.issue(number)?.body.is_some_and(|body| body.starts_with(&marker)) {
            issue = Some(number);
            break;
        }
    }
    let labels = gh.labels()?;
    for label in [REPAIR_LABEL, &config.ready_label] {
        if !labels.iter().any(|known| known == label) {
            gh.create_label(label, "afkconvoy: fleet repair")?;
        }
    }
    let issue = match issue {
        Some(issue) => issue,
        None => gh.open_issue(&format!("Finish legacy landing {key}"), &format!("{marker}\nPreserve the work of an older release at {head}. Continue the existing PR and worktree; resolve its merge or red gate, commit, and land with `afkconvoy land --issue <n>`. Preserve both sides of a conflict. The legacy line stays until its history is on the base branch."), &[REPAIR_LABEL, &config.ready_label])?,
    };
    let worktree = workspace(repo, gh, issue, head)?;
    let open = gh.open_prs()?;
    if let Some(pr) = closing_pr(&open, issue) {
        return Ok((issue, pr.clone()));
    }
    let checkout = Repo::new(worktree.path);
    let head = checkout.git(&["rev-parse", "HEAD"])?;
    checkout.remote().push(&Push::set(&head, &format!("refs/heads/{}", worktree.branch)))?;
    let title = format!("Preserve legacy landing {key}");
    let body =
        format!("Preserve work left by an older release.\n\nCloses {}", gh.tracker.issue(issue));
    let number = gh.open_pr(&config.base_branch, &worktree.branch, &title, &body)?;
    Ok((
        issue,
        afkconvoy_core::plan::PullRequest {
            number,
            title,
            head_branch: worktree.branch,
            head_sha: head,
            base_branch: config.base_branch.clone(),
            closes: vec![issue],
        },
    ))
}

fn attempt(
    repo: &Repo,
    config: &Config,
    gh: &crate::gh::GitHub,
    issue: u64,
    pr: &afkconvoy_core::plan::PullRequest,
    (host, now): (&str, u64),
) -> Result<serde_json::Value> {
    use serde_json::json;
    let worktree = workspace(repo, gh, issue, &pr.head_sha)?;
    if (worktree.terminal()?.is_some() && worktree.recorded_harness()?.is_some())
        || crate::claims::Scan::read(repo)?.claim_of(issue).is_some()
    {
        return Ok(json!({"outcome":"pending", "issue":issue, "worktree":worktree.path}));
    }
    let checkout = Repo::new(worktree.path.clone());
    if !checkout.git_raw(&["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty() {
        return Ok(json!({"outcome":"pending", "issue":issue, "worktree":worktree.path,
            "detail":"the retained workspace has unfinished work; its worker continues it"}));
    }
    crate::turn::try_join(
        &checkout,
        config,
        &gh.tracker,
        pr,
        &afkconvoy_core::landing::Of::Issue(issue),
        json!({"issue":issue, "worktree":worktree.path}),
        (host, now),
    )
}

/// Finishes a read-only legacy line. A certified tip fast-forwards directly;
/// otherwise PRs take turns in discoverable issue workspaces. Missing PRs,
/// rewritten heads and commits made by the old worker are all preserved by
/// a final recovery PR. A conflict or red gate keeps its issue and workspace.
pub fn migrate(
    repo: &Repo,
    config: &Config,
    gh: &crate::gh::GitHub,
    host: &str,
    now: u64,
) -> Result<serde_json::Value> {
    use afkconvoy_core::gate::{GateCommand, Proven, Tree};
    use serde_json::json;
    let refs = train_refs(repo.layout(), &config.base_branch);
    let Some(old) = repo.remote().sha(&refs.line, Fresh::Process)? else {
        return Ok(json!({"outcome":"idle"}));
    };
    let legacy = Legacy::read(repo, config)?;
    let source = crate::orca::Source::of(repo)?;
    if let Some(worktree) = crate::orca::Worktree::of_train(&source)? {
        worktree.stop_for_continuation()?;
    }
    let retire = || -> Result<()> {
        repo.remote().push(&Push::delete(&refs.line).leased(&old))?;
        if let Some(red) = repo.remote().sha(&refs.red, Fresh::Process)? {
            let _ =
                repo.remote()
                    .try_push_retried(&Push::delete(&refs.red).leased(&red), None, || Ok(false));
        }
        Ok(())
    };
    if legacy.tip == legacy.base {
        retire()?;
        return Ok(json!({"outcome":"merged", "legacy":old}));
    }
    let declared = crate::config::parse(
        &repo.git_raw(&["show", &format!("{}:{}", legacy.tip, crate::config::FILE)])?,
    )?;
    let command = GateCommand::new(&declared.gate_command);
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{}^{{tree}}", legacy.tip)])?);
    let joins = legacy.joins(repo, &gh.tracker)?;
    {
        let _permit =
            crate::exclusive::Permit::take(repo, "migrate recorded legacy work", false, true)?;
        if let Some(run) = crate::gate::recorded(repo, &tree, &command, now)?
            && let Some(proven) = Proven::by(&tree, &command, &run, now)
            && crate::landing::push_base(repo, &config.base_branch, &legacy.tip, &proven)?
        {
            for issue in joins.iter().filter_map(|join| join.issue()) {
                if !gh.issue(issue)?.is_closed() {
                    gh.close_issue(issue)?;
                }
            }
            retire()?;
            return Ok(
                json!({"outcome":"merged", "commit":legacy.tip, "source":"recorded", "legacy":old}),
            );
        }
    }
    for join in &joins {
        let open = gh.open_prs()?;
        let Some(pr) = open.iter().find(|pr| pr.number == join.pr) else {
            continue;
        };
        repo.bring(&pr.head_sha, &format!("refs/heads/{}", pr.head_branch))?;
        if repo.within(&pr.head_sha, &crate::turn::base(repo, config)?)? {
            continue;
        }
        let (issue, pr) = match join.issue() {
            Some(issue) if !gh.issue(issue)?.is_closed() => (issue, pr.clone()),
            _ => recovery(repo, config, gh, &old, &pr.number.to_string(), &pr.head_sha)?,
        };
        let answer = attempt(repo, config, gh, issue, &pr, (host, now))?;
        if answer["outcome"] != "merged" {
            return Ok(answer);
        }
    }
    // Always preserve the recorded history, including closed PR heads and
    // old repairs that no current open PR can represent.
    let (issue, pr) = recovery(repo, config, gh, &old, "history", &legacy.tip)?;
    let answer = attempt(repo, config, gh, issue, &pr, (host, now))?;
    if answer["outcome"] == "merged" {
        retire()?;
    }
    Ok(answer)
}

/// Ends owned proposal workers without removing their work or treating a
/// peer's worktree as ours. Retired claims never enter slot or idle counts.
/// A failed retirement is retried by the next tick without blocking issues.
pub fn retire_proposals(fleet: &mut crate::fleet::Fleet, me: &crate::claims::Me) -> Result<()> {
    use crate::claims::drop_run;
    use afkconvoy_core::claims::Holder;
    use afkconvoy_core::legacy::Proposal;

    let runs = fleet.scan.runs.clone();
    for claim in runs {
        let Some(old) = Proposal::claimed(&claim.key) else { continue };
        let local_owner = claim
            .owner
            .as_ref()
            .is_some_and(|owner| owner.instance == me.instance.as_str() && owner.host == me.host);
        let dead = claim.holder(Some(&me.instance), &fleet.scan.beats(), me.now) == Holder::Dead;
        if !local_owner && !dead {
            continue;
        }
        // Check the remote again before stopping a local worker. A moved
        // claim is another fleet's; releasing below is leased to this sha too.
        let refname = afkconvoy_core::claims::run_ref(fleet.repo.layout(), &claim.key);
        if fleet.remote.sha(&refname, Fresh::Process)?.as_deref() != Some(claim.sha.as_str()) {
            continue;
        }
        if local_owner {
            let source = crate::orca::Source::of(&fleet.repo)?;
            if let Some(worktree) = crate::orca::Worktree::named(&source, &old.branch())? {
                anyhow::ensure!(
                    worktree.branch.rsplit('/').next().is_some_and(|name| {
                        afkconvoy_core::worker::is_worktree_named(name, &old.branch())
                    }),
                    "retained proposal worktree has another branch: {}; left alone",
                    worktree.path.display()
                );
                worktree.stop_for_continuation()?;
            }
        }
        drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
    }
    report_proposals(fleet)
}

/// Lists the team's legacy PRs once per checkout/team binding, then records
/// completion on the remote. Each PR keeps an idempotent disposition comment.
/// This is upgrade migration, never an acceptance queue or ongoing PR poll.
fn report_proposals(fleet: &crate::fleet::Fleet) -> Result<()> {
    use afkconvoy_core::legacy::Proposal;
    let Some(bound) = &fleet.config.bound else { return Ok(()) };
    let team = &bound.team.repo;
    let refname = fleet.repo.layout().under(&format!("migration/proposals/{}", bound.team));
    if fleet.remote.sha(&refname, Fresh::Process)?.is_some() {
        return Ok(());
    }
    for pr in fleet.gh.open_prs_of(team)? {
        let Some(old) = Proposal::branch_named(&pr.head_ref_name) else { continue };
        let marker = "<!--convoy:retired-proposal-->";
        let number = pr.number.to_string();
        let read = fleet.gh.gh(&[
            "pr",
            "view",
            &number,
            "--repo",
            team.as_str(),
            "--json",
            "comments",
            "--jq",
            ".comments[].body",
        ])?;
        if read.contains(marker) {
            continue;
        }
        let link = format!("https://github.com/{team}/pull/{number}");
        let body = format!(
            "{marker}\n**afkconvoy** — retired automatic proposal: {link}. Its legacy branch names \
             issue {} and ruling comment {}; the source remains in this PR's history. \
             Disposition: retain this PR, its branch and decision history for review; \
             no automatic merge or close. Review its effective content through ordinary work in \
             the owning ADR, team or role rules, amend conflicting older wording and preserve \
             source links. A shared GitHub identity, an agent ruling or a merged document alone \
             establishes no human decision. The issue's ruling still stands on its issue.",
            old.issue, old.comment
        );
        fleet.gh.gh(&["pr", "comment", &number, "--repo", team.as_str(), "--body", &body])?;
    }
    let sha = fleet.repo.record_commit("convoy:migration retired=proposals")?;
    fleet.remote.push(&Push::set(&sha, &refname))?;
    Ok(())
}
