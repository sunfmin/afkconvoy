//! Stands in for `gh` in the tests: GitHub's issues, pull requests and comments
//! as one JSON file (`CONVOY_FAKE_GH`), the code in the bare repo it names.
//!
//! State: `issues` [{number, title, labels: [name…], state, state_reason?, closed_at?, closed_by?,
//! blocked_by?, id?}], `labels` [the labels the repo has beside those an issue carries], `edges` {number: [the issues it is blocked by…]}, `comments` {number: [{id, body, html_url, at?}]}, `now` the time a comment posted here is written at, when there is one, `prs` [{number, headRefName, headRefOid, body, state,
//! linked?: [issue…], fork?: its head is a fork's branch, which the bare repo does not hold, at
//! `headRefOid`}], `protection` {branch: rules | "why it cannot be read"}, `bare`, `base`, `calls`, and `fail`: substrings of a
//! command line that make the call fail. `cut_short` {a substring of a command
//! line: how many more calls it cuts short}: each such call fails as one whose
//! answer was cut short in transit. `repos` {owner/repo: the git repo that
//! stands in for it}: the other repos GitHub holds, which `repo clone` clones and
//! `repo create` adds to.
//! `prs_of` {owner/repo: [{number, headRefName, state, closed_with?, fork?}]}: the
//! PRs of those, each from a branch of the repo that stands in — or, where it
//! says `fork`, of a fork, that branch standing in for the fork's; what one
//! changes is that branch against the default one.
//! `deleted`, `transferred` and `missing` {number: how many more calls about
//! it are answered}: past those, each call about the issue answers as GitHub
//! does for an issue deleted — 410 —, transferred to another repo — a read
//! of the issue redirected to `acme/elsewhere`, its number 1000 more, and
//! 404 of any other call —, or never issued — 404.
//! `envs`: beside each of `calls`, the names of the session's environment it
//! ran in (`fake::State`).
//!
//! A checkout whose `.git/convoy-fake-gh` names another file is of a repo of
//! its own: every call run in it reads and writes that file instead.
//!
//! `trackers` {owner/repo: the file of its state}: the repos whose issues a
//! call names by name — an endpoint under `repos/owner/repo/`, or a query's
//! `owner` and `repo` — each with a state of its own, as this one's issues,
//! comments, labels and edges. Such a call is logged here and there, and
//! answered from there.
//!
//! `visibility`: who GitHub lets see the repo — of a tracker, in its own
//! state — `private` where it says none.

mod fake;

use afkconvoy::gh::{
    ACCOUNTS_JQ, ACCOUNTS_QUERY, BLOCKED_BY_JQ, CARRYING_JQ, CLOSED_JQ, COMMENTS_JQ, COMMENTS_WITH,
    DELIVERY_PRS_JQ, ISSUE_JQ, LABELS_JQ, LANDED_ISSUES_JQ, LANDED_ISSUES_QUERY,
    OPEN_AND_LANDED_JQ, OPEN_AND_LANDED_QUERY, OPEN_ISSUES_JQ, OPEN_ISSUES_QUERY, OPEN_LABELLED_JQ,
    PR_FIELDS, PR_HEADS_JQ, PR_OF_FIELDS, PR_OF_JQ, VISIBILITY_JQ, WATCH_ISSUES_JQ,
    WATCH_ISSUES_QUERY, WATCH_PRS_JQ,
};
use std::path::PathBuf;

use fake::State;
use serde_json::{Value, json};

const REPO: &str = "repos/{owner}/{repo}/";

