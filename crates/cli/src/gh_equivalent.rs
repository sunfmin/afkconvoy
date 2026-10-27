//! Equivalent GitHub operations, used only after a primary-quota rejection.
//! The output is the original caller's projection. Unsupported facts fail
//! explicitly; these adapters never fabricate relationships or an empty read.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::gh::{self, GitHub};

const OWN: &str = "{owner}/{repo}";

fn opt<'a>(args: &'a [&str], flag: &str) -> Option<&'a str> {
    args.windows(2).find(|p| p[0] == flag).map(|p| p[1])
}
fn field<'a>(args: &'a [&str], name: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(name)?.strip_prefix('='))
}
fn pages(out: &str) -> Result<Vec<Value>> {
    let pages: Vec<Value> = serde_json::Deserializer::from_str(out)
        .into_iter()
        .collect::<std::result::Result<_, _>>()
        .context("GitHub equivalent: invalid paginated JSON")?;
    if pages.is_empty() {
        bail!("GitHub equivalent: missing collection receipt; cannot infer empty facts")
    }
    Ok(pages)
}
fn lines(rows: impl IntoIterator<Item = Value>) -> String {
    rows.into_iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n")
}
fn time(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|s| crate::doctor::watching::since(s).ok())
        .map_or(Value::Null, |t| json!(t))
}
fn names(labels: &Value) -> Result<Value> {
    let rows = labels.as_array().context("GitHub: missing labels")?;
    Ok(json!(
        rows.iter()
            .map(|l| l["name"].as_str().context("GitHub: label without name"))
            .collect::<Result<Vec<_>>>()?
    ))
}
fn database_id(value: &Value) -> Result<Value> {
    let id = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
        .filter(|id| *id > 0)
        .context("GitHub GraphQL: missing or invalid fullDatabaseId")?;
    Ok(json!(id))
}
fn list(gh: &GitHub, endpoint: &str) -> Result<Vec<Value>> {
    let out = gh.on_api(&["api", "--paginate", endpoint], false)?;
    let mut rows = vec![];
    for page in pages(&out)? {
        rows.extend(
            page.as_array().context("GitHub REST: expected a complete collection")?.iter().cloned(),
        );
    }
    Ok(rows)
}
fn rest_comments(gh: &GitHub, repo: &str, number: &Value) -> Result<Value> {
    Ok(json!(list(gh, &format!("repos/{repo}/issues/{number}/comments?per_page=100"))?
        .into_iter().map(|r| json!({"id":r["id"],"body":r["body"],"url":r["html_url"],"at":time(&r["created_at"])})).collect::<Vec<_>>()))
}
fn rest_blockers(gh: &GitHub, repo: &str, number: &Value) -> Result<Value> {
    let mut rows = vec![];
    for r in
        list(gh, &format!("repos/{repo}/issues/{number}/dependencies/blocked_by?per_page=100"))?
    {
        let repo = r["repository_url"]
            .as_str()
            .and_then(|s| s.split_once("/repos/").map(|(_, r)| r))
            .context("GitHub REST blocker missing repository identity")?;
        rows.push(json!({"number":r["number"],"state":r["state"],"repo":repo,"labels":names(&r["labels"])?}));
    }
    Ok(json!(rows))
}

/// Read one raw GraphQL selection, or a complete connection aliased `resource`.
/// Nested connections are completed separately; an absent/null resource is an
/// error, never an authoritative empty connection.
fn graph(gh: &GitHub, repo: &str, selection: &str, connection: bool) -> Result<Vec<Value>> {
    graph_read(gh, repo, selection, connection, false)
}

