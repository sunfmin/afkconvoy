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
//! `deleted` and `transferred` {number: how many more calls about it are
//! answered}: past those, each call about the issue answers 410 — or 404 —
//! as GitHub does for an issue deleted, or transferred to another repo.
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
    BLOCKED_BY_JQ, CARRYING_JQ, CLOSED_JQ, COMMENTS_JQ, COMMENTS_WITH, ISSUE_JQ, LABELS_JQ,
    LANDED_ISSUES_JQ, LANDED_ISSUES_QUERY, OPEN_ISSUES_JQ, OPEN_ISSUES_QUERY, PR_FIELDS,
    PR_OF_FIELDS, PR_OF_JQ, VISIBILITY_JQ, WATCH_ISSUES_JQ, WATCH_ISSUES_QUERY, WATCH_PRS_JQ,
};
use std::path::PathBuf;

use fake::State;
use serde_json::{Value, json};

const REPO: &str = "repos/{owner}/{repo}/";

fn main() {
    let mut st = State::open_routed("CONVOY_FAKE_GH", "convoy-fake-gh");
    let line = st.argv.join(" ");
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
                    "fork": is_fork(pr), "of": pr["of"]})
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
        ["api", endpoint, "--jq", ".sha"] if endpoint.ends_with("/commits/HEAD") => {
            let name = endpoint.trim_start_matches("repos/").trim_end_matches("/commits/HEAD");
            let Some(from) = st.doc["repos"][name].as_str() else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let head = fake::git(&["-C", from, "rev-parse", "HEAD"]);
            st.finish(&head, 0, "")
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
        ["api", "repos/{owner}/{repo}", "--jq", jq] if *jq == VISIBILITY_JQ => {
            let visibility = st.doc["visibility"].as_str().unwrap_or("private").to_string();
            st.finish(&visibility, 0, "")
        }
        ["api", "graphql", ..] => open_issues(&st),
        ["api", ..] => api(&mut st),
        ["issue", "create", ..] => issue_create(&mut st),
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
            || st.argv.contains(&format!("owner={owner}"))
                && st.argv.contains(&format!("repo={name}"))
    });
    let Some((repo, file)) = named.map(|(repo, file)| (repo.clone(), file.clone())) else {
        return st;
    };
    st.save();
    drop(st);
    let mut there = State::at(PathBuf::from(file.as_str().unwrap()));
    let (owner, name) = repo.split_once('/').unwrap();
    let (endpoint, owner, name) =
        (format!("repos/{repo}/"), format!("owner={owner}"), format!("repo={name}"));
    let mut argv = vec![];
    for arg in &there.argv {
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
    let base = format!("refs/heads/{}", st.doc["base"].as_str().unwrap());
    let slow = st.doc["slow_merge"].as_array().cloned().unwrap_or_default();
    for pr in st.doc["prs"].as_array_mut().unwrap() {
        if !is_open(pr) || slow.contains(&pr["number"]) {
            continue;
        }
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
    let rows: Vec<Value> = st.doc["prs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pr| is_open(pr))
        .map(|pr| {
            // A PR's head is wherever its branch points now, as on GitHub.
            let head = branch_head(pr, bare);
            let head = if head.is_empty() { pr["headRefOid"].clone() } else { json!(head) };
            let linked: Vec<Value> = pr["linked"]
                .as_array()
                .map(|numbers| numbers.iter().map(|n| json!({"number": n})).collect())
                .unwrap_or_default();
            json!({"number": pr["number"], "title": pr["title"], "headRefName": pr["headRefName"],
                "headRefOid": head, "body": pr["body"], "closingIssuesReferences": linked,
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
    let comment = st.opt("--comment").map(str::to_string);
    let rows = st.doc["prs_of"][repo].as_array_mut();
    let found = rows.and_then(|rows| rows.iter_mut().find(|pr| wanted == pr["number"].as_u64()));
    let Some(pr) = found.filter(|pr| is_open(pr)) else {
        st.finish("", 1, "gh: no open pull request found\n")
    };
    match verb {
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
    let number = ["issues", "prs"]
        .into_iter()
        .flat_map(|key| st.doc[key].as_array().into_iter().flatten())
        .filter_map(|row| row["number"].as_u64())
        .max()
        .unwrap_or(0)
        + 1;
    st.list("prs").push(json!({"number": number, "headRefName": head,
        "headRefOid": commit, "baseRefName": base, "title": title, "body": body,
        "state": "open", "labels": []}));
    st.finish(&format!("https://gh/pull/{number}"), 0, "")
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
    let Some(endpoint) = st.argv.iter().find_map(|a| a.strip_prefix(REPO)).map(str::to_string)
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
        ] {
            let Some(left) = st.doc.get_mut(key).and_then(|calls| calls.get_mut(*number)) else {
                continue;
            };
            match left.as_u64().unwrap_or(0) {
                0 => st.finish("", 1, said),
                more => *left = json!(more - 1),
            }
        }
    }
    let jq = st.opt("--jq").map(str::to_string);
    let jq = jq.as_deref();
    match parts.as_slice() {
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
            // Hold an inspection's final comment while its waiting worker
            // cools, without blocking the sampler's orca observations.
            if body.starts_with("<!--convoy:inspection ")
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
                "url": format!("https://gh/i/{number}"), "at": issue["at"],
                "labels": issue["labels"],
                "blockers": issue["blockers_at_rebuild"].as_array().unwrap_or(&edges),
                "blockers_more": false,
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