fn main() {
    let mut st = State::open_routed("CONVOY_FAKE_GH", "convoy-fake-gh");
    st.argv.retain(|a| a != "--include");
    let line = st.argv.join(" ");
    let resource = if line.starts_with("api graphql") || line.starts_with("pr list") {
        "graphql"
    } else if line.starts_with("api ") && !line.starts_with("api rate_limit") {
        "core"
    } else {
        "other"
    };
    if let Some(said) = st.doc["quota_fail"][resource].as_str() {
        st.finish("", 1, said);
    }
    if st.doc["quota_fail"][resource].is_object() {
        let refusal = &st.doc["quota_fail"][resource];
        st.finish(
            refusal["stdout"].as_str().unwrap_or(""),
            1,
            refusal["stderr"].as_str().unwrap_or(""),
        );
    }
    let injected = st.doc["fail"]
        .as_array()
        .is_some_and(|parts| parts.iter().filter_map(|p| p.as_str()).any(|p| line.contains(p)));
    if injected {
        st.finish("", 1, "fake gh: injected failure\n");
    }
    let cut_short = st.doc.get_mut("cut_short").and_then(Value::as_object_mut).and_then(|cuts| {
        cuts.iter_mut().find(|(part, left)| line.contains(part.as_str()) && left.as_u64() > Some(0))
    });
    if let Some((_, left)) = cut_short {
        *left = json!(left.as_u64().unwrap() - 1);
        st.finish("", 1, "unexpected end of JSON input\n");
    }
    let mut st = tracked(st);
    let argv: Vec<&str> = st.argv.iter().map(String::as_str).collect();
    let other = st.opt("--repo").map(str::to_string);
    match argv.as_slice() {
        ["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"] => {
            st.finish(st.doc["repository"].as_str().unwrap_or("acme/dev"), 0, "")
        }
        ["api", "--paginate", endpoint, "--jq", jq]
            if endpoint == &format!("{REPO}pulls?state=all&per_page=100") =>
        {
            assert_eq!(*jq, DELIVERY_PRS_JQ);
            notice_merges(&mut st);
            let rows: Vec<String> = st.doc["prs"].as_array().unwrap().iter().map(|pr| {
                json!({"number":pr["number"], "body":pr["body"].as_str().unwrap_or(""),
                    "head_branch":pr["headRefName"], "base_branch":pr["baseRefName"].as_str().unwrap_or("main"),
                    "state":pr["state"], "draft":pr["draft"].as_bool().unwrap_or(false),
                    "merged":pr["merged"].as_bool().unwrap_or(false) || pr["state"] == "merged",
                    "url":format!("https://gh/pull/{}", pr["number"]), "fork":is_fork(pr)}).to_string()
            }).collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["config", "get", "user", "--host", _] => {
            st.finish(st.doc["login"].as_str().unwrap_or("fixture-user"), 0, "")
        }
        ["api", "user", ..] => st.finish(&identity_login(&st), 0, ""),
        ["api", "graphql", ..] if line.contains("ConvoyQuotaIdentity") => {
            st.finish(&identity_login(&st), 0, "")
        }
        ["auth", "status", ..] if st.has("--json") => {
            st.finish(st.doc["login"].as_str().unwrap_or("fixture-user"), 0, "")
        }
        ["api", "rate_limit", ..] => {
            let default = json!({"resources": {
                "core": {"remaining":5000,"reset":4_000_000_000u64},
                "graphql": {"remaining":5000,"reset":4_000_000_000u64}
            }});
            st.finish(&st.doc.get("quota").unwrap_or(&default).to_string(), 0, "")
        }
        ["api", "graphql", ..] if line.contains("ConvoyQuotaEquivalent") => equivalent_graph(&st),
        ["api", "graphql", ..] if line.contains("ConvoyQuotaWrite") => equivalent_write(&mut st),
        ["api", endpoint, "-H", "Accept: application/vnd.github.diff"] => {
            let (repo, number) =
                endpoint.trim_start_matches("repos/").rsplit_once("/pulls/").unwrap();
            let number = number.parse::<u64>().unwrap();
            let from = st.doc["repos"][repo].as_str().unwrap();
            let row = st.doc["prs_of"][repo]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["number"] == number)
                .unwrap();
            st.finish(
                &fake::git(&[
                    "-C",
                    from,
                    "diff",
                    &format!("HEAD...{}", row["headRefName"].as_str().unwrap()),
                ]),
                0,
                "",
            )
        }
        ["api", endpoint, "--jq", jq] if endpoint.starts_with(&format!("{REPO}pulls?")) => {
            assert_eq!(*jq, WATCH_PRS_JQ);
            let page = endpoint.rsplit("page=").next().unwrap().parse::<usize>().unwrap();
            let mut prs = st.doc["prs"].as_array().cloned().unwrap_or_default();
            prs.sort_by_key(|pr| std::cmp::Reverse(pr["updated_at"].as_u64().unwrap_or(0)));
            let rows: Vec<String> = prs
                .iter()
                .skip((page - 1) * 100)
                .take(100)
                .map(|pr| {
                    json!({"number": pr["number"], "title": pr["title"], "body": pr["body"],
                    "updated": pr["updated_at"].as_u64().unwrap_or(0),
                    "closed": pr["closed_at"], "merged": pr["merged_at"],
                    "fork": is_fork(pr), "of": pr["of"], "head_branch": pr["headRefName"].as_str().unwrap_or("")})
                    .to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["pr", verb, ..] if other.is_some() => {
            let (verb, repo) = (verb.to_string(), other.unwrap());
            pr_of(&mut st, &verb, &repo)
        }
        ["pr", "list", ..] => pr_list(&mut st),
        ["pr", "create", ..] => pr_create(&mut st),
        ["pr", "close", number, ..] => {
            let number = number.to_string();
            pr_close(&mut st, &number)
        }
        ["pr", "edit", number, ..] => {
            let number = number.to_string();
            pr_edit(&mut st, &number)
        }
        ["pr", "ready", number, ..] => {
            let number = number.to_string();
            pr_ready(&mut st, &number)
        }
        ["api", endpoint, "--jq", ".sha"] if endpoint.ends_with("/commits/HEAD") => {
            let name = endpoint.trim_start_matches("repos/").trim_end_matches("/commits/HEAD");
            let Some(from) = st.doc["repos"][name].as_str() else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let head = fake::git(&["-C", from, "rev-parse", "HEAD"]);
            st.finish(&head, 0, "")
        }
        ["api", endpoint, "--jq", ".message"] if endpoint.contains("/git/commits/") => {
            let (repo, sha) =
                endpoint.trim_start_matches("repos/").split_once("/git/commits/").unwrap();
            let from = st.doc["repos"][repo].as_str().or_else(|| st.doc["bare"].as_str()).unwrap();
            let message = fake::git(&["-C", from, "show", "-s", "--format=%B", sha]);
            st.finish(&message, 0, "")
        }
        ["api", "-H", _, endpoint] if endpoint.contains("/contents/") => {
            // A file of another repo: its working tree's, as its default branch.
            let (name, path) =
                endpoint.trim_start_matches("repos/").split_once("/contents/").unwrap();
            let file = st.doc["repos"][name].as_str().map(|from| format!("{from}/{path}"));
            match file.and_then(|file| std::fs::read_to_string(file).ok()) {
                Some(text) => st.finish(&text, 0, ""),
                None => st.finish("", 1, "gh: Not Found (HTTP 404)\n"),
            }
        }
        ["issue", "view", number, ..] => {
            assert_eq!(st.opt("--json"), Some("url"));
            let Some(issue) = issue(&st, number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let url = issue["url"].as_str().map(str::to_string).unwrap_or_else(|| {
                format!("https://github.com/{}/issues/{number}", other.as_deref().unwrap())
            });
            st.finish(&json!({"url": url}).to_string(), 0, "")
        }
        ["issue", "list", ..] if other.is_some() => {
            // The issues of another repo, kept under `other_issues`.
            let repo = other.unwrap();
            let rows: Vec<Value> = st.doc["other_issues"][&repo]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| is_open(row))
                .map(|row| json!({"number": row["number"], "body": row["body"]}))
                .collect();
            st.finish(&Value::from(rows).to_string(), 0, "")
        }
        ["issue", "create", ..] if other.is_some() => {
            let repo = other.unwrap();
            let (title, body) = (st.opt("--title").unwrap_or(""), st.opt("--body").unwrap_or(""));
            let issue = json!({"title": title, "body": body, "labels": st.opts("--label"),
                "state": "open"});
            if !st.doc["other_issues"].is_object() {
                st.doc["other_issues"] = json!({});
            }
            if !st.doc["other_issues"][&repo].is_array() {
                st.doc["other_issues"][&repo] = json!([]);
            }
            let rows = st.doc["other_issues"][&repo].as_array_mut().unwrap();
            let number = rows.len() + 1;
            let mut issue = issue;
            issue["number"] = json!(number);
            rows.push(issue);
            st.finish(&format!("https://gh/{repo}/issues/{number}"), 0, "")
        }
        ["api", endpoint, "--jq", jq] if *jq == VISIBILITY_JQ && endpoint.starts_with("repos/") => {
            let visibility = st.doc["visibility"].as_str().unwrap_or("private").to_string();
            st.finish(&visibility, 0, "")
        }
        ["api", "graphql", ..] => open_issues(&st),
        ["api", "--paginate", endpoint]
            if endpoint.starts_with("repos/") && !endpoint.starts_with(REPO) =>
        {
            let mut parts = endpoint.trim_start_matches("repos/").splitn(3, '/');
            let repo = format!("{}/{}", parts.next().unwrap(), parts.next().unwrap());
            if st.doc["repos"][&repo].is_null() {
                st.finish("", 1, "gh: Not Found (HTTP 404)")
            }
            let what = parts.next().unwrap();
            let rows: Vec<_> = if what.starts_with("pulls?") {
                st.doc["prs_of"][&repo].as_array().into_iter().flatten().filter(|r|is_open(r)).map(|r|json!({"number":r["number"],"title":r["title"],"body":r["body"],"created_at":stamp(&r["since"]),
                    "head":{"ref":r["headRefName"],"repo":{"full_name":if is_fork(r){"fork/other"}else{&repo}}},"base":{"repo":{"full_name":repo}}})).collect()
            } else {
                st.doc["other_issues"][&repo]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|r| is_open(r))
                    .map(rest_issue)
                    .collect()
            };
            st.finish(&json!(rows).to_string(), 0, "")
        }
        ["api", ..] => api(&mut st),
        ["issue", "create", ..] => issue_create(&mut st),
        ["issue", "edit", number, ..] => {
            let number = number.parse::<u64>().unwrap();
            let add = st.opts("--add-label");
            let remove = st.opts("--remove-label");
            let has = repo_labels(&st);
            if let Some(lacked) = add.iter().find(|label| !has.contains(label)) {
                st.finish("", 1, &format!("could not add label: '{lacked}' not found\n"));
            }
            let issues = st.doc["issues"].as_array_mut().unwrap();
            let Some(issue) = issues.iter_mut().find(|issue| issue["number"] == number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n");
            };
            let labels = issue["labels"].as_array_mut().unwrap();
            labels.retain(|label| !remove.iter().any(|off| label == off));
            for label in add {
                if !labels.contains(&json!(label)) {
                    labels.push(json!(label));
                }
            }
            st.finish(&format!("https://gh/issues/{number}"), 0, "");
        }
        ["repo", "clone", repo, into, "--", flags @ ..] => {
            let Some(from) = st.doc["repos"][*repo].as_str() else {
                st.finish(
                    "",
                    1,
                    &format!(
                        "GraphQL: Could not resolve to a Repository with the name '{repo}'.\n"
                    ),
                )
            };
            let cloned = fake::ok(&[&["clone"], flags, &[from, *into]].concat());
            st.finish(
                "",
                if cloned { 0 } else { 1 },
                if cloned { "" } else { "fake gh: clone failed\n" },
            )
        }
        // The directory a repo is created from stands in for it from then on.
        ["repo", "create", repo, "--private" | "--public", "--source", ".", "--push"] => {
            if !st.doc["repos"][*repo].is_null() {
                st.finish("", 1, "GraphQL: Name already exists on this account\n")
            }
            let (repo, here) = (repo.to_string(), std::env::current_dir().unwrap());
            if !st.doc["repos"].is_object() {
                st.doc["repos"] = json!({});
            }
            st.doc["repos"][&repo] = json!(here);
            st.finish(&format!("https://github.com/{repo}"), 0, "")
        }
        _ => unsupported(&st),
    }
}

/// The state of the tracker the call names (`trackers`) — an endpoint under
/// it, or the repo itself — with the call as though it named this repo; this
/// one's where it names none.
fn tracked(st: State) -> State {
    let named = st.doc["trackers"].as_object().into_iter().flatten().find(|(repo, _)| {
        let (owner, name) = repo.split_once('/').unwrap();
        let endpoint = format!("repos/{repo}/");
        st.argv.iter().any(|arg| arg.starts_with(&endpoint) || *arg == format!("repos/{repo}"))
            || st.opt("--repo") == Some(repo.as_str())
            || st.argv.contains(&format!("owner={owner}"))
                && st.argv.contains(&format!("repo={name}"))
    });
    let Some((repo, file)) = named.map(|(repo, file)| (repo.clone(), file.clone())) else {
        return st;
    };
    st.save();
    drop(st);
    let mut there = State::at(PathBuf::from(file.as_str().unwrap()));
    there.argv.retain(|arg| arg != "--include");
    let (owner, name) = repo.split_once('/').unwrap();
    let (endpoint, owner, name) =
        (format!("repos/{repo}/"), format!("owner={owner}"), format!("repo={name}"));
    let mut argv = vec![];
    let mut args = there.argv.iter();
    while let Some(arg) = args.next() {
        if arg == "--repo" {
            args.next();
            continue;
        }
        argv.push(match arg {
            arg if *arg == owner => "owner={owner}".to_string(),
            arg if *arg == name => "repo={repo}".to_string(),
            arg if *arg == endpoint.trim_end_matches('/') => REPO.trim_end_matches('/').to_string(),
            arg => arg.replacen(&endpoint, REPO, 1),
        });
    }
    // a query names a repo by `-f`; this repo it names by `-F`
    for at in 1..argv.len() {
        if argv[at - 1] == "-f" && ["owner={owner}", "repo={repo}"].contains(&argv[at].as_str()) {
            argv[at - 1] = "-F".to_string();
        }
    }
    there.argv = argv;
    there
}

fn unsupported(st: &State) -> ! {
    st.finish("", 1, &format!("fake gh: unsupported call {:?}\n", st.argv))
}

/// Whether `issue` carries `label`: GitHub tells labels apart by their
/// letters, not their case.
fn carries(issue: &Value, label: &str) -> bool {
    let labels = issue["labels"].as_array().into_iter().flatten();
    labels.filter_map(Value::as_str).any(|had| had.eq_ignore_ascii_case(label))
}

fn is_open(row: &Value) -> bool {
    row["state"].as_str().unwrap_or("open") == "open"
}

fn is_fork(row: &Value) -> bool {
    row["fork"].as_bool().unwrap_or(false)
}

/// Where the branch of a PR from the repo itself points now in the bare
/// repo: empty where it is gone, and for a fork's, which it does not hold.
fn branch_head(pr: &Value, bare: &str) -> String {
    if is_fork(pr) {
        return String::new();
    }
    let branch = format!("refs/heads/{}", pr["headRefName"].as_str().unwrap());
    fake::git(&["--git-dir", bare, "rev-parse", "-q", "--verify", &branch])
}

/// GitHub marks a PR merged by itself once its head is on the base branch —
/// except the PRs listed under `slow_merge`, which it has not got round to.
fn notice_merges(st: &mut State) {
    let bare = st.doc["bare"].as_str().unwrap().to_string();
    let default_base = st.doc["base"].as_str().unwrap().to_string();
    let slow = st.doc["slow_merge"].as_array().cloned().unwrap_or_default();
    for pr in st.doc["prs"].as_array_mut().unwrap() {
        if !is_open(pr) || slow.contains(&pr["number"]) {
            continue;
        }
        let base = format!("refs/heads/{}", pr["baseRefName"].as_str().unwrap_or(&default_base));
        let head = branch_head(pr, &bare);
        let landed = !head.is_empty()
            && fake::ok(&["--git-dir", &bare, "merge-base", "--is-ancestor", &head, &base]);
        if landed {
            pr["state"] = json!("merged");
            pr["headRefOid"] = json!(head);
        }
    }
}

fn pr_list(st: &mut State) -> ! {
    if st.opt("--state") == Some("merged") {
        // The merged PRs carrying a label, as a cut asks for them: their numbers.
        assert_eq!(st.opt("--jq"), Some(".[].number"), "fake gh: unsupported jq");
        let label = json!(st.opt("--label").expect("a label"));
        notice_merges(st);
        let rows = st.doc["prs"].as_array().unwrap().iter();
        let merged = rows.filter(|pr| pr["state"] == "merged");
        let labelled =
            merged.filter(|pr| pr["labels"].as_array().is_some_and(|l| l.contains(&label)));
        let numbers: Vec<String> = labelled.map(|pr| pr["number"].to_string()).collect();
        st.finish(&numbers.join("\n"), 0, "")
    }
    assert_eq!(st.opt("--state"), Some("open"), "{:?}", st.argv);
    assert_eq!(st.opt("--json"), Some(PR_FIELDS), "{:?}", st.argv);
    notice_merges(st);
    let bare = st.doc["bare"].as_str().unwrap();
    let limit = st.opt("--limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1000);
    let rows: Vec<Value> = st.doc["prs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pr| is_open(pr))
        .take(limit)
        .map(|pr| {
            // A PR's head is wherever its branch points now, as on GitHub.
            let head = branch_head(pr, bare);
            let head = if head.is_empty() { pr["headRefOid"].clone() } else { json!(head) };
            let linked: Vec<Value> = pr["linked"]
                .as_array()
                .map(|numbers| {
                    numbers
                        .iter()
                        .take(100)
                        .map(|n| if n.is_object() { n.clone() } else { json!({"number": n}) })
                        .collect()
                })
                .unwrap_or_default();
            json!({"number": pr["number"], "title": pr["title"], "headRefName": pr["headRefName"],
                "headRefOid": head, "baseRefName": pr["baseRefName"].as_str().unwrap_or(st.doc["base"].as_str().unwrap()), "body": pr["body"], "closingIssuesReferences": linked,
                "isCrossRepository": is_fork(pr)})
        })
        .collect();
    st.finish(&Value::from(rows).to_string(), 0, "")
}

/// `gh pr <verb> … --repo <repo>`: the PRs of another repo.
fn pr_of(st: &mut State, verb: &str, repo: &str) -> ! {
    let Some(from) = st.doc["repos"][repo].as_str().map(str::to_string) else {
        st.finish("", 1, "GraphQL: Could not resolve to a Repository\n")
    };
    if verb == "list" {
        assert_eq!(st.opt("--state"), Some("open"), "{:?}", st.argv);
        assert_eq!(st.opt("--json"), Some(PR_OF_FIELDS), "{:?}", st.argv);
        assert_eq!(st.opt("--jq"), Some(PR_OF_JQ), "{:?}", st.argv);
        let rows = st.doc["prs_of"][repo].as_array().cloned().unwrap_or_default();
        let open: Vec<Value> = rows
            .iter()
            .filter(|pr| is_open(pr))
            .map(|pr| {
                json!({"number": pr["number"], "headRefName": pr["headRefName"],
                    "isCrossRepository": is_fork(pr), "since": pr["since"]})
            })
            .collect();
        st.finish(&Value::from(open).to_string(), 0, "")
    }
    let wanted: Option<u64> = st.argv.get(2).and_then(|number| number.parse().ok());
    let comment = st.opt("--comment").or(st.opt("--body")).map(str::to_string);
    let rows = st.doc["prs_of"][repo].as_array_mut();
    let found = rows.and_then(|rows| rows.iter_mut().find(|pr| wanted == pr["number"].as_u64()));
    let Some(pr) = found.filter(|pr| is_open(pr)) else {
        st.finish("", 1, "gh: no open pull request found\n")
    };
    match verb {
        "view" => {
            let bodies: Vec<String> = pr["comments"]
                .as_array()
                .map(|rows| {
                    rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect()
                })
                .unwrap_or_default();
            st.finish(&bodies.join("\n"), 0, "")
        }
        "comment" => {
            if !pr["comments"].is_array() {
                pr["comments"] = json!([]);
            }
            pr["comments"].as_array_mut().unwrap().push(json!(comment.unwrap()));
            st.finish("", 0, "")
        }

        "diff" => {
            let against = format!("HEAD...{}", pr["headRefName"].as_str().unwrap());
            let diff = fake::git(&["-C", &from, "diff", &against]);
            st.finish(&diff, 0, "")
        }
        "close" => {
            pr["state"] = json!("closed");
            pr["closed_with"] = json!(comment);
            st.finish("", 0, "")
        }
        _ => unsupported(st),
    }
}

/// Opens a local branch's PR, as a cut does, with the real pushed head.
fn pr_create(st: &mut State) -> ! {
    let head = st.opt("--head").expect("a PR has a head").to_string();
    let base = st.opt("--base").expect("a PR has a base").to_string();
    let title = st.opt("--title").expect("a PR has a title").to_string();
    let body = st.opt("--body").expect("a PR has a body").to_string();
    let bare = st.doc["bare"].as_str().unwrap();
    let commit =
        fake::git(&["--git-dir", bare, "rev-parse", "--verify", &format!("refs/heads/{head}")]);
    if commit.is_empty() {
        st.finish("", 1, "gh: head branch does not exist\n");
    }
    if st.doc["prs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|pr| is_open(pr) && pr["headRefName"] == head && pr["baseRefName"] == base)
    {
        st.finish("", 1, "a pull request already exists for that head and base\n");
    }
    let number = ["issues", "prs"]
        .into_iter()
        .flat_map(|key| st.doc[key].as_array().into_iter().flatten())
        .filter_map(|row| row["number"].as_u64())
        .max()
        .unwrap_or(0)
        + 1;
    let draft = st.has("--draft");
    st.list("prs").push(json!({"number": number, "headRefName": head,
        "headRefOid": commit, "baseRefName": base, "title": title, "body": body,
        "state": "open", "labels": [], "draft": draft}));
    if st.doc["lose_pr_create_receipt"] == true {
        st.doc["lose_pr_create_receipt"] = json!(false);
        st.finish("", 1, "connection lost after PR creation\n");
    }
    st.finish(&format!("https://gh/pull/{number}"), 0, "")
}

fn pr_edit(st: &mut State, number: &str) -> ! {
    let body = st.opt("--body").map(str::to_string);
    let wanted: u64 = number.parse().unwrap();
    let rows = st.doc["prs"].as_array_mut().unwrap();
    let Some(pr) = rows.iter_mut().find(|pr| pr["number"] == wanted) else {
        st.finish("", 1, "gh: no pull request found\n")
    };
    if let Some(body) = body {
        pr["body"] = json!(body);
    }
    st.finish("", 0, "")
}

fn pr_ready(st: &mut State, number: &str) -> ! {
    let undo = st.has("--undo");
    let wanted: u64 = number.parse().unwrap();
    let rows = st.doc["prs"].as_array_mut().unwrap();
    let Some(pr) = rows.iter_mut().find(|pr| pr["number"] == wanted && is_open(pr)) else {
        st.finish("", 1, "gh: no open pull request found\n")
    };
    pr["draft"] = json!(undo);
    st.finish("", 0, "")
}

fn pr_close(st: &mut State, number: &str) -> ! {
    let comment = st.opt("--comment").map(str::to_string);
    let wanted: u64 = number.parse().unwrap();
    let rows = st.doc["prs"].as_array_mut().unwrap();
    let Some(pr) = rows.iter_mut().find(|pr| pr["number"] == wanted && is_open(pr)) else {
        st.finish("", 1, "gh: no open pull request found\n")
    };
    pr["state"] = json!("closed");
    if let Some(body) = comment {
        let id = next_comment_id(st);
        let rows = &mut st.doc["comments"][number];
        if !rows.is_array() {
            *rows = json!([]);
        }
        rows.as_array_mut().unwrap().push(json!({"id": id, "body": body,
            "html_url": format!("https://gh/c/{id}")}));
    }
    st.finish("", 0, "")
}

/// The labels the repo has: those under `labels`, and — GitHub creating a
/// label the API puts on an issue — every one an issue carries.
fn repo_labels(st: &State) -> Vec<String> {
    let made = st.doc["labels"].as_array().into_iter().flatten();
    let issues = st.doc["issues"].as_array().into_iter().flatten();
    let carried = issues.flat_map(|issue| issue["labels"].as_array().into_iter().flatten());
    let mut names: Vec<String> = vec![];
    for name in made.chain(carried).filter_map(Value::as_str) {
        if !names.iter().any(|had| had.eq_ignore_ascii_case(name)) {
            names.push(name.to_string());
        }
    }
    names
}

/// `gh issue create`: unlike the API, it refuses a label the repo lacks.
fn issue_create(st: &mut State) -> ! {
    let title = st.opt("--title").expect("an issue has a title").to_string();
    let labels = st.opts("--label");
    let has = repo_labels(st);
    if let Some(lacked) = labels.iter().find(|label| !has.iter().any(|had| had == *label)) {
        st.finish("", 1, &format!("could not add label: '{lacked}' not found\n"))
    }
    let rows = st.doc["issues"].as_array().unwrap();
    let number = rows.iter().filter_map(|row| row["number"].as_u64()).max().unwrap_or(0) + 1;
    let issue = json!({"number": number, "title": title, "labels": labels, "state": "open"});
    st.doc["issues"].as_array_mut().unwrap().push(issue);
    st.finish(&format!("https://gh/issues/{number}"), 0, "")
}

fn api(st: &mut State) -> ! {
    let own = format!("repos/{}/", st.doc["repository"].as_str().unwrap_or("acme/dev"));
    let Some(endpoint) = st
        .argv
        .iter()
        .find_map(|a| a.strip_prefix(REPO).or_else(|| a.strip_prefix(&own)))
        .map(str::to_string)
    else {
        st.finish("", 1, "gh: Not Found (HTTP 404)\n")
    };
    let endpoint = endpoint.as_str();
    let method = st.opt("--method").unwrap_or("GET").to_string();
    let body = st.argv.iter().find_map(|a| a.strip_prefix("body=")).map(str::to_string);
    let (path, query) = endpoint.split_once('?').unwrap_or((endpoint, ""));
    let parts: Vec<&str> = path.split('/').collect();
    if let ["issues", number, ..] = parts.as_slice() {
        for (key, said) in [
            ("deleted", "gh: This issue was deleted (HTTP 410)\n"),
            ("transferred", "gh: Not Found (HTTP 404)\n"),
            ("missing", "gh: Not Found (HTTP 404)\n"),
        ] {
            let Some(left) = st.doc.get_mut(key).and_then(|calls| calls.get_mut(*number)) else {
                continue;
            };
            match left.as_u64().unwrap_or(0) {
                0 if key == "transferred" && parts.len() == 2 && method == "GET" => {
                    st.finish(&moved(number).to_string(), 0, "")
                }
                0 => st.finish("", 1, said),
                more => *left = json!(more - 1),
            }
        }
    }
    let jq = st.opt("--jq").map(str::to_string);
    let jq = jq.as_deref();
    if jq.is_none()
        && method == "GET"
        && let ["issues", number] = parts.as_slice()
    {
        let Some(row) = issue(st, number) else { st.finish("", 1, "gh: Not Found (HTTP 404)") };
        st.finish(&rest_issue(row).to_string(), 0, "");
    }
    if jq.is_none()
        && method == "GET"
        && (path == "issues"
            || path == "pulls"
            || path.ends_with("/comments")
            || path.ends_with("/blocked_by"))
    {
        equivalent_rest(st, path, query)
    }
    match parts.as_slice() {
        ["issues"] if method == "GET" && jq.is_some_and(|jq| jq.contains("created_at")) => {
            let rows: Vec<_> = st.doc["issues"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|issue| is_open(issue) && issue["pull_request"] != true)
                .map(|i| {
                    json!({"number": i["number"], "body": i["body"].as_str().unwrap_or(""),
                    "labels": i["labels"], "created": i["at"].as_u64().unwrap_or(0)})
                    .to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["issues", number, "timeline"] => {
            let rows = st.doc["timelines"][*number].as_array().cloned().unwrap_or_default();
            st.finish(&rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n"), 0, "")
        }
        ["pulls"] if query.contains("state=open") => {
            assert!(st.has("--paginate"));
            assert_eq!(jq, Some(PR_HEADS_JQ));
            notice_merges(st);
            let bare = st.doc["bare"].as_str().unwrap();
            let rows: Vec<String> = st.doc["prs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|pr| is_open(pr) && !is_fork(pr))
                .map(|pr| {
                    let head = branch_head(pr, bare);
                    json!({"number":pr["number"], "title":pr["title"].as_str().unwrap_or(""), "body":pr["body"].as_str().unwrap_or(""),
                        "head_branch":pr["headRefName"], "head_sha":head, "base_branch":pr["baseRefName"].as_str().unwrap_or(st.doc["base"].as_str().unwrap())})
                    .to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["issues"] if method == "GET" && query.contains("state=open") => {
            assert!(st.has("--paginate"));
            assert_eq!(jq, Some(OPEN_LABELLED_JQ));
            let labels = query.split('&').find_map(|field| field.strip_prefix("labels="));
            let labels = unescaped(labels.expect("the open issues of one label"));
            let rows: Vec<String> = st.doc["issues"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|issue| is_open(issue) && issue["pull_request"] != true)
                .filter(|issue| labels.split(',').all(|label| carries(issue, label)))
                .map(|issue| {
                    json!({"number":issue["number"], "labels":issue["labels"]}).to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["labels"] if method == "POST" => {
            let name = st.argv.iter().find_map(|a| a.strip_prefix("name=")).expect("a name");
            let name = name.to_string();
            if repo_labels(st).iter().any(|had| had.eq_ignore_ascii_case(&name)) {
                st.finish("", 1, "gh: Validation Failed (HTTP 422)\n")
            }
            st.list("labels").push(json!(name));
            st.finish("", 0, "")
        }
        ["labels"] => {
            assert_eq!(jq, Some(LABELS_JQ), "fake gh: unsupported jq");
            assert!(st.has("--paginate"), "{:?}", st.argv);
            st.finish(&repo_labels(st).join("\n"), 0, "")
        }
        ["issues"] if method == "POST" => {
            let field = |name: &str| {
                let prefix = format!("{name}=");
                st.argv.iter().find_map(|a| a.strip_prefix(&prefix)).map(str::to_string)
            };
            let title = field("title").expect("an issue has a title");
            let labels: Vec<&str> =
                st.argv.iter().filter_map(|a| a.strip_prefix("labels[]=")).collect();
            let rows = st.doc["issues"].as_array().unwrap();
            let number =
                rows.iter().filter_map(|row| row["number"].as_u64()).max().unwrap_or(0) + 1;
            let issue = json!({"number": number, "title": title, "labels": labels,
                "state": "open", "body": body});
            st.doc["issues"].as_array_mut().unwrap().push(issue);
            st.finish(&number.to_string(), 0, "")
        }
        ["issues"] if query.contains("state=closed") => {
            assert!(st.has("--paginate"), "{:?}", st.argv);
            assert_eq!(jq, Some(CLOSED_JQ), "fake gh: unsupported jq");
            let labels = query.split('&').find_map(|field| field.strip_prefix("labels="));
            let labels = unescaped(labels.expect("the closed issues of one label"));
            let rows: Vec<String> = st.doc["issues"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|issue| !is_open(issue))
                .filter(|issue| labels.split(',').all(|label| carries(issue, label)))
                .map(|issue| {
                    json!({"number": issue["number"], "at": issue["closed_at"]}).to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["issues"] if query.contains("state=all") => {
            assert!(st.has("--paginate"), "{:?}", st.argv);
            assert_eq!(jq, Some(CARRYING_JQ), "fake gh: unsupported jq");
            let labels = query.split('&').find_map(|field| field.strip_prefix("labels="));
            let labels = unescaped(labels.expect("the issues of one label"));
            let rows: Vec<String> = st.doc["issues"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|issue| labels.split(',').all(|label| carries(issue, label)))
                .map(|issue| {
                    json!({"number": issue["number"], "open": is_open(issue),
                        "closed_at": issue["closed_at"]})
                    .to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["issues", "comments", id] if method == "PATCH" => {
            let body = body.expect("an edit has a body");
            let id: u64 = id.parse().unwrap();
            let every = st.doc["comments"].as_object_mut().unwrap().values_mut();
            let mut rows = every.flat_map(|rows| rows.as_array_mut().unwrap().iter_mut());
            let Some(comment) = rows.find(|comment| comment["id"] == id) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            comment["body"] = json!(body);
            st.finish("", 0, "")
        }
        ["issues", number] if method == "PATCH" => {
            let state = st.argv.iter().find_map(|a| a.strip_prefix("state=")).map(str::to_string);
            let number: u64 = number.parse().unwrap();
            let rows = st.doc["issues"].as_array_mut().unwrap();
            let Some(issue) = rows.iter_mut().find(|issue| issue["number"] == number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            if let Some(state) = state {
                issue["state_reason"] =
                    if state == "closed" { json!("completed") } else { Value::Null };
                issue["state"] = json!(state);
            }
            st.finish("", 0, "")
        }
        ["branches", branch @ .., "protection"] => {
            // `protection`: {branch: the rules GitHub would answer with}, or a
            // string — the words of a read GitHub refuses.
            match &st.doc["protection"][branch.join("/")] {
                Value::Null => st.finish(
                    "{\"message\":\"Branch not protected\"}",
                    1,
                    "gh: Branch not protected (HTTP 404)\n",
                ),
                Value::String(refusal) => {
                    let refusal = format!("gh: {refusal}\n");
                    st.finish("", 1, &refusal)
                }
                rules => {
                    let rules = rules.to_string();
                    st.finish(&rules, 0, "")
                }
            }
        }
        ["issues", number] => {
            assert_eq!(jq, Some(ISSUE_JQ), "fake gh: unsupported jq");
            let Some(issue) = issue(st, number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let row = json!({"id": issue_id(issue), "title": issue["title"], "body": issue["body"],
                "state": issue["state"].as_str().unwrap_or("open"),
                "state_reason": issue["state_reason"], "labels": issue["labels"],
                "pull_request": issue["pull_request"].as_bool().unwrap_or(false),
                "closed_by": issue["closed_by"]});
            st.finish(&row.to_string(), 0, "")
        }
        ["issues", number, "dependencies", "blocked_by"] if method == "POST" => {
            let id = st.argv.iter().find_map(|a| a.strip_prefix("issue_id=")).expect("issue_id");
            let id: u64 = id.parse().unwrap();
            assert!(st.has("-F"), "an issue id is sent as a number: {:?}", st.argv);
            let rows = st.doc["issues"].as_array().unwrap();
            let Some(blocker) = rows.iter().find(|issue| issue_id(issue) == id) else {
                st.finish("", 1, "gh: Validation Failed (HTTP 422)\n")
            };
            let blocker = blocker["number"].clone();
            let edges = &mut st.doc["edges"][*number];
            if !edges.is_array() {
                *edges = json!([]);
            }
            if edges.as_array().unwrap().contains(&blocker) {
                st.finish("", 1, "gh: Validation Failed (HTTP 422)\n")
            }
            edges.as_array_mut().unwrap().push(blocker);
            st.finish("", 0, "")
        }
        ["issues", number, "dependencies", "blocked_by"] => {
            if jq == Some(".[]") {
                let rows: Vec<_> =
                    blockers(st, number).iter().map(rest_issue).map(|i| i.to_string()).collect();
                st.finish(&rows.join("\n"), 0, "")
            }
            assert_eq!(jq, Some(BLOCKED_BY_JQ), "fake gh: unsupported jq");
            assert!(st.has("--paginate"), "{:?}", st.argv);
            let lines: Vec<String> = blockers(st, number)
                .iter()
                .map(|blocker| {
                    let state = blocker["state"].as_str().unwrap_or("open");
                    json!({"number": blocker["number"], "state": state,
                        "repo": blocker["repo"], "labels": blocker["labels"]})
                    .to_string()
                })
                .collect();
            st.finish(&lines.join("\n"), 0, "")
        }
        ["issues", number, "labels"] if method == "POST" => {
            let added: Vec<String> = st
                .argv
                .iter()
                .filter_map(|a| a.strip_prefix("labels[]="))
                .map(str::to_string)
                .collect();
            let number: u64 = number.parse().unwrap();
            let rows = st.doc["issues"].as_array_mut().unwrap();
            let Some(issue) = rows.iter_mut().find(|issue| issue["number"] == number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let labels = issue["labels"].as_array_mut().unwrap();
            for label in added {
                if !labels.iter().any(|l| l == label.as_str()) {
                    labels.push(json!(label));
                }
            }
            st.finish("", 0, "")
        }
        ["issues", number, "labels", label] if method == "DELETE" => {
            let label = unescaped(label);
            let number: u64 = number.parse().unwrap();
            let rows = st.doc["issues"].as_array_mut().unwrap();
            let Some(issue) = rows.iter_mut().find(|issue| issue["number"] == number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let labels = issue["labels"].as_array_mut().unwrap();
            // as GitHub: taking off a label the issue does not carry is an error
            if !labels.iter().any(|l| l == label.as_str()) {
                st.finish("", 1, "gh: Label does not exist (HTTP 404)\n")
            }
            labels.retain(|l| l != label.as_str());
            st.finish("", 0, "")
        }
        ["issues", number, "comments"] if method == "POST" => {
            let body = body.expect("a comment has a body");
            // Hold a watch's final comment while its waiting worker
            // cools, without blocking the sampler's orca observations.
            if body.starts_with("<!--convoy:watch ")
                && let Some(release) = st.doc["comment_wait"].as_str()
            {
                std::fs::write(format!("{release}.waiting"), "").unwrap();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !std::path::Path::new(release).exists() {
                    assert!(std::time::Instant::now() < until, "comment never released");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            let id = next_comment_id(st);
            let url = format!("https://gh/c/{id}");
            let rows = &mut st.doc["comments"][*number];
            if !rows.is_array() {
                *rows = json!([]);
            }
            let at = st.doc["now"].clone();
            let rows = &mut st.doc["comments"][*number];
            rows.as_array_mut()
                .unwrap()
                .push(json!({"id": id, "body": body, "html_url": url, "at": at}));
            st.finish(&json!({"id": id, "html_url": url}).to_string(), 0, "")
        }
        ["issues", number, "comments"] => {
            assert_eq!(jq, Some(COMMENTS_JQ), "fake gh: unsupported jq");
            let lines: Vec<String> = comments(st, number).iter().map(Value::to_string).collect();
            st.finish(&lines.join("\n"), 0, "")
        }
        _ => unsupported(st),
    }
}

/// The comments on an issue or a PR, oldest first, as a read answers them.
fn comments(st: &State, number: &str) -> Vec<Value> {
    let rows = st.doc["comments"][number].as_array().cloned().unwrap_or_default();
    let read =
        |c: &Value| json!({"id": c["id"], "body": c["body"], "url": c["html_url"], "at": c["at"]});
    rows.iter().map(read).collect()
}

/// `gh api graphql`: the one query the fleet makes — every open issue, each
/// with the first `COMMENTS_WITH` of its comments.
fn open_issues(st: &State) -> ! {
    // `labels`, where the query names it: those carrying any of them.
    let labels: Vec<&str> = st.argv.iter().filter_map(|a| a.strip_prefix("labels[]=")).collect();
    let labelled = |issue: &&Value| labels.is_empty() || labels.iter().any(|l| carries(issue, l));
    if st.argv.contains(&format!("query={ACCOUNTS_QUERY}")) {
        assert_eq!(st.opt("--jq"), Some(ACCOUNTS_JQ));
        assert!(st.has("--paginate"));
        let rows: Vec<String> = st.doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(labelled)
            .map(|issue| {
                let mut with = comments(st, &issue["number"].to_string());
                let more = with.len() > COMMENTS_WITH;
                with.truncate(COMMENTS_WITH);
                json!({"number": issue["number"],
                    "url": format!("https://gh/issues/{}", issue["number"]),
                    "labels": issue["labels"], "more": more, "comments": with})
                .to_string()
            })
            .collect();
        st.finish(&rows.join("\n"), 0, "")
    }
    if st.argv.contains(&format!("query={WATCH_ISSUES_QUERY}")) {
        assert_eq!(st.opt("--jq"), Some(WATCH_ISSUES_JQ));
        assert!(st.has("--paginate"));
        let raw = st.argv.iter().find_map(|a| a.strip_prefix("since=")).unwrap();
        let since = afkconvoy::doctor::watching::since(raw).unwrap();
        let rows: Vec<String> = st.doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(labelled)
            .filter_map(|issue| {
                let mut with = comments(st, &issue["number"].to_string());
                let updated = with
                    .iter()
                    .filter_map(|c| c["at"].as_u64())
                    .chain(issue["closed_at"].as_u64())
                    .chain(issue["updated_at"].as_u64())
                    .max()
                    .unwrap_or(0);
                if updated < since {
                    return None;
                }
                let more = with.len() > COMMENTS_WITH;
                with.truncate(COMMENTS_WITH);
                Some(
                    json!({"number": issue["number"], "title": issue["title"],
                    "closed": issue["closed_at"], "labels": issue["labels"],
                    "more": more, "comments": with})
                    .to_string(),
                )
            })
            .collect();
        st.finish(&rows.join("\n"), 0, "")
    }
    if st.argv.contains(&format!("query={OPEN_AND_LANDED_QUERY}")) {
        assert_eq!(st.opt("--jq"), Some(OPEN_AND_LANDED_JQ));
        assert!(st.has("--paginate"));
        let open_rows: Vec<String> = st.doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|issue| is_open(issue))
            .filter(labelled)
            .map(|issue| {
                let number = issue["number"].to_string();
                let edges: Vec<Value> = blockers(st, &number)
                    .iter()
                    .map(|blocker| {
                        json!({"number": blocker["number"],
                        "state": blocker["state"].as_str().unwrap_or("open").to_uppercase(),
                        "repo": blocker["repo"], "labels": blocker["labels"]})
                    })
                    .collect();
                let mut with = comments(st, &number);
                let more = with.len() > COMMENTS_WITH;
                with.truncate(COMMENTS_WITH);
                json!({"part": "open", "number": issue["number"], "title": issue["title"],
                    "body": issue["body"].as_str().unwrap_or(""),
                    "url": format!("https://gh/i/{number}"), "at": issue["at"], "edited": issue["edited"],
                    "labels": issue["labels"],
                    "blockers": issue["blockers_at_rebuild"].as_array().unwrap_or(&edges),
                    "blockers_more": issue["blockers_more"].as_bool().unwrap_or(false),
                    "more": more, "comments": with})
                .to_string()
            })
            .collect();
        let closed: Vec<&Value> = st.doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|issue| !is_open(issue))
            .filter(labelled)
            .collect();
        let mut rows = open_rows;
        rows.extend(closed.iter().filter(|issue| issue["state_reason"] == "completed").map(
            |issue| {
                let mut with = comments(st, &issue["number"].to_string());
                let more = with.len() > COMMENTS_WITH;
                with.truncate(COMMENTS_WITH);
                json!({"part": "landed", "number": issue["number"], "at": issue["closed_at"],
                "more": more, "comments": with})
                .to_string()
            },
        ));
        // Every closed issue on the page, before the completed filter.
        rows.push(json!({"part": "landed_count", "n": closed.len()}).to_string());
        st.finish(&rows.join("\n"), 0, "")
    }
    if st.argv.contains(&format!("query={LANDED_ISSUES_QUERY}")) {
        assert_eq!(st.opt("--jq"), Some(LANDED_ISSUES_JQ));
        assert!(st.has("--paginate"));
        let rows: Vec<String> = st.doc["issues"].as_array().unwrap().iter()
            .filter(|issue| !is_open(issue) && issue["state_reason"] == "completed")
            .filter(labelled)
            .map(|issue| {
                let mut with = comments(st, &issue["number"].to_string());
                let more = with.len() > COMMENTS_WITH;
                with.truncate(COMMENTS_WITH);
                json!({"number": issue["number"], "at": issue["closed_at"], "more": more, "comments": with}).to_string()
            }).collect();
        st.finish(&rows.join("\n"), 0, "")
    }
    let query = format!("query={OPEN_ISSUES_QUERY}");

    assert!(st.argv.contains(&query), "fake gh: unsupported query {:?}", st.argv);
    assert_eq!(st.opt("--jq"), Some(OPEN_ISSUES_JQ), "fake gh: unsupported jq");
    assert!(st.has("--paginate"), "{:?}", st.argv);
    for filled in ["owner={owner}", "repo={repo}"] {
        assert!(st.argv.iter().any(|arg| arg == filled), "{:?}", st.argv);
    }
    let rows: Vec<String> = st.doc["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| is_open(issue))
        .filter(labelled)
        .map(|issue| {
            let number = issue["number"].to_string();
            let edges: Vec<Value> = blockers(st, &number)
                .iter()
                .map(|blocker| {
                    json!({"number": blocker["number"],
                    "state": blocker["state"].as_str().unwrap_or("open").to_uppercase(),
                    "repo": blocker["repo"], "labels": blocker["labels"]})
                })
                .collect();
            let mut with = comments(st, &number);
            let more = with.len() > COMMENTS_WITH;
            with.truncate(COMMENTS_WITH);
            json!({"number": issue["number"], "title": issue["title"],
                "body": issue["body"].as_str().unwrap_or(""),
                "url": format!("https://gh/i/{number}"), "at": issue["at"], "edited": issue["edited"],
                "labels": issue["labels"],
                "blockers": issue["blockers_at_rebuild"].as_array().unwrap_or(&edges),
                "blockers_more": issue["blockers_more"].as_bool().unwrap_or(false),
                "more": more, "comments": with})
            .to_string()
        })
        .collect();
    st.finish(&rows.join("\n"), 0, "")
}

/// A path segment with its `%XX` escapes read, as GitHub reads one.
fn unescaped(segment: &str) -> String {
    let mut out = vec![];
    let mut bytes = segment.bytes();
    while let Some(byte) = bytes.next() {
        let escaped = (byte == b'%').then(|| bytes.clone().take(2).collect::<Vec<u8>>());
        let read = escaped
            .and_then(|hex| String::from_utf8(hex).ok())
            .filter(|hex| hex.len() == 2)
            .and_then(|hex| u8::from_str_radix(&hex, 16).ok());
        match read {
            Some(read) => {
                out.push(read);
                bytes.nth(1);
            }
            None => out.push(byte),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn issue_id(issue: &Value) -> u64 {
    issue["id"].as_u64().unwrap_or(9000 + issue["number"].as_u64().unwrap())
}

/// The issues `number` is recorded as blocked by.
fn blockers(st: &State, number: &str) -> Vec<Value> {
    let named = st.doc["edges"][number].as_array().cloned().unwrap_or_default();
    let rows = st.doc["issues"].as_array().unwrap();
    named
        .iter()
        .filter_map(|n| {
            if n.is_object() {
                Some(n.clone())
            } else {
                rows.iter().find(|issue| issue["number"] == *n).cloned()
            }
        })
        .collect()
}

fn next_comment_id(st: &State) -> u64 {
    let every = st.doc["comments"].as_object().into_iter().flat_map(|all| all.values());
    let ids = every.flat_map(|rows| rows.as_array().into_iter().flatten());
    1 + ids.filter_map(|c| c["id"].as_u64()).max().unwrap_or(1000)
}

fn issue<'a>(st: &'a State, number: &str) -> Option<&'a Value> {
    let number: u64 = number.parse().ok()?;
    st.doc["issues"].as_array().unwrap().iter().find(|issue| issue["number"] == number)
}

fn stamp(at: &Value) -> Value {
    at.as_u64().map_or(Value::Null, |at| json!(afkconvoy::log::stamp(at)))
}
/// A read of issue `number`, transferred: `gh` follows GitHub's redirect,
/// and answers with the issue as it is in the repo it moved to.
fn moved(number: &str) -> Value {
    let to = number.parse::<u64>().unwrap() + 1000;
    json!({"id": to, "url": format!("https://api.github.com/repos/acme/elsewhere/issues/{to}"),
        "number": to, "title": "moved", "body": null, "state": "open", "state_reason": null,
        "labels": [], "pull_request": false, "closed_by": null})
}

fn rest_issue(row: &Value) -> Value {
    let labels: Vec<_> =
        row["labels"].as_array().into_iter().flatten().map(|n| json!({"name":n})).collect();
    json!({"id":issue_id(row),"number":row["number"],"title":row["title"],"body":row["body"],
        "labels":labels,"state":if is_open(row){"open"}else{"closed"},"state_reason":row["state_reason"],
        "created_at":stamp(&row["at"]),"closed_at":stamp(&row["closed_at"]),
        "html_url":format!("https://gh/i/{}",row["number"]),
        "repository_url":format!("https://api.github.com/repos/{}",row["repo"].as_str().unwrap_or("acme/dev")),
        "pull_request":if row["pull_request"]==true {json!({})}else{Value::Null}})
}
fn equivalent_rest(st: &mut State, path: &str, query: &str) -> ! {
    assert!(st.has("--paginate"));
    let rows = if path == "issues" {
        let labels = query
            .split('&')
            .find_map(|p| p.strip_prefix("labels="))
            .map(unescaped)
            .unwrap_or_default();
        st.doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                (!query.contains("state=open") || is_open(r))
                    && (!query.contains("state=closed") || !is_open(r))
                    && labels.split(',').filter(|l| !l.is_empty()).all(|l| carries(r, l))
            })
            .map(rest_issue)
            .collect::<Vec<_>>()
    } else if path == "pulls" {
        notice_merges(st);
        st.doc["prs"].as_array().unwrap().iter().filter(|r| {
            if query.contains("state=open") {is_open(r)}else{!is_open(r)}
        }).map(|r|json!({"number":r["number"],"title":r["title"],"body":r["body"],"created_at":stamp(&r["since"]),"merged_at":stamp(&r["merged_at"]),"labels":[],
            "head":{"ref":r["headRefName"],"sha":r["headRefOid"],"repo":{"full_name":if is_fork(r){"fork/dev"}else{"acme/dev"}}},"base":{"ref":r["baseRefName"].as_str().unwrap_or(st.doc["base"].as_str().unwrap()),"repo":{"full_name":"acme/dev"}}})).collect()
    } else if path.ends_with("/blocked_by") {
        let number = path.split('/').nth(1).unwrap();
        blockers(st, number).iter().map(rest_issue).collect()
    } else {
        let number = path.split('/').nth(1).unwrap();
        st.doc["comments"][number].as_array().into_iter().flatten().map(|r|json!({"id":r["id"],"body":r["body"],"html_url":r["html_url"],"created_at":stamp(&r["at"])})).collect()
    };
    // Multiple HTTP pages become multiple JSON arrays in gh api --paginate.
    let out = if rows.is_empty() {
        "[]".into()
    } else {
        rows.chunks(100).map(|p| json!(p).to_string()).collect::<Vec<_>>().join("\n")
    };
    st.finish(&out, 0, "")
}
fn graph_issue_row(row: &Value) -> Value {
    let labels: Vec<_> =
        row["labels"].as_array().into_iter().flatten().map(|n| json!({"name":n})).collect();
    json!({"id":format!("issue-{}",row["number"]),"__typename":if row["pull_request"]==true{"PullRequest"}else{"Issue"},"fullDatabaseId":issue_id(row).to_string(),
        "number":row["number"],"title":row["title"],"body":row["body"],"url":format!("https://gh/i/{}",row["number"]),
        "state":if is_open(row){"OPEN"}else{"CLOSED"},"stateReason":row["state_reason"].as_str().map(str::to_uppercase),
        "createdAt":stamp(&row["at"]),"closedAt":stamp(&row["closed_at"]),
        "repository":{"nameWithOwner":row["repo"].as_str().unwrap_or("acme/dev")},
        "labels":{"nodes":labels,"pageInfo":{"hasNextPage":false,"endCursor":null}}})
}
fn equivalent_graph(st: &State) -> ! {
    let query = st.argv.iter().find_map(|a| a.strip_prefix("query=")).unwrap();
    let number = query
        .split("number:")
        .nth(1)
        .and_then(|s| s.split(')').next())
        .and_then(|s| s.parse::<u64>().ok());
    let mut resource = if query.contains("resource: pullRequests(") {
        let rows:Vec<_>=st.doc["prs"].as_array().unwrap().iter().filter(|r|!query.contains("states:[OPEN]")||is_open(r)).map(|r|{
            let links:Vec<_>=r["linked"].as_array().into_iter().flatten().map(|n|if n.is_object(){n.clone()}else{json!({"number":n})}).collect();
            json!({"number":r["number"],"title":r["title"],"body":r["body"],"headRefName":r["headRefName"],"headRefOid":r["headRefOid"],"baseRefName":r["baseRefName"].as_str().unwrap_or(st.doc["base"].as_str().unwrap()),"isCrossRepository":is_fork(r),
                "state":r["state"].as_str().unwrap_or("open").to_ascii_uppercase(), "isDraft":r["draft"].as_bool().unwrap_or(false), "url":format!("https://gh/pull/{}", r["number"]),
                "createdAt":stamp(&r["since"]),"updatedAt":stamp(&r["updated_at"]),"closedAt":stamp(&r["closed_at"]),"mergedAt":stamp(&r["merged_at"]),"baseRepository":{"nameWithOwner":r["of"].as_str().unwrap_or("acme/dev")},
                "closingIssuesReferences":{"nodes":links.iter().take(100).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":links.len()>100,"endCursor":if links.len()>100{json!("100")}else{Value::Null}}}})
        }).collect();
        json!({"nodes":rows})
    } else if query.contains("resource: closingIssuesReferences(") {
        let pr = st.doc["prs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["number"] == number.unwrap())
            .unwrap();
        let links: Vec<_> = pr["linked"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|n| if n.is_object() { n.clone() } else { json!({"number":n}) })
            .collect();
        json!({"nodes":links})
    } else if query.contains("resource: defaultBranchRef") {
        let owner = st.argv.iter().find_map(|a| a.strip_prefix("owner=")).unwrap();
        let repo = st.argv.iter().find_map(|a| a.strip_prefix("repo=")).unwrap();
        let named = format!("{owner}/{repo}");
        let from = st.doc["repos"][&named].as_str().unwrap();
        json!({"target":{"oid":fake::git(&["-C",from,"rev-parse","HEAD"])}})
    } else if query.contains("resource: id") {
        json!("repo-node")
    } else if query.contains("resource: ref(") {
        json!({"branchProtectionRule":null})
    } else if query.contains("resource: visibility") {
        json!(st.doc["visibility"].as_str().unwrap_or("private").to_uppercase())
    } else if query.contains("resource: issues(") {
        let rows: Vec<_> = st.doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                (!query.contains("states:[OPEN]") || is_open(r))
                    && (!query.contains("states:[CLOSED]") || !is_open(r))
            })
            .map(graph_issue_row)
            .collect();
        json!({"nodes":rows,"pageInfo":{"hasNextPage":false,"endCursor":null}})
    } else if query.contains("resource: labels(") {
        json!({"nodes":repo_labels(st).iter().map(|n|json!({"id":format!("label-{n}"),"name":n})).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":false,"endCursor":null}})
    } else if query.contains("resource: comments(") {
        let rows = comments(st, &number.unwrap().to_string());
        json!({"nodes":rows.iter().map(|r|json!({"fullDatabaseId":r["id"].as_u64().unwrap().to_string(),"body":r["body"],"url":r["url"],"createdAt":stamp(&r["at"])})).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":false,"endCursor":null}})
    } else if query.contains("resource: blockedBy(") {
        json!({"nodes":blockers(st,&number.unwrap().to_string()).iter().map(graph_issue_row).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":false,"endCursor":null}})
    } else if query.contains("resource: issueOrPullRequest(") {
        let Some(row) = issue(st, &number.unwrap().to_string()) else {
            st.finish("", 1, "gh: Not Found (HTTP 404)")
        };
        graph_issue_row(row)
    } else if query.contains("timelineItems") {
        let row = issue(st, &number.unwrap().to_string()).unwrap();
        json!({"timelineItems":{"nodes":[{"actor":{"login":row["closed_by"]}}]}})
    } else {
        unsupported(st)
    };
    if query.contains("$endCursor") {
        let after = st
            .argv
            .iter()
            .find_map(|a| a.strip_prefix("endCursor="))
            .map(|n| n.parse::<usize>().unwrap())
            .unwrap_or(0);
        let rows = resource["nodes"].as_array().expect("connection nodes");
        let more = rows.len() > after + 100;
        resource = json!({"nodes":rows.iter().skip(after).take(100).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":more,"endCursor":if more{json!((after+100).to_string())}else{Value::Null}}});
    }
    st.finish(&json!({"data":{"repository":{"resource":resource}}}).to_string(), 0, "")
}

/// Decode a JSON/GraphQL string literal; it can contain quotes and newlines.
fn input(query: &str, key: &str) -> Value {
    let after = query.split(&format!("{key}:")).nth(1).expect("mutation input");
    serde_json::Deserializer::from_str(after).into_iter::<Value>().next().unwrap().unwrap()
}
fn equivalent_write(st: &mut State) -> ! {
    let query = st.argv.iter().find_map(|a| a.strip_prefix("query=")).unwrap().to_string();
    let operation = query.split("Write { ").nth(1).unwrap().split('(').next().unwrap();
    let data = match operation {
        "addComment" => {
            let node = input(&query, "subjectId");
            let number = node.as_str().unwrap().strip_prefix("issue-").unwrap();
            let body = input(&query, "body");
            let id = next_comment_id(st);
            let at = st.doc["now"].clone();
            if !st.doc["comments"][number].is_array() {
                st.doc["comments"][number] = json!([])
            }
            st.doc["comments"][number]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":id,"body":body,"html_url":format!("https://gh/c/{id}"),"at":at}));
            json!({"commentEdge":{"node":{"id":id}}})
        }
        "closeIssue" => {
            let node = input(&query, "issueId");
            let number =
                node.as_str().unwrap().strip_prefix("issue-").unwrap().parse::<u64>().unwrap();
            let row = st.doc["issues"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["number"] == number)
                .unwrap();
            row["state"] = json!("closed");
            row["state_reason"] = json!("completed");
            json!({"issue":{"id":node}})
        }
        "addLabelsToLabelable" | "removeLabelsFromLabelable" => {
            let node = input(&query, "labelableId");
            let number =
                node.as_str().unwrap().strip_prefix("issue-").unwrap().parse::<u64>().unwrap();
            let ids = input(&query, "labelIds");
            let row = st.doc["issues"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["number"] == number)
                .unwrap();
            let labels = row["labels"].as_array_mut().unwrap();
            for id in ids.as_array().unwrap() {
                let name = json!(id.as_str().unwrap().strip_prefix("label-").unwrap());
                if operation.starts_with("add") {
                    if !labels.contains(&name) {
                        labels.push(name)
                    }
                } else {
                    labels.retain(|n| n != &name)
                }
            }
            json!({"labelable":{"__typename":"Issue"}})
        }
        "createIssue" => {
            let number = st.doc["issues"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|r| r["number"].as_u64())
                .max()
                .unwrap_or(0)
                + 1;
            let ids = input(&query, "labelIds");
            let labels: Vec<_> = ids
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().strip_prefix("label-").unwrap())
                .collect();
            st.doc["issues"].as_array_mut().unwrap().push(json!({"number":number,"title":input(&query,"title"),"body":input(&query,"body"),"state":"open","labels":labels}));
            json!({"issue":{"number":number}})
        }
        "createLabel" => {
            let name = input(&query, "name");
            st.list("labels").push(name);
            json!({"label":{"id":"label-node"}})
        }
        _ => unsupported(st),
    };
    if st.doc["write_receipt_lost"] == true {
        st.finish("", 1, "unexpected end of JSON input")
    }
    st.finish(&json!({"data":{operation:data}}).to_string(), 0, "")
}

fn identity_login(st: &State) -> String {
    std::env::var("GH_TOKEN")
        .ok()
        .and_then(|token| st.doc["identity_by_token"][&token].as_str().map(str::to_string))
        .or_else(|| st.doc["login"].as_str().map(str::to_string))
        .unwrap_or_else(|| "fixture-user".into())
}