fn graph_read(
    gh: &GitHub,
    repo: &str,
    selection: &str,
    connection: bool,
    missing_blob: bool,
) -> Result<Vec<Value>> {
    let (owner, name) = repo.split_once('/').context("GitHub repo must be owner/repo")?;
    let cursor = if connection { ", $endCursor: String" } else { "" };
    let query = format!(
        "query ConvoyQuotaEquivalent($owner: String!, $repo: String!{cursor}) {{ repository(owner: $owner, name: $repo) {{ {selection} }} }}"
    );
    let owner = format!("owner={owner}");
    let name = format!("repo={name}");
    let query = format!("query={query}");
    let flag = if repo == OWN { "-F" } else { "-f" };
    let mut rows = vec![];
    let mut after = None::<String>;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let cursor = after.as_ref().map(|c| format!("endCursor={c}"));
        let mut args = vec!["api", "graphql", flag, &owner, flag, &name, "-f", &query];
        if let Some(cursor) = &cursor {
            args.extend(["-f", cursor]);
        }
        let out = gh.on_api(&args, false)?;
        let page: Value =
            serde_json::from_str(&out).context("GitHub GraphQL: invalid page receipt")?;
        if page.get("errors").is_some_and(|e| !e.is_null() && e != &json!([])) {
            bail!("GitHub GraphQL returned incomplete facts: {}", page["errors"]);
        }
        let repository = &page["data"]["repository"];
        let resource = repository
            .get("resource")
            .or_else(|| repository["issueOrPullRequest"].get("resource"))
            .or_else(|| repository["issue"].get("resource"))
            .or_else(|| repository["pullRequest"].get("resource"))
            .context("GitHub GraphQL: missing required resource")?;
        if resource.is_null() && !missing_blob {
            bail!("GitHub GraphQL: required resource unavailable (not an empty collection)");
        }
        if connection {
            rows.extend(
                resource["nodes"]
                    .as_array()
                    .context("GitHub GraphQL: missing connection nodes")?
                    .iter()
                    .cloned(),
            );
            let more = resource["pageInfo"]["hasNextPage"]
                .as_bool()
                .context("GitHub GraphQL: missing pagination evidence")?;
            if !more {
                break;
            }
            let cursor = resource["pageInfo"]["endCursor"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("GitHub GraphQL: next page missing cursor")?;
            if !seen.insert(cursor.to_string()) {
                bail!("GitHub GraphQL: pagination cursor repeated; refusing incomplete facts")
            }
            after = Some(cursor.to_string());
        } else {
            rows.push(resource.clone());
            break;
        }
    }
    Ok(rows)
}

const LABELS: &str = "labels(first:100) { nodes { name } pageInfo { hasNextPage endCursor } }";
const ISSUE: &str = "id number title body url fullDatabaseId state stateReason createdAt closedAt";
const COMMENT: &str = "fullDatabaseId body url createdAt";
const PR: &str = "number title body headRefName headRefOid baseRefName isCrossRepository state isDraft url createdAt updatedAt closedAt mergedAt baseRepository { nameWithOwner }";

fn graph_labels(gh: &GitHub, repo: &str, row: &Value) -> Result<Value> {
    let conn = &row["labels"];
    let labels = if conn["pageInfo"]["hasNextPage"] == true {
        let number = row["number"].as_u64().context("GitHub issue without number")?;
        graph(
            gh,
            repo,
            &format!(
                "issueOrPullRequest(number:{number}) {{ ... on Issue {{ resource: labels(first:100, after:$endCursor) {{ nodes {{ name }} pageInfo {{ hasNextPage endCursor }} }} }} ... on PullRequest {{ resource: labels(first:100, after:$endCursor) {{ nodes {{ name }} pageInfo {{ hasNextPage endCursor }} }} }} }}"
            ),
            true,
        )?
    } else {
        conn["nodes"].as_array().context("GitHub issue missing labels")?.clone()
    };
    names(&json!(labels))
}

fn graph_issue(gh: &GitHub, repo: &str, number: u64) -> Result<Value> {
    let issue = format!(
        "resource: issueOrPullRequest(number:{number}) {{ __typename ... on Issue {{ {ISSUE} {LABELS} }} ... on PullRequest {{ id number title body fullDatabaseId state {LABELS} }} }}"
    );
    graph(gh, repo, &issue, false)?.pop().context("GitHub: missing issue")
}
fn graph_comments(gh: &GitHub, repo: &str, number: u64) -> Result<Vec<Value>> {
    // The alias sits under issueOrPullRequest; flatten it at the query root
    // through the repository selection helper's nested-resource support.
    graph(
        gh,
        repo,
        &format!(
            "issueOrPullRequest(number:{number}) {{ ... on Issue {{ resource: comments(first:100, after:$endCursor) {{ nodes {{ {COMMENT} }} pageInfo {{ hasNextPage endCursor }} }} }} ... on PullRequest {{ resource: comments(first:100, after:$endCursor) {{ nodes {{ {COMMENT} }} pageInfo {{ hasNextPage endCursor }} }} }} }}"
        ),
        true,
    )
}
fn graph_blockers(gh: &GitHub, repo: &str, number: u64) -> Result<Vec<Value>> {
    graph(
        gh,
        repo,
        &format!(
            "issue(number:{number}) {{ resource: blockedBy(first:100, after:$endCursor) {{ nodes {{ number state repository {{ nameWithOwner }} {LABELS} }} pageInfo {{ hasNextPage endCursor }} }} }}"
        ),
        true,
    )
}

