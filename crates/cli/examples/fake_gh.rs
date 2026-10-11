//! Stands in for `gh` in the tests: GitHub's issues, pull requests and comments
//! as one JSON file (`CONVOY_FAKE_GH`), the code in the bare repo it names.
//!
//! State: `issues` [{number, title, labels: [name…], state, state_reason?,
//! blocked_by?, id?}], `comments` {number: [{id, body, html_url}]}, `prs` [{number, headRefName, headRefOid, body, state,
//! linked?: [issue…]}], `bare`, `base`, `calls`, and `fail`: substrings of a
//! command line that make the call fail.

mod fake;

use afkconvoy::gh::{COMMENTS_JQ, ISSUE_JQ, ISSUES_JQ, PR_FIELDS};
use fake::State;
use serde_json::{Value, json};

const REPO: &str = "repos/{owner}/{repo}/";

fn main() {
    let mut st = State::open("CONVOY_FAKE_GH");
    let line = st.argv.join(" ");
    let injected = st.doc["fail"]
        .as_array()
        .is_some_and(|parts| parts.iter().filter_map(|p| p.as_str()).any(|p| line.contains(p)));
    if injected {
        st.finish("", 1, "fake gh: injected failure\n");
    }
    let argv: Vec<&str> = st.argv.iter().map(String::as_str).collect();
    match argv.as_slice() {
        ["pr", "list", ..] => pr_list(&st),
        ["api", ..] => api(&mut st),
        _ => unsupported(&st),
    }
}

fn unsupported(st: &State) -> ! {
    st.finish("", 1, &format!("fake gh: unsupported call {:?}\n", st.argv))
}

fn is_open(row: &Value) -> bool {
    row["state"].as_str().unwrap_or("open") == "open"
}

fn pr_list(st: &State) -> ! {
    assert_eq!(st.opt("--state"), Some("open"), "{:?}", st.argv);
    assert_eq!(st.opt("--json"), Some(PR_FIELDS), "{:?}", st.argv);
    let bare = st.doc["bare"].as_str().unwrap();
    let rows: Vec<Value> = st.doc["prs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pr| is_open(pr))
        .map(|pr| {
            // A PR's head is wherever its branch points now, as on GitHub.
            let branch = format!("refs/heads/{}", pr["headRefName"].as_str().unwrap());
            let head = fake::git(&["--git-dir", bare, "rev-parse", "-q", "--verify", &branch]);
            let head = if head.is_empty() { pr["headRefOid"].clone() } else { json!(head) };
            let linked: Vec<Value> = pr["linked"]
                .as_array()
                .map(|numbers| numbers.iter().map(|n| json!({"number": n})).collect())
                .unwrap_or_default();
            json!({"number": pr["number"], "title": pr["title"], "headRefName": pr["headRefName"], "headRefOid": head,
                "body": pr["body"], "closingIssuesReferences": linked})
        })
        .collect();
    st.finish(&Value::from(rows).to_string(), 0, "")
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
    let jq = st.opt("--jq").map(str::to_string);
    let jq = jq.as_deref();
    match parts.as_slice() {
        ["issues"] => {
            assert!(query.contains("state=open") && st.has("--paginate"), "{:?}", st.argv);
            assert_eq!(jq, Some(ISSUES_JQ), "fake gh: unsupported jq");
            let rows: Vec<String> = st.doc["issues"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|issue| is_open(issue))
                .map(|issue| {
                    json!({"number": issue["number"], "title": issue["title"],
                        "labels": issue["labels"],
                        "blocked_by": issue["blocked_by"].as_u64().unwrap_or(0)})
                    .to_string()
                })
                .collect();
            st.finish(&rows.join("\n"), 0, "")
        }
        ["issues", number] => {
            assert_eq!(jq, Some(ISSUE_JQ), "fake gh: unsupported jq");
            let Some(issue) = issue(st, number) else {
                st.finish("", 1, "gh: Not Found (HTTP 404)\n")
            };
            let id = issue["id"].as_u64().unwrap_or(9000 + issue["number"].as_u64().unwrap());
            let row = json!({"id": id, "title": issue["title"],
                "state": issue["state"].as_str().unwrap_or("open"),
                "state_reason": issue["state_reason"], "labels": issue["labels"],
                "pull_request": issue["pull_request"].as_bool().unwrap_or(false)});
            st.finish(&row.to_string(), 0, "")
        }
        ["issues", number, "comments"] if method == "POST" => {
            let body = body.expect("a comment has a body");
            let id = next_comment_id(st);
            let url = format!("https://gh/c/{id}");
            let rows = &mut st.doc["comments"][*number];
            if !rows.is_array() {
                *rows = json!([]);
            }
            rows.as_array_mut().unwrap().push(json!({"id": id, "body": body, "html_url": url}));
            st.finish(&json!({"id": id, "html_url": url}).to_string(), 0, "")
        }
        ["issues", number, "comments"] => {
            assert_eq!(jq, Some(COMMENTS_JQ), "fake gh: unsupported jq");
            let rows = st.doc["comments"][*number].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = rows
                .iter()
                .map(|c| {
                    json!({"id": c["id"], "body": c["body"], "url": c["html_url"]}).to_string()
                })
                .collect();
            st.finish(&lines.join("\n"), 0, "")
        }
        _ => unsupported(st),
    }
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
