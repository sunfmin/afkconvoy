//! Stands in for `orca` in the tests: its worktrees and terminals as one JSON
//! file (`CONVOY_FAKE_ORCA`).

mod fake;

use serde_json::json;

fn main() {
    let st = fake::State::open("CONVOY_FAKE_ORCA");
    let error = format!("fake orca: unsupported call {:?}", st.argv);
    let answer = json!({"id": "x", "ok": false, "error": {"code": error, "message": error}});
    st.finish(&answer.to_string(), 1, "");
}