pub(crate) fn run(gh: &GitHub, args: &[&str], writing: bool) -> Result<String> {
    if writing {
        return write(gh, args);
    }
    if args.starts_with(&["api", "graphql"])
        || args.starts_with(&["pr", "list"])
        || args.starts_with(&["issue", "list"])
        || args.starts_with(&["issue", "view"])
    {
        return rest(gh, args);
    }
    graphql(gh, args)
}

fn rest(gh: &GitHub, args: &[&str]) -> Result<String> {
    if args.starts_with(&["issue", "view"]) && opt(args, "--json") == Some("url") {
        let number: u64 = args.get(2).context("missing issue number")?.parse()?;
        let repo = opt(args, "--repo").unwrap_or(OWN);
        let out = gh.on_api(&["api", &format!("repos/{repo}/issues/{number}")], false)?;
        let row: Value =
            serde_json::from_str(&out).context("GitHub REST: invalid issue receipt")?;
        let url = row["html_url"].as_str().context("GitHub REST: missing issue URL")?;
        return Ok(json!({"url":url}).to_string());
    }
    if args.starts_with(&["api", "graphql"]) {
        let owner = field(args, "owner").context("GitHub query missing owner")?;
        let name = field(args, "repo").context("GitHub query missing repo")?;
        let repo = format!("{owner}/{name}");
        let query = field(args, "query").unwrap_or("");
        let state = if query == gh::OPEN_ISSUES_QUERY {
            "open"
        } else if query == gh::LANDED_ISSUES_QUERY {
            "closed"
        } else if query == gh::WATCH_ISSUES_QUERY {
            "all"
        } else {
            bail!("operation has no supported equivalent REST query")
        };
        let mut endpoint = format!("repos/{repo}/issues?state={state}&per_page=100");
        let labels: Vec<_> = args
            .iter()
            .filter_map(|a| a.strip_prefix("labels[]="))
            .map(crate::judge::label_segment)
            .collect();
        if !labels.is_empty() {
            endpoint.push_str(&format!("&labels={}", labels.join(",")));
        }
        if let Some(since) = field(args, "since") {
            endpoint.push_str(&format!("&since={since}"));
        }
        let mut rows = vec![];
        for r in list(gh, &endpoint)? {
            if !r["pull_request"].is_null() {
                continue;
            }
            if state == "closed" && r["state_reason"] != "completed" {
                continue;
            }
            let mut row = json!({"number":r["number"],"title":r["title"],"body":r["body"].as_str().unwrap_or(""),"url":r["html_url"],"at":time(&r["created_at"]),"closed":time(&r["closed_at"]),"labels":names(&r["labels"])? ,"more":false,"comments":rest_comments(gh, &repo, &r["number"])?});
            if state == "open" {
                row["blockers"] = rest_blockers(gh, &repo, &r["number"])?;
                row["blockers_more"] = json!(false);
            }
            if state == "closed" {
                row["at"] = time(&r["closed_at"]);
            }
            rows.push(row);
        }
        return Ok(lines(rows));
    }
    let repo = opt(args, "--repo").unwrap_or(OWN);
    if args.starts_with(&["issue", "list"]) {
        let rows = list(gh, &format!("repos/{repo}/issues?state=open&per_page=100"))?;
        return Ok(json!(
            rows.into_iter()
                .filter(|r| r["pull_request"].is_null())
                .map(|r| json!({"number":r["number"],"body":r["body"].as_str().unwrap_or("")}))
                .collect::<Vec<_>>()
        )
        .to_string());
    }
    let merged = opt(args, "--state") == Some("merged");
    let state = if merged { "closed" } else { "open" };
    let rows = list(gh, &format!("repos/{repo}/pulls?state={state}&per_page=100"))?;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| {
            !r["head"]["repo"].is_null()
                && r["head"]["repo"]["full_name"] == r["base"]["repo"]["full_name"]
        })
        .collect();
    if opt(args, "--json") == Some(gh::PR_FIELDS) {
        if !rows.is_empty() {
            bail!(
                "open_prs: REST cannot supply native/manual PR closingIssuesReferences (including cross-tracker identities) for open PRs; rebuild/dispatch/ordinary issue landing/settlement wait for GraphQL reset; no body inference or empty relationship list is permitted"
            );
        }
        return Ok("[]".into());
    }
    if merged {
        let label = opt(args, "--label").context("merged PRs missing label")?;
        let mut numbers = vec![];
        for row in rows {
            if !row["merged_at"].is_null()
                && names(&row["labels"])?.as_array().is_some_and(|ls| ls.contains(&json!(label)))
            {
                numbers.push(row["number"].to_string());
            }
        }
        return Ok(numbers.join("\n"));
    }
    Ok(json!(rows.into_iter().map(|r| json!({"number":r["number"],"headRefName":r["head"]["ref"],"isCrossRepository":false,"since":time(&r["created_at"])})).collect::<Vec<_>>()).to_string())
}

fn graphql(gh: &GitHub, args: &[&str]) -> Result<String> {
    let endpoint = args
        .iter()
        .find(|s| s.starts_with("repos/"))
        .context("operation has no supported GraphQL equivalent")?;
    let mut parts = endpoint.trim_start_matches("repos/").splitn(3, '/');
    let repo = format!(
        "{}/{}",
        parts.next().context("missing owner")?,
        parts.next().context("missing repo")?
    );
    let rest = parts.next().unwrap_or("");
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let pieces: Vec<_> = path.split('/').collect();
    let jq = opt(args, "--jq");
    if path.is_empty() && jq == Some(gh::VISIBILITY_JQ) {
        let r = graph(gh, &repo, "resource: visibility", false)?
            .pop()
            .context("GitHub: missing visibility")?;
        return Ok(r.as_str().context("GitHub: invalid visibility")?.to_ascii_lowercase());
    }
    if path == "issues" {
        let state = parameter(query, "state").unwrap_or("open");
        let states = match state {
            "open" => "[OPEN]",
            "closed" => "[CLOSED]",
            _ => "[OPEN,CLOSED]",
        };
        let labels = parameter(query, "labels").map(decode).transpose()?.unwrap_or_default();
        let labels = json!(labels.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>());
        let selection = format!(
            "resource: issues(states:{states}, labels:{labels}, first:100, after:$endCursor) {{ nodes {{ {ISSUE} {LABELS} }} pageInfo {{ hasNextPage endCursor }} }}"
        );
        let mut rows = vec![];
        for r in graph(gh, &repo, &selection, true)? {
            rows.push(match jq {
                Some(gh::OPEN_LABELLED_JQ) => json!({"number":r["number"],"labels":graph_labels(gh,&repo,&r)?}),
                Some(gh::CLOSED_JQ) => json!({"number":r["number"],"at":time(&r["closedAt"])}),
                Some(gh::CARRYING_JQ) => json!({"number":r["number"],"open":r["state"]=="OPEN","closed_at":time(&r["closedAt"])}),
                _ => bail!("issue listing projection has no supported GraphQL equivalent"),
            });
        }
        return Ok(lines(rows));
    }
    if path == "labels" {
        return Ok(graph(gh, &repo, "resource: labels(first:100, after:$endCursor) { nodes { name } pageInfo { hasNextPage endCursor } }", true)?.into_iter().map(|r| r["name"].as_str().context("GitHub: missing label name").map(str::to_string)).collect::<Result<Vec<_>>>()?.join("\n"));
    }
    if let ["issues", number, tail @ ..] = pieces.as_slice() {
        let number: u64 = number.parse().context("GitHub: invalid issue number")?;
        if tail == ["comments"] {
            let rows=graph_comments(gh,&repo,number)?.into_iter().map(|r|Ok(json!({"id":database_id(&r["fullDatabaseId"])? ,"body":r["body"],"url":r["url"],"at":time(&r["createdAt"])}))).collect::<Result<Vec<Value>>>()?;
            return Ok(lines(rows));
        }
        if tail == ["dependencies", "blocked_by"] {
            let mut rows = vec![];
            for r in graph_blockers(gh, &repo, number)? {
                let of = r["repository"]["nameWithOwner"]
                    .as_str()
                    .context("GitHub blocker missing repository identity")?;
                rows.push(json!({"number":r["number"],"state":r["state"],"repo":of,"labels":graph_labels(gh,of,&r)?}));
            }
            return Ok(lines(rows));
        }
        if tail.is_empty() {
            // REST closed_by is not available from Issue's ordinary fields.
            // Read the final closure event, with its actor, authoritatively.
            let r = graph_issue(gh, &repo, number)?;
            let mut closed_by = Value::Null;
            if r["state"] == "CLOSED" {
                let selection = format!(
                    "resource: issue(number:{number}) {{ timelineItems(last:1, itemTypes:[CLOSED_EVENT]) {{ nodes {{ ... on ClosedEvent {{ actor {{ login }} }} }} }} }}"
                );
                let event = graph(gh, &repo, &selection, false)?
                    .pop()
                    .context("GitHub: missing closure event")?;
                closed_by = event["timelineItems"]["nodes"]
                    .as_array()
                    .and_then(|rows| rows.last())
                    .map_or(Value::Null, |r| r["actor"]["login"].clone());
            }
            let labels = graph_labels(gh, &repo, &r)?;
            let mut state =
                r["state"].as_str().context("GitHub: missing issue state")?.to_ascii_lowercase();
            if state == "merged" {
                state = "closed".into();
            }
            let reason = r["stateReason"].as_str().map(|s| s.to_ascii_lowercase());
            return Ok(json!({"id":database_id(&r["fullDatabaseId"])? ,"title":r["title"],"body":r["body"],"state":state,"state_reason":reason,"labels":labels,"pull_request":r["__typename"]=="PullRequest","closed_by":closed_by}).to_string());
        }
    }
    if path == "pulls" {
        let state = parameter(query, "state").unwrap_or("open");
        let states = if state == "open" { "[OPEN]" } else { "[OPEN,CLOSED,MERGED]" };
        let selection = format!(
            "resource: pullRequests(states:{states}, first:100, after:$endCursor, orderBy:{{field:UPDATED_AT,direction:DESC}}) {{ nodes {{ {PR} }} pageInfo {{ hasNextPage endCursor }} }}"
        );
        let rows = graph(gh, &repo, &selection, true)?;
        if jq == Some(gh::PR_HEADS_JQ) {
            return Ok(lines(rows.into_iter().filter(|r|r["isCrossRepository"]==false).map(|r|json!({"number":r["number"],"title":r["title"],"body":r["body"].as_str().unwrap_or(""),"head_branch":r["headRefName"],"head_sha":r["headRefOid"],"base_branch":r["baseRefName"]}))));
        }
        if jq == Some(gh::WATCH_PRS_JQ) {
            let page: usize = parameter(query, "page").unwrap_or("1").parse()?;
            return Ok(lines(rows.into_iter().skip((page-1)*100).take(100).map(|r|json!({"number":r["number"],"title":r["title"],"body":r["body"],"head_branch":r["headRefName"].as_str().unwrap_or(""),"updated":time(&r["updatedAt"]),"closed":time(&r["closedAt"]),"merged":time(&r["mergedAt"]),"fork":r["isCrossRepository"],"of":r["baseRepository"]["nameWithOwner"]}))));
        }
        if jq == Some(gh::DELIVERY_PRS_JQ) {
            return Ok(lines(rows.into_iter().map(|r| {
                json!({
                    "number":r["number"], "body":r["body"].as_str().unwrap_or(""),
                    "head_branch":r["headRefName"], "base_branch":r["baseRefName"],
                    "state":r["state"].as_str().map(str::to_ascii_lowercase),
                    "draft":r["isDraft"],
                    "merged": r["mergedAt"].as_str().is_some()
                        || r["state"].as_str().is_some_and(|s| s.eq_ignore_ascii_case("merged")),
                    "url":r["url"], "fork":r["isCrossRepository"]
                })
            })));
        }
    }
    if path == "commits/HEAD" {
        let r = graph(gh, &repo, "resource: defaultBranchRef { target { oid } }", false)?
            .pop()
            .context("GitHub: missing default branch")?;
        return Ok(r["target"]["oid"]
            .as_str()
            .context("GitHub: missing default branch commit")?
            .to_string());
    }
    if let Some(branch) = path.strip_prefix("branches/").and_then(|p| p.strip_suffix("/protection"))
    {
        let branch = json!(format!("refs/heads/{}", decode(branch)?));
        let selection = format!(
            "resource: ref(qualifiedName:{branch}) {{ branchProtectionRule {{ requiresStatusChecks requiredStatusCheckContexts requiresApprovingReviews restrictsPushes lockBranch }} }}"
        );
        let r = graph(gh, &repo, &selection, false)?.pop().context("GitHub: missing branch")?;
        let rule = &r["branchProtectionRule"];
        return Ok(json!({"required_status_checks":{"contexts":if rule["requiresStatusChecks"]==true {rule["requiredStatusCheckContexts"].clone()}else{json!([])}},
            "required_pull_request_reviews":if rule["requiresApprovingReviews"]==true{json!({})}else{Value::Null},
            "restrictions":if rule["restrictsPushes"]==true{json!({})}else{Value::Null},"lock_branch":{"enabled":rule["lockBranch"]==true}}).to_string());
    }
    if let Some(path) = path.strip_prefix("contents/") {
        let expression = json!(format!("HEAD:{}", decode(path)?));
        let selection = format!(
            "resource: object(expression:{expression}) {{ __typename ... on Blob {{ text isBinary isTruncated }} }}"
        );
        let r = graph_read(gh, &repo, &selection, false, true)?
            .pop()
            .context("GitHub: missing file")?;
        if r.is_null() {
            bail!("gh: Not Found (HTTP 404)");
        }
        if r["__typename"] != "Blob" || r["isBinary"] != false || r["isTruncated"] != false {
            bail!("file_of: GraphQL cannot supply the complete text blob; wait for REST reset");
        }
        return Ok(r["text"].as_str().context("GitHub: missing file text")?.to_string());
    }
    bail!(
        "gh api {path}: operation has no supported equivalent GraphQL capability; wait for REST core reset"
    )
}

fn parameter<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k == key).then_some(v)
    })
}
fn decode(text: &str) -> Result<String> {
    let mut bytes = vec![];
    let mut chars = text.bytes();
    while let Some(c) = chars.next() {
        bytes.push(if c == b'%' {
            let a = chars.next().context("invalid percent escape")?;
            let b = chars.next().context("invalid percent escape")?;
            u8::from_str_radix(std::str::from_utf8(&[a, b])?, 16)?
        } else {
            c
        });
    }
    Ok(String::from_utf8(bytes)?)
}

fn mutation(gh: &GitHub, operation: &str, input: &str, fields: &str) -> Result<Value> {
    let query = format!(
        "query=mutation ConvoyQuotaWrite {{ {operation}(input:{{{input}}}) {{ {fields} }} }}"
    );
    // One mutation, once: losing its receipt never starts another mutation.
    let out = gh.on_api(&["api", "graphql", "-f", &query], true)?;
    let doc: Value = serde_json::from_str(&out)
        .context("write receipt invalid; inspect authoritative result before retrying")?;
    if doc.get("errors").is_some_and(|e| !e.is_null() && e != &json!([])) {
        bail!(
            "write outcome uncertain; inspect authoritative result before retrying: {}",
            doc["errors"]
        );
    }
    let result = &doc["data"][operation];
    if result.is_null() {
        bail!("write receipt missing; inspect authoritative result before retrying")
    }
    Ok(result.clone())
}

fn label_ids(gh: &GitHub, repo: &str, names: &[String]) -> Result<Value> {
    if names.is_empty() {
        return Ok(json!([]));
    }
    let rows = graph(
        gh,
        repo,
        "resource: labels(first:100, after:$endCursor) { nodes { id name } pageInfo { hasNextPage endCursor } }",
        true,
    )?;
    let ids=names.iter().map(|name|{
        rows.iter().find(|r|r["name"].as_str().is_some_and(|n|n.eq_ignore_ascii_case(name)))
            .and_then(|r|r["id"].as_str()).with_context(||format!("label {name:?} has no existing GraphQL identity; no partial write attempted"))
    }).collect::<Result<Vec<_>>>()?;
    Ok(json!(ids))
}

fn write(gh: &GitHub, args: &[&str]) -> Result<String> {
    if args.first() != Some(&"api") || args.get(1) == Some(&"graphql") {
        bail!(
            "high-level write has no safe equivalent adapter; inspect authoritative result before retrying after reset"
        );
    }
    let endpoint = args
        .iter()
        .find(|s| s.starts_with("repos/"))
        .context("write endpoint has no equivalent capability")?;
    let mut parts = endpoint.trim_start_matches("repos/").splitn(3, '/');
    let repo = format!(
        "{}/{}",
        parts.next().context("missing owner")?,
        parts.next().context("missing repo")?
    );
    let path = parts.next().unwrap_or("");
    let pieces: Vec<_> = path.split('/').collect();
    let method = opt(args, "--method").or_else(|| opt(args, "-X")).unwrap_or("POST");
    let text = |name: &str| field(args, name).with_context(|| format!("write missing {name}"));
    if path == "issues" && method == "POST" {
        let repo_id = graph(gh, &repo, "resource: id", false)?
            .pop()
            .context("GitHub: missing repository id")?;
        let labels: Vec<_> =
            args.iter().filter_map(|a| a.strip_prefix("labels[]=").map(str::to_string)).collect();
        let ids = label_ids(gh, &repo, &labels)?;
        let input = format!(
            "repositoryId:{repo_id}, title:{}, body:{}, labelIds:{ids}",
            json!(text("title")?),
            json!(text("body")?)
        );
        let result = mutation(gh, "createIssue", &input, "issue { number }")?;
        return Ok(result["issue"]["number"]
            .as_u64()
            .context("issue created but number missing; inspect before retrying")?
            .to_string());
    }
    if path == "labels" && method == "POST" {
        let repo_id = graph(gh, &repo, "resource: id", false)?
            .pop()
            .context("GitHub: missing repository id")?;
        let input = format!(
            "repositoryId:{repo_id}, name:{}, description:{}, color:{}",
            json!(text("name")?),
            json!(text("description")?),
            json!(text("color")?)
        );
        mutation(gh, "createLabel", &input, "label { id }")?;
        return Ok(String::new());
    }
    if let ["issues", number, tail @ ..] = pieces.as_slice() {
        let number: u64 =
            number.parse().context("write needs an issue number; no mutation attempted")?;
        let issue = graph_issue(gh, &repo, number)?;
        let id = issue["id"]
            .as_str()
            .context("GitHub: missing issue node identity; no write attempted")?;
        if tail == ["comments"] && method == "POST" {
            mutation(
                gh,
                "addComment",
                &format!("subjectId:{}, body:{}", json!(id), json!(text("body")?)),
                "commentEdge { node { id } }",
            )?;
            return Ok(String::new());
        }
        if tail.is_empty()
            && method == "PATCH"
            && field(args, "state") == Some("closed")
            && issue["__typename"] == "Issue"
        {
            mutation(
                gh,
                "closeIssue",
                &format!("issueId:{}, stateReason:COMPLETED", json!(id)),
                "issue { id }",
            )?;
            return Ok(String::new());
        }
        if tail == ["labels"] && method == "POST" {
            let names: Vec<_> = args
                .iter()
                .filter_map(|a| a.strip_prefix("labels[]=").map(str::to_string))
                .collect();
            let ids = label_ids(gh, &repo, &names)?;
            mutation(
                gh,
                "addLabelsToLabelable",
                &format!("labelableId:{}, labelIds:{ids}", json!(id)),
                "labelable { __typename }",
            )?;
            return Ok(String::new());
        }
        if let ["labels", label] = tail
            && method == "DELETE"
        {
            let name = decode(label)?;
            let held = graph_labels(gh, &repo, &issue)?;
            if !held.as_array().is_some_and(|labels| {
                labels.iter().any(|l| l.as_str().is_some_and(|s| s.eq_ignore_ascii_case(&name)))
            }) {
                bail!("gh: Label does not exist (HTTP 404)");
            }
            let ids = label_ids(gh, &repo, &[name])?;
            mutation(
                gh,
                "removeLabelsFromLabelable",
                &format!("labelableId:{}, labelIds:{ids}", json!(id)),
                "labelable { __typename }",
            )?;
            return Ok(String::new());
        }
    }
    bail!(
        "write {method} {path} has no supported equivalent GraphQL adapter (database-id dependency/comment edits require an authoritative node identity); confirmed primary rejection was not retried; inspect authoritative result and retry after REST reset"
    )
}

/// gh list bounds its result and embeds only the first 100 native closing
/// references. At either boundary, replace its result with a complete paged
/// query, not a concatenation that can duplicate or lose changing rows.
/// Small ordinary rebuilds retain their one gh pr list request.
pub(crate) fn complete_listing(gh: &GitHub, args: &[&str], out: String) -> Result<String> {
    if !(args.starts_with(&["pr", "list"]) || args.starts_with(&["issue", "list"])) {
        return Ok(out);
    }
    let refs = opt(args, "--json") == Some(gh::PR_FIELDS);
    let rows: Vec<Value> = if opt(args, "--json") == Some("number") {
        out.lines()
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_str(s).context("gh pr list: invalid number"))
            .collect::<Result<_>>()?
    } else {
        serde_json::from_str(&out).context("gh list: invalid receipt")?
    };
    if rows.len() < 1000
        && (!refs
            || rows
                .iter()
                .all(|r| r["closingIssuesReferences"].as_array().is_some_and(|a| a.len() < 100)))
    {
        return Ok(out);
    }
    let repo = opt(args, "--repo").unwrap_or(OWN);
    if args[0] == "issue" {
        return Ok(json!(graph(gh,repo,&format!("resource: issues(states:[OPEN], first:100, after:$endCursor) {{ nodes {{ {ISSUE} }} pageInfo {{ hasNextPage endCursor }} }}"),true)?).to_string());
    }
    let merged = opt(args, "--state") == Some("merged");
    let state = if merged { "[MERGED]" } else { "[OPEN]" };
    let label =
        opt(args, "--label").map(|l| format!(", labels:[{}]", json!(l))).unwrap_or_default();
    let linked = if refs {
        "closingIssuesReferences(first:100) { nodes { number repository { name owner { login } } } pageInfo { hasNextPage endCursor } } headRepository { name } headRepositoryOwner { login }"
    } else {
        ""
    };
    let selection = format!(
        "resource: pullRequests(states:{state}{label}, first:100, after:$endCursor) {{ nodes {{ {PR} {linked} }} pageInfo {{ hasNextPage endCursor }} }}"
    );
    let mut rows = graph(gh, repo, &selection, true)?;
    if merged {
        return Ok(rows.iter().map(|r| r["number"].to_string()).collect::<Vec<_>>().join("\n"));
    }
    for row in &mut rows {
        if refs {
            let conn = &row["closingIssuesReferences"];
            let links = if conn["pageInfo"]["hasNextPage"] == true {
                let number = row["number"].as_u64().context("PR missing number")?;
                graph(
                    gh,
                    repo,
                    &format!(
                        "pullRequest(number:{number}) {{ resource: closingIssuesReferences(first:100, after:$endCursor) {{ nodes {{ number repository {{ name owner {{ login }} }} }} pageInfo {{ hasNextPage endCursor }} }} }}"
                    ),
                    true,
                )?
            } else {
                conn["nodes"].as_array().context("PR missing closing references")?.clone()
            };
            row["closingIssuesReferences"] = json!(links);
        }
        row["since"] = time(&row["createdAt"]);
    }
    Ok(json!(rows).to_string())
}
